//! Command Code adapter.
//!
//! Four calls rather than one, because the billing endpoints are organised
//! around an organisation id the caller has to look up first, and the usage
//! summary needs the period start that the subscription record carries. They
//! run once every few minutes, so the round trips cost little.
//!
//! The monthly figure is money: what the period has cost against what is left
//! on the plan. Neither number is a ceiling on its own — the ceiling is the
//! two added together.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, Backoff};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageWindow};

const API: &str = "https://api.commandcode.ai/alpha";
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    pub key: String,
    pub user: Option<String>,
}

/// The key from the environment if it is set, otherwise from the CLI's file.
pub fn credentials() -> Option<Credential> {
    if let Some(key) = std::env::var("COMMAND_CODE_API_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return Some(Credential { key, user: None });
    }
    let path = home_dir()?.join(".commandcode").join("auth.json");
    let root: Json = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let key = root.get("apiKey")?.as_str()?.trim().to_string();
    (!key.is_empty()).then(|| Credential {
        key,
        user: root
            .get("userName")
            .and_then(Json::as_str)
            .map(str::to_string),
    })
}

fn number(value: Option<&Json>) -> Option<f64> {
    match value? {
        Json::Number(n) => n.as_f64(),
        Json::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// ISO-8601, or epoch seconds, or epoch milliseconds. Zero means "no reset",
/// not 1970.
fn moment(value: Option<&Json>) -> Option<DateTime<Utc>> {
    match value? {
        Json::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|at| at.with_timezone(&Utc)),
        other => {
            let raw = number(Some(other)).filter(|value| *value > 0.0)?;
            let secs = if raw > 1_000_000_000_000.0 {
                raw / 1000.0
            } else {
                raw
            };
            Utc.timestamp_opt(secs as i64, 0).single()
        }
    }
}

/// One of the rolling windows, when the plan publishes a cap for it.
fn rolling(entry: Option<&Json>, key: &str, label: &str, minutes: u32) -> Option<UsageWindow> {
    let entry = entry?;
    let cap = number(entry.get("cap")).filter(|cap| *cap > 0.0)?;
    let used = number(entry.get("used")).unwrap_or(0.0);
    let mut window = UsageWindow::new(key, label)
        .with_pct((used / cap * 100.0) as f32)
        .lasting(minutes)
        .weekly(key == "weekly");
    window.resets_at = moment(entry.get("resetAt"));
    Some(window)
}

/// Build the windows from the three replies.
pub fn parse_windows(credits: &str, subscription: &str, summary: &str) -> Vec<UsageWindow> {
    let credits: Json = serde_json::from_str(credits).unwrap_or(Json::Null);
    let subscription: Json = serde_json::from_str(subscription).unwrap_or(Json::Null);
    let summary: Json = serde_json::from_str(summary).unwrap_or(Json::Null);

    let sub = subscription.get("data").unwrap_or(&subscription);
    let spent = number(summary.get("totalCost")).unwrap_or(0.0);
    let left = number(credits.get("credits").and_then(|c| c.get("monthlyCredits"))).unwrap_or(0.0);

    let mut windows = Vec::new();
    // Neither figure is the ceiling; their sum is.
    let cap = spent + left;
    if cap > 0.0 {
        let mut monthly =
            UsageWindow::new("monthly", "Monthly limit").with_pct((spent / cap * 100.0) as f32);
        monthly.resets_at = moment(sub.get("currentPeriodEnd"));
        windows.push(monthly);
    }

    let limits = credits.get("windowLimits");
    if let Some(window) = rolling(
        limits.and_then(|l| l.get("fiveHour")),
        "fiveHour",
        "5h limit",
        5 * 60,
    ) {
        windows.push(window);
    }
    if let Some(window) = rolling(
        limits.and_then(|l| l.get("weekly")),
        "weekly",
        "Weekly limit",
        7 * 24 * 60,
    ) {
        windows.push(window);
    }
    windows
}

/// `goat-monthly` -> `GOAT`.
fn plan_name(subscription: &Json) -> Option<String> {
    let sub = subscription.get("data").unwrap_or(subscription);
    let plan = sub.get("planId")?.as_str()?;
    Some(if plan.to_lowercase().contains("goat") {
        "GOAT".to_string()
    } else {
        plan.to_string()
    })
}

/// Reads a Command Code plan through the key its CLI holds.
#[derive(Debug)]
pub struct CommandCodeAdapter {
    http: reqwest::Client,
    backoff: Backoff,
}

impl CommandCodeAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            backoff: Backoff::default(),
        }
    }

    /// One authenticated GET, returning the body.
    async fn get(&self, url: &str, key: &str) -> Result<String, Health> {
        let response = self
            .http
            .get(url)
            .bearer_auth(key)
            .header("User-Agent", "command-code-desktop")
            .header("x-command-code-version", "desktop")
            .header("Accept", "application/json")
            .timeout(TIMEOUT)
            .send()
            .await
            .map_err(|_| Health::Error)?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(Health::NeedsAuth);
        }
        if status.as_u16() == 429 {
            return Err(Health::RateLimited);
        }
        if !status.is_success() {
            return Err(Health::Error);
        }
        response.text().await.map_err(|_| Health::Error)
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(creds) = credentials() else {
            return ProviderSnapshot::degraded(
                ProviderId::CommandCode,
                Health::Unavailable,
                "Command Code not found",
            );
        };
        if !self.backoff.ready_at(now) {
            return ProviderSnapshot::degraded(
                ProviderId::CommandCode,
                Health::RateLimited,
                "Command Code is rate limiting; waiting before asking again",
            );
        }

        let fail = |health: Health| {
            ProviderSnapshot::degraded(
                ProviderId::CommandCode,
                health,
                match health {
                    Health::NeedsAuth => "Command Code rejected the key",
                    Health::RateLimited => "Command Code is rate limiting",
                    _ => "Command Code could not be read",
                },
            )
        };

        // Everything else is scoped to the organisation, so it comes first.
        let whoami = match self.get(&format!("{API}/whoami"), &creds.key).await {
            Ok(body) => body,
            Err(health) => {
                if health != Health::NeedsAuth {
                    self.backoff
                        .record_failure(now, None, rand::random::<f64>());
                }
                return fail(health);
            }
        };
        let Some(org) = serde_json::from_str::<Json>(&whoami)
            .ok()
            .and_then(|root| Some(root.get("org")?.get("id")?.as_str()?.to_string()))
        else {
            return ProviderSnapshot::degraded(
                ProviderId::CommandCode,
                Health::Error,
                "Command Code did not say which organisation this key belongs to",
            );
        };

        let credits = self
            .get(&format!("{API}/billing/credits?orgId={org}"), &creds.key)
            .await;
        let subscription = self
            .get(
                &format!("{API}/billing/subscriptions?orgId={org}"),
                &creds.key,
            )
            .await;
        let (credits, subscription) = match (credits, subscription) {
            (Ok(credits), Ok(subscription)) => (credits, subscription),
            (Err(health), _) | (_, Err(health)) => {
                if health != Health::NeedsAuth {
                    self.backoff
                        .record_failure(now, None, rand::random::<f64>());
                }
                return fail(health);
            }
        };

        // The summary counts from the start of the current period, which only
        // the subscription knows.
        let since = serde_json::from_str::<Json>(&subscription)
            .ok()
            .and_then(|root| {
                let sub = root.get("data").cloned().unwrap_or(root);
                Some(sub.get("currentPeriodStart")?.as_str()?.to_string())
            })
            .unwrap_or_default();
        let summary = self
            .get(
                &format!("{API}/usage/summary?orgId={org}&since={since}"),
                &creds.key,
            )
            .await
            .unwrap_or_default();

        self.backoff.record_success();
        let windows = parse_windows(&credits, &subscription, &summary);
        if windows.is_empty() {
            return ProviderSnapshot::degraded(
                ProviderId::CommandCode,
                Health::Ok,
                "Command Code has nothing metered on this account yet",
            );
        }

        let mut snapshot = ProviderSnapshot::new(ProviderId::CommandCode);
        snapshot.source = Some("commandcode auth".to_string());
        snapshot.account = serde_json::from_str::<Json>(&subscription)
            .ok()
            .and_then(|root| plan_name(&root))
            .or(creds.user);
        snapshot.windows = windows;
        snapshot.updated_at = now;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDITS: &str = r#"{
      "credits": {"monthlyCredits": 60},
      "windowLimits": {
        "fiveHour": {"cap": 100, "used": 25, "resetAt": 1758326400},
        "weekly": {"cap": 500, "used": 100, "resetAt": 0}
      }
    }"#;
    const SUB: &str = r#"{"data": {"planId": "goat-monthly",
      "currentPeriodStart": "2026-09-01T00:00:00.000Z",
      "currentPeriodEnd": "2026-10-01T00:00:00.000Z"}}"#;
    const SUMMARY: &str = r#"{"totalCost": 40}"#;

    /// Spent and remaining are both money; the limit is their sum.
    #[test]
    fn the_monthly_ceiling_is_spent_plus_remaining() {
        let windows = parse_windows(CREDITS, SUB, SUMMARY);
        assert_eq!(windows[0].key, "monthly");
        // 40 of 100.
        assert_eq!(windows[0].used_pct, Some(40.0));
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn the_rolling_windows_come_from_their_own_caps() {
        let windows = parse_windows(CREDITS, SUB, SUMMARY);
        assert_eq!(windows[1].key, "fiveHour");
        assert_eq!(windows[1].used_pct, Some(25.0));
        assert_eq!(windows[2].key, "weekly");
        assert_eq!(windows[2].used_pct, Some(20.0));
    }

    /// Zero is how this API says "no reset", and 1970 is not a reset time.
    #[test]
    fn a_zero_reset_is_absent_rather_than_ancient() {
        let windows = parse_windows(CREDITS, SUB, SUMMARY);
        assert_eq!(windows[2].resets_at, None);
        assert!(windows[1].resets_at.is_some());
    }

    #[test]
    fn a_window_without_a_cap_is_left_out() {
        let windows = parse_windows(
            r#"{"credits": {"monthlyCredits": 1}, "windowLimits": {"fiveHour": {"used": 5}}}"#,
            SUB,
            SUMMARY,
        );
        assert!(windows.iter().all(|w| w.key != "fiveHour"));
    }

    #[test]
    fn nothing_funded_and_nothing_spent_is_nothing_to_draw() {
        let windows = parse_windows(
            r#"{"credits": {"monthlyCredits": 0}}"#,
            SUB,
            r#"{"totalCost": 0}"#,
        );
        assert!(windows.is_empty());
    }

    #[test]
    fn the_plan_is_named_for_the_card() {
        let sub: Json = serde_json::from_str(SUB).unwrap();
        assert_eq!(plan_name(&sub).as_deref(), Some("GOAT"));
    }
}
