//! Grok adapter.
//!
//! The Grok CLI signs in and keeps the result in `~/.grok/auth.json`, which it
//! rotates as tokens expire. This borrows that session — it never signs in
//! itself — and asks the CLI's own billing proxy what share of the plan has
//! gone.
//!
//! The file holds one entry per identity provider, keyed `<issuer>::<client>`.
//! Only entries issued by x.ai are used: an enterprise customer's own IdP can
//! write an entry here too, and its token must not be sent to a public proxy.
//! The issuer is compared as a whole segment, because a prefix test would also
//! accept `https://auth.x.ai.example.com`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, parse_timestamp, Backoff};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageWindow};

/// The only issuer whose tokens may be sent to the billing proxy.
const TRUSTED_ISSUER: &str = "https://auth.x.ai";
const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
/// The CLI identifies itself with this; the proxy rejects requests without it.
const CLIENT_HEADER: &str = "xai-grok-cli";
const TIMEOUT: Duration = Duration::from_secs(15);

/// A session the Grok CLI left behind.
#[derive(Debug, Clone, PartialEq)]
pub struct Creds {
    pub token: String,
    /// When the CLI says it expires; `None` when the entry didn't say.
    pub expires_at: Option<DateTime<Utc>>,
    pub email: Option<String>,
}

/// Whether an entry was issued by x.ai, by key or by its own field.
fn is_trusted(key: &str, entry: &Json) -> bool {
    key.split("::").next() == Some(TRUSTED_ISSUER)
        || entry.get("oidc_issuer").and_then(Json::as_str) == Some(TRUSTED_ISSUER)
}

/// The session to use, out of however many the file holds.
///
/// An unexpired entry wins; failing that the first trusted one is used anyway,
/// because the CLI may have refreshed the file since it was read and only the
/// proxy's answer settles it.
pub fn pick(root: &Json, now: DateTime<Utc>) -> Option<Creds> {
    let mut trusted: Vec<Creds> = root
        .as_object()?
        .iter()
        .filter(|(key, entry)| is_trusted(key, entry))
        .filter_map(|(_, entry)| {
            let token = entry.get("key").and_then(Json::as_str).unwrap_or_default();
            (!token.is_empty()).then(|| Creds {
                token: token.to_string(),
                expires_at: entry.get("expires_at").and_then(parse_timestamp),
                email: entry
                    .get("email")
                    .and_then(Json::as_str)
                    .map(str::to_string),
            })
        })
        .collect();

    if let Some(index) = trusted
        .iter()
        .position(|c| c.expires_at.is_none_or(|at| at > now))
    {
        return Some(trusted.swap_remove(index));
    }
    trusted.into_iter().next()
}

