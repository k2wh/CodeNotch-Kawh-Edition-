//! Kimi Code adapter.
//!
//! The Kimi CLI signs in and writes a short-lived token — about fifteen
//! minutes — into its own credentials file, refreshing it as it goes. This
//! reads that file on every poll rather than holding the token, because a
//! copy kept in memory is stale almost immediately.
//!
//! Nothing here refreshes anything: the CLI owns the file, and an expired
//! token means the CLI hasn't run lately, not that something is broken.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, Backoff};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageWindow};

const USAGE_URL: &str = "https://api.kimi.com/coding/v1/usages";
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    pub token: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl Credential {
    fn expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|at| at <= now)
    }
}

/// `%USERPROFILE%\.kimi-code\credentials\kimi-code.json`, or wherever
/// `KIMI_CODE_HOME` points instead.
pub fn auth_path() -> Option<std::path::PathBuf> {
    let root = std::env::var("KIMI_CODE_HOME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| Some(home_dir()?.join(".kimi-code")))?;
    Some(root.join("credentials").join("kimi-code.json"))
}

pub fn parse_credential(raw: &str) -> Option<Credential> {
    let root: Json = serde_json::from_str(raw).ok()?;
    let token = root.get("access_token")?.as_str()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    Some(Credential {
        token,
        // Seconds here; other providers in this tree use milliseconds.
        expires_at: root
            .get("expires_at")
            .and_then(Json::as_i64)
            .filter(|secs| *secs > 0)
            .and_then(|secs| Utc.timestamp_opt(secs, 0).single()),
    })
}

/// Counts arrive as decimal strings as often as numbers.
fn count(value: Option<&Json>) -> Option<i64> {
    match value? {
        Json::Number(number) => number.as_i64(),
        Json::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn reset_at(detail: &Json) -> Option<DateTime<Utc>> {
    let text = detail.get("resetTime")?.as_str()?;
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// One row, from whichever object holds the counts.
fn row(key: &str, label: &str, detail: &Json, minutes: u32) -> Option<UsageWindow> {
    let used = count(detail.get("used"))?;
    let mut window = UsageWindow::new(key, label)
        .lasting(minutes)
        .weekly(key == "weekly");
    window.resets_at = reset_at(detail);

    match count(detail.get("limit")).filter(|limit| *limit > 0) {
        // A ceiling makes it a gauge.
        Some(limit) => {
            window = window.with_pct((used as f64 / limit as f64 * 100.0) as f32);
        }
        // Without one, the count is all there is to say.
        None => {
            window.used = Some(used as f64);
            window.unit = crate::model::UsageUnit::Requests;
        }
    }
    Some(window)
}

/// Which lane a `limits[]` entry describes.
fn lane(window: Option<&Json>) -> Option<(&'static str, &'static str, u32)> {
    let window = window?;
    let duration = count(window.get("duration")).filter(|d| *d > 0)?;
    match window.get("timeUnit")?.as_str()? {
        "TIME_UNIT_MINUTE" if duration % 60 == 0 && duration / 60 == 5 => {
            Some(("rolling", "5h limit", (duration) as u32))
        }
        "TIME_UNIT_HOUR" if duration == 5 => Some(("rolling", "5h limit", 5 * 60)),
        "TIME_UNIT_WEEK" if duration == 1 => Some(("weekly", "Weekly limit", 7 * 24 * 60)),
        _ => None,
    }
}

pub fn parse_usage(body: &str) -> Vec<UsageWindow> {
    let Ok(root) = serde_json::from_str::<Json>(body) else {
        return Vec::new();
    };
    let mut windows = Vec::new();

    // The summary object is the week; the plan doesn't label it as one.
    if let Some(summary) = root.get("usage") {
        if let Some(window) = row("weekly", "Weekly limit", summary, 7 * 24 * 60) {
            windows.push(window);
        }
    }
    for entry in root
        .get("limits")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        let Some((key, label, minutes)) = lane(entry.get("window")) else {
            continue;
        };
        let Some(detail) = entry.get("detail") else {
            continue;
        };
        if let Some(window) = row(key, label, detail, minutes) {
            // The summary already gave us the week.
            if !windows.iter().any(|existing| existing.key == window.key) {
                windows.push(window);
            }
        }
    }

    // The short window leads: it is the one that bites first.
    windows.sort_by_key(|w| u8::from(w.key != "rolling"));
    windows
}

/// `LEVEL_ADVANCED` -> `Advanced`.
fn plan_name(root: &Json) -> Option<String> {
    let level = root
        .get("user")?
        .get("membership")?
        .get("level")?
        .as_str()?
        .trim_start_matches("LEVEL_");
    let mut chars = level.chars();
    let first = chars.next()?;
    Some(format!(
        "{}{}",
        first.to_uppercase(),
        chars.as_str().to_lowercase()
    ))
}

/// Reads a Kimi Code plan through the CLI's own session.
#[derive(Debug)]
pub struct KimiAdapter {
    http: reqwest::Client,
    backoff: Backoff,
}

impl KimiAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            backoff: Backoff::default(),
        }
    }

    pub fn installed() -> bool {
        auth_path().is_some_and(|path| path.exists())
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(path) = auth_path().filter(|path| path.exists()) else {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::Unavailable,
                "Kimi Code not found",
            );
        };
        let Some(creds) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| parse_credential(&raw))
        else {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::NeedsAuth,
                "Run `kimi` and `/login` — it writes the token this reads",
            );
        };
        if creds.expired(now) {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::NeedsAuth,
                "Kimi's token has expired — run `kimi` to refresh it",
            );
        }
        if !self.backoff.ready_at(now) {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::RateLimited,
                "Kimi is rate limiting; waiting before asking again",
            );
        }

        let response = self
            .http
            .get(USAGE_URL)
            .bearer_auth(&creds.token)
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
                    ProviderId::Kimi,
                    Health::Error,
                    format!("Could not reach Kimi: {err}"),
                );
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::NeedsAuth,
                "Kimi rejected the session — run `kimi` and `/login` again",
            );
        }
        // Not a failure: the account simply has no coding plan.
        if status == reqwest::StatusCode::NOT_FOUND {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::Ok,
                "No Kimi Code plan on this account",
            );
        }
        if !status.is_success() {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                if status.as_u16() == 429 {
                    Health::RateLimited
                } else {
                    Health::Error
                },
                format!("Kimi's usage endpoint returned {status}"),
            );
        }

        let Ok(body) = response.text().await else {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::Error,
                "Kimi's reply could not be read",
            );
        };
        self.backoff.record_success();

        let windows = parse_usage(&body);
        if windows.is_empty() {
            return ProviderSnapshot::degraded(
                ProviderId::Kimi,
                Health::Ok,
                "Kimi has nothing metered on this account yet",
            );
        }

        let mut snapshot = ProviderSnapshot::new(ProviderId::Kimi);
        snapshot.source = Some("kimi cli".to_string());
        snapshot.account = serde_json::from_str::<Json>(&body)
            .ok()
            .and_then(|root| plan_name(&root));
        snapshot.windows = windows;
        snapshot.updated_at = now;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "user": {"userId": "u", "membership": {"level": "LEVEL_ADVANCED"}},
      "usage": {"limit": "100", "used": "2", "remaining": "98",
                "resetTime": "2026-09-15T19:39:34.389610Z"},
      "limits": [{"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                  "detail": {"limit": "100", "used": "8", "remaining": "92",
                             "resetTime": "2026-09-11T16:39:34.389610Z"}}]
    }"#;

    #[test]
    fn both_lanes_come_back_with_the_short_one_first() {
        let windows = parse_usage(SAMPLE);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].key, "rolling");
        assert_eq!(windows[0].used_pct, Some(8.0));
        assert_eq!(windows[0].window_minutes, Some(300));
        assert_eq!(windows[1].key, "weekly");
        assert_eq!(windows[1].used_pct, Some(2.0));
        assert!(windows[1].weekly);
    }

    /// Six fractional digits: a parser that only accepts whole seconds drops
    /// the reset time entirely.
    #[test]
    fn the_reset_time_keeps_its_fractional_seconds() {
        let windows = parse_usage(SAMPLE);
        assert_eq!(
            windows[0].resets_at,
            Some(
                DateTime::parse_from_rfc3339("2026-09-11T16:39:34.389610Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    /// Counts arrive as strings; treating them as numbers loses every row.
    #[test]
    fn counts_written_as_strings_still_count() {
        let windows = parse_usage(
            r#"{"usage": {"limit": 50, "used": 25, "resetTime": "2026-09-15T00:00:00Z"}}"#,
        );
        assert_eq!(windows[0].used_pct, Some(50.0));
    }

    /// Without a ceiling there is no gauge, only a tally.
    #[test]
    fn a_row_without_a_limit_reports_the_count() {
        let windows = parse_usage(r#"{"usage": {"used": "7"}}"#);
        assert_eq!(windows[0].used_pct, None);
        assert_eq!(windows[0].used, Some(7.0));
    }

    #[test]
    fn the_plan_name_is_tidied_up() {
        let root: Json = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(plan_name(&root).as_deref(), Some("Advanced"));
    }

    #[test]
    fn an_expired_token_is_recognised() {
        let creds = parse_credential(r#"{"access_token": "t", "expires_at": 1600000000}"#).unwrap();
        assert!(creds.expired(Utc.timestamp_opt(1_700_000_000, 0).unwrap()));
        assert!(!creds.expired(Utc.timestamp_opt(1_500_000_000, 0).unwrap()));
    }

    #[test]
    fn a_credential_without_a_token_is_no_credential() {
        assert!(parse_credential(r#"{"access_token": ""}"#).is_none());
        assert!(parse_credential(r#"{"expires_at": 1}"#).is_none());
    }
}
