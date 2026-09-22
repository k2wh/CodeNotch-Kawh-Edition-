//! OpenCode (Go plan) adapter.
//!
//! OpenCode keeps one auth file for every provider it can talk to, keyed by
//! name. Only its own `opencode-go` entry is read: the neighbouring `openai`
//! and `google` entries are the user's keys for those vendors, and sending one
//! of those anywhere would be borrowing a credential for a purpose its owner
//! never agreed to.

use std::time::Duration;

use chrono::{DateTime, Datelike, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, Backoff};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageWindow};

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
const TIMEOUT: Duration = Duration::from_secs(15);

/// Where OpenCode keeps its auth file. The Go build follows the XDG layout
/// even on Windows, but a native build would use `%APPDATA%`, so both are
/// tried.
pub fn auth_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("opencode")
                .join("auth.json"),
        );
    }
    if let Some(roaming) = dirs::config_dir() {
        paths.push(roaming.join("opencode").join("auth.json"));
    }
    paths
}

/// The Go plan's key, out of the file's many entries.
pub fn parse_key(raw: &str) -> Option<String> {
    let root: Json = serde_json::from_str(raw).ok()?;
    let entry = root.get("opencode-go")?;
    match entry {
        Json::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Json::Object(_) => ["key", "apiKey", "api_key", "token", "accessToken"]
            .iter()
            .find_map(|name| {
                let text = entry.get(*name)?.as_str()?.trim();
                (!text.is_empty()).then(|| text.to_string())
            }),
        _ => None,
    }
}

pub fn credentials() -> Option<String> {
    auth_paths()
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .find_map(|raw| parse_key(&raw))
}

/// A month back from the reset, since the plan's month isn't 30 days.
fn monthly_minutes(reset: Option<DateTime<Utc>>) -> Option<u32> {
    let reset = reset?;
    let month = if reset.month() == 1 {
        12
    } else {
        reset.month() - 1
    };
    let year = if reset.month() == 1 {
        reset.year() - 1
    } else {
        reset.year()
    };
    let start = reset.with_year(year)?.with_month(month)?;
    Some((reset - start).num_minutes() as u32)
}

pub fn parse_usage(body: &str) -> Vec<UsageWindow> {
    let Ok(root) = serde_json::from_str::<Json>(body) else {
        return Vec::new();
    };
    let Some(usage) = root.get("usage") else {
        return Vec::new();
    };

    let lanes: [(&str, &str); 3] = [
        ("rolling", "5h limit"),
        ("weekly", "Weekly limit"),
        ("monthly", "Monthly limit"),
    ];

    lanes
        .iter()
        .filter_map(|(key, label)| {
            let entry = usage.get(*key)?;
            let percent = entry.get("percent").and_then(Json::as_f64)?;
            let resets_at = entry
                .get("resetsAt")
                .and_then(Json::as_str)
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|at| at.with_timezone(&Utc));

            let mut window = UsageWindow::new(*key, *label)
                .with_pct(percent.clamp(0.0, 100.0) as f32)
                .weekly(*key == "weekly");
            window.resets_at = resets_at;
            let minutes = match *key {
                "rolling" => Some(5 * 60),
                "weekly" => Some(7 * 24 * 60),
                _ => monthly_minutes(resets_at),
            };
            if let Some(minutes) = minutes {
                window = window.lasting(minutes);
            }
            Some(window)
        })
        .collect()
}

/// Reads an OpenCode Go plan through the key OpenCode already holds.
#[derive(Debug)]
pub struct OpenCodeAdapter {
    http: reqwest::Client,
    backoff: Backoff,
}