/// `GrokBuild` -> `Grok Build`.
fn humanise(wire: &str) -> String {
    let mut out = String::with_capacity(wire.len() + 2);
    for (i, ch) in wire.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Grok reports the share already spent, 0–100.
fn pct(value: Option<&Json>) -> Option<f32> {
    value
        .and_then(Json::as_f64)
        .map(|p| p.clamp(0.0, 100.0) as f32)
}

/// Turn a billing reply into windows.
///
/// There is no limit figure to divide by: the percentage is the whole answer.
/// The period's end is the reset, and it is a usage period rather than a
/// billing date — the plan's week, not the day the card is charged.
pub fn parse_credits(body: &str) -> Vec<UsageWindow> {
    let Ok(root) = serde_json::from_str::<Json>(body) else {
        return Vec::new();
    };
    let Some(config) = root.get("config") else {
        return Vec::new();
    };

    let current = config.get("currentPeriod");
    let resets_at = current
        .and_then(|p| p.get("end"))
        .and_then(parse_timestamp)
        .or_else(|| config.get("billingPeriodEnd").and_then(parse_timestamp));

    let weekly = current
        .and_then(|p| p.get("type"))
        .and_then(Json::as_str)
        .is_some_and(|kind| kind.to_ascii_uppercase().contains("WEEKLY"));
    let products = config.get("productUsage").and_then(Json::as_array);

    let finish = |mut window: UsageWindow| {
        window.resets_at = resets_at;
        if weekly {
            window = window.lasting(7 * 24 * 60);
        }
        window
    };

    // The headline percentage, when the account has one.
    if let Some(used) = pct(config.get("creditUsagePercent")) {
        let label = products
            .and_then(|list| list.first())
            .and_then(|product| product.get("product"))
            .and_then(Json::as_str)
            .map(humanise)
            .unwrap_or_else(|| "Grok Build".to_string());
        return vec![finish(UsageWindow::new("credits", label).with_pct(used))];
    }

    // Otherwise one per metered product.
    let mut windows: Vec<UsageWindow> = Vec::new();
    for product in products.into_iter().flatten() {
        let Some(used) = pct(product.get("usagePercent")) else {
            continue;
        };
        let wire = product.get("product").and_then(Json::as_str);
        let label = wire.map(humanise).unwrap_or_else(|| "Usage".to_string());
        // The ring reads the window called `credits`, so the first one has to
        // answer to that name whatever the product is called.
        let key = if windows.is_empty() {
            "credits".to_string()
        } else {
            wire.unwrap_or(&label).to_string()
        };
        windows.push(finish(UsageWindow::new(key, label).with_pct(used)));
    }

    // A fresh weekly period reports no percentages at all until something is
    // spent. An empty week is worth drawing; nothing at all is not.
    if windows.is_empty() && weekly {
        windows.push(finish(
            UsageWindow::new("credits", "Weekly limit").with_pct(0.0),
        ));
    }
    windows
}

/// Reads Grok's usage from the session its CLI keeps.
#[derive(Debug)]
pub struct GrokAdapter {
    http: reqwest::Client,
    backoff: Backoff,
    /// A token the proxy has already refused. Re-sending it only earns
    /// another 401, so it waits for the CLI to write a new one.
    rejected: Option<String>,
}

impl GrokAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            backoff: Backoff::default(),
            rejected: None,
        }
    }

    /// `%USERPROFILE%\.grok\auth.json`, which the CLI owns.
    fn auth_path() -> Option<std::path::PathBuf> {
        Some(home_dir()?.join(".grok").join("auth.json"))
    }

    pub fn installed() -> bool {
        Self::auth_path().is_some_and(|path| path.exists())
    }

    fn credentials(&self, now: DateTime<Utc>) -> Option<Creds> {
        let path = Self::auth_path()?;
        let raw = std::fs::read_to_string(path).ok()?;
        let root: Json = serde_json::from_str(&raw).ok()?;
        pick(&root, now)
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        if !Self::installed() {
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::Unavailable,
                "Grok CLI not found",
            );
        }

        let Some(creds) = self.credentials(now) else {
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::NeedsAuth,
                "Run `grok login` — it signs in and refreshes the token this reads",
            );
        };
        if self.rejected.as_deref() == Some(creds.token.as_str()) {
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::NeedsAuth,
                "Grok rejected this session — run `grok login` again",
            );
        }
        if !self.backoff.ready_at(now) {
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::RateLimited,
                "Grok is rate limiting; waiting before asking again",
            );
        }

        let response = self
            .http
            .get(BILLING_URL)
            .bearer_auth(&creds.token)
            .header("X-XAI-Token-Auth", CLIENT_HEADER)
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
                    ProviderId::Grok,
                    Health::Error,
                    format!("Could not reach Grok: {err}"),
                );
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            self.rejected = Some(creds.token.clone());
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::NeedsAuth,
                "Grok rejected this session — run `grok login` again",
            );
        }
        if !status.is_success() {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            let health = if status.as_u16() == 429 {
                Health::RateLimited
            } else {
                Health::Error
            };
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                health,
                format!("Grok's billing endpoint returned {status}"),
            );
        }

        let Ok(body) = response.text().await else {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::Error,
                "Grok's reply could not be read",
            );
        };
        self.backoff.record_success();
        self.rejected = None;

        let windows = parse_credits(&body);
        if windows.is_empty() {
            return ProviderSnapshot::degraded(
                ProviderId::Grok,
                Health::Ok,
                "Grok has nothing metered on this account yet",
            );
        }

        let mut snapshot = ProviderSnapshot::new(ProviderId::Grok);
        snapshot.source = Some("grok cli".to_string());
        snapshot.account = creds.email;
        snapshot.windows = windows;
        snapshot.updated_at = now;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&Utc)
    }

    /// An enterprise IdP can write an entry into the same file, and its token
    /// must never be sent to x.ai's proxy.
    #[test]
    fn only_x_ai_issued_sessions_are_used() {
        let root: Json = serde_json::from_str(
            r#"{
              "https://sso.example.com::cli": {"key": "customer", "email": "a@example.com"},
              "https://auth.x.ai::cli": {"key": "ours", "email": "b@example.com"}
            }"#,
        )
        .unwrap();
        assert_eq!(
            pick(&root, at("2026-01-01T00:00:00Z")).unwrap().token,
            "ours"
        );
    }

    /// A prefix test would let this through.
    #[test]
    fn a_lookalike_issuer_is_refused() {
        let root: Json =
            serde_json::from_str(r#"{"https://auth.x.ai.example.com::cli": {"key": "no"}}"#)
                .unwrap();
        assert!(pick(&root, at("2026-01-01T00:00:00Z")).is_none());
    }

    #[test]
    fn an_unexpired_session_is_preferred() {
        let root: Json = serde_json::from_str(
            r#"{
              "https://auth.x.ai::old": {"key": "stale", "expires_at": "2020-01-01T00:00:00Z"},
              "https://auth.x.ai::new": {"key": "fresh", "expires_at": "2030-01-01T00:00:00Z"}
            }"#,
        )
        .unwrap();
        assert_eq!(
            pick(&root, at("2026-01-01T00:00:00Z")).unwrap().token,
            "fresh"
        );
    }

    /// The CLI rotates the file, so an expired entry is still worth trying:
    /// only the proxy's answer settles it.
    #[test]
    fn an_expired_session_is_still_offered() {
        let root: Json = serde_json::from_str(
            r#"{"https://auth.x.ai::cli": {"key": "maybe", "expires_at": "2020-01-01T00:00:00Z"}}"#,
        )
        .unwrap();
        assert_eq!(
            pick(&root, at("2026-01-01T00:00:00Z")).unwrap().token,
            "maybe"
        );
    }

    #[test]
    fn the_headline_percentage_becomes_the_window() {
        let windows = parse_credits(
            r#"{"config": {
              "currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-09-27T00:00:00Z"},
              "creditUsagePercent": 8.0,
              "productUsage": [{"product": "GrokBuild", "usagePercent": 8.0}]
            }}"#,
        );
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "credits");
        assert_eq!(windows[0].label, "Grok Build");
        assert_eq!(windows[0].used_pct, Some(8.0));
        assert_eq!(windows[0].window_minutes, Some(7 * 24 * 60));
        assert_eq!(windows[0].resets_at, Some(at("2026-09-27T00:00:00Z")));
    }

    /// Without a headline figure, each metered product gets a window — and the
    /// first has to be called `credits` or the ring finds nothing to draw.
    #[test]
    fn products_stand_in_for_a_missing_headline() {
        let windows = parse_credits(
            r#"{"config": {
              "billingPeriodEnd": "2026-10-01T00:00:00Z",
              "productUsage": [
                {"product": "GrokBuild", "usagePercent": 12.0},
                {"product": "GrokChat", "usagePercent": 3.0}
              ]
            }}"#,
        );
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].key, "credits");
        assert_eq!(windows[1].key, "GrokChat");
        assert_eq!(windows[1].label, "Grok Chat");
        assert_eq!(windows[0].resets_at, Some(at("2026-10-01T00:00:00Z")));
    }

    /// A week that has just rolled over reports no percentages at all.
    #[test]
    fn a_fresh_week_is_drawn_as_empty_rather_than_missing() {
        let windows = parse_credits(
            r#"{"config": {"currentPeriod": {"type": "USAGE_PERIOD_TYPE_WEEKLY", "end": "2026-09-27T00:00:00Z"}}}"#,
        );
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_pct, Some(0.0));
        assert_eq!(windows[0].label, "Weekly limit");
    }

    #[test]
    fn a_reply_of_the_wrong_shape_draws_nothing() {
        assert!(parse_credits("not json").is_empty());
        assert!(parse_credits(r#"{"unexpected": true}"#).is_empty());
        assert!(parse_credits(r#"{"config": {}}"#).is_empty());
    }
}