impl OpenCodeAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            backoff: Backoff::default(),
        }
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(key) = credentials() else {
            return ProviderSnapshot::degraded(
                ProviderId::OpenCode,
                Health::Unavailable,
                "No OpenCode Go plan key found",
            );
        };
        if !self.backoff.ready_at(now) {
            return ProviderSnapshot::degraded(
                ProviderId::OpenCode,
                Health::RateLimited,
                "OpenCode is rate limiting; waiting before asking again",
            );
        }

        let response = self
            .http
            .get(USAGE_URL)
            .bearer_auth(&key)
            .header("Accept", "application/json")
            .timeout(TIMEOUT)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(err) => {
                self.backoff
                    .record_failure(now, None, rand::random::<f64>());
                return ProviderSnapshot::degraded(
                    ProviderId::OpenCode,
                    Health::Error,
                    format!("Could not reach OpenCode: {err}"),
                );
            }
        };

        let status = response.status();
        // The same 401 covers a bad key and an account with no Go plan.
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return ProviderSnapshot::degraded(
                ProviderId::OpenCode,
                Health::NeedsAuth,
                "OpenCode rejected the key, or this account has no Go plan",
            );
        }
        if !status.is_success() {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::OpenCode,
                if status.as_u16() == 429 {
                    Health::RateLimited
                } else {
                    Health::Error
                },
                format!("OpenCode's usage endpoint returned {status}"),
            );
        }

        let Ok(body) = response.text().await else {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::OpenCode,
                Health::Error,
                "OpenCode's reply could not be read",
            );
        };
        self.backoff.record_success();

        let windows = parse_usage(&body);
        if windows.is_empty() {
            return ProviderSnapshot::degraded(
                ProviderId::OpenCode,
                Health::Ok,
                "OpenCode has nothing metered on this account yet",
            );
        }

        let mut snapshot = ProviderSnapshot::new(ProviderId::OpenCode);
        snapshot.source = Some("opencode auth".to_string());
        snapshot.account = Some("Go".to_string());
        snapshot.windows = windows;
        snapshot.updated_at = now;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file holds the user's keys for other vendors too.
    #[test]
    fn only_the_go_plans_entry_is_taken() {
        let raw =
            r#"{"openai": {"key": "sk-not-ours"}, "opencode-go": {"type": "api", "key": "ours"}}"#;
        assert_eq!(parse_key(raw).as_deref(), Some("ours"));
        assert!(parse_key(r#"{"openai": {"key": "sk-not-ours"}}"#).is_none());
    }

    #[test]
    fn the_entry_may_be_a_bare_string() {
        assert_eq!(
            parse_key(r#"{"opencode-go": "bare"}"#).as_deref(),
            Some("bare")
        );
        assert!(parse_key(r#"{"opencode-go": "   "}"#).is_none());
    }

    #[test]
    fn all_three_lanes_come_back_in_order() {
        let windows = parse_usage(
            r#"{"usage": {
              "rolling": {"status": "ok", "percent": 12, "resetsAt": "2026-09-06T12:31:06.611Z"},
              "weekly":  {"status": "ok", "percent": 34, "resetsAt": "2026-09-07T00:00:00.611Z"},
              "monthly": {"status": "ok", "percent": 56, "resetsAt": "2026-10-03T13:09:45.611Z"}
            }}"#,
        );
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].key, "rolling");
        assert_eq!(windows[0].used_pct, Some(12.0));
        assert_eq!(windows[0].window_minutes, Some(300));
        assert!(windows[1].weekly);
        // September is 30 days, and the plan's month ends where it ends.
        assert_eq!(windows[2].window_minutes, Some(30 * 24 * 60));
    }

    /// Milliseconds in the timestamp: a parser without them drops every reset.
    #[test]
    fn the_reset_times_keep_their_milliseconds() {
        let windows = parse_usage(
            r#"{"usage": {"rolling": {"percent": 1, "resetsAt": "2026-09-06T12:31:06.611Z"}}}"#,
        );
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn a_reply_of_the_wrong_shape_draws_nothing() {
        assert!(parse_usage("nonsense").is_empty());
        assert!(parse_usage(r#"{"usage": {}}"#).is_empty());
    }
}
