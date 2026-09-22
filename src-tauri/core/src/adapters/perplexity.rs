//! Perplexity adapter.
//!
//! Perplexity's desktop app and Comet browser keep Electron-style state under
//! `%APPDATA%\Perplexity\` (and `%LOCALAPPDATA%\Perplexity\`). Unlike the
//! coding CLIs there is no local usage or quota record: the subscription is
//! tracked server-side and nothing is cached to disk that says how much of it
//! is left.
//!
//! So this adapter is deliberately modest. It reports whether Perplexity is
//! installed and, where the app has cached a plan or account, which one — and
//! otherwise says there is no local quota. It does **not** log in, scrape the
//! website, or read browser cookies to manufacture a number.
//!
//! [`parse_subscription`] will pick up a plan/quota blob if a future build
//! caches one, in the same tolerant way the other adapters handle drift.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, parse_timestamp};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageUnit, UsageWindow};

/// Cap on files inspected per directory; these folders hold caches we skip.
const MAX_FILES: usize = 60;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct PerplexityPaths {
    pub dirs: Vec<PathBuf>,
}

impl PerplexityPaths {
    pub fn detect() -> Self {
        let mut dirs = Vec::new();
        // %APPDATA% on Windows, then %LOCALAPPDATA%.
        if let Some(roaming) = dirs::config_dir() {
            dirs.push(roaming.join("Perplexity"));
            dirs.push(roaming.join("Comet"));
        }
        if let Some(local) = dirs::data_local_dir() {
            dirs.push(local.join("Perplexity"));
        }
        if let Some(home) = home_dir() {
            dirs.push(home.join(".perplexity"));
        }
        Self { dirs }
    }

    pub fn installed(&self) -> bool {
        self.dirs.iter().any(|d| d.is_dir())
    }
}

/// What a cached account blob can tell us.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Subscription {
    pub plan: Option<String>,
    pub email: Option<String>,
    pub windows: Vec<UsageWindow>,
}

impl Subscription {
    fn is_empty(&self) -> bool {
        self.plan.is_none() && self.email.is_none() && self.windows.is_empty()
    }
}

fn pretty_plan(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "free" => "Free".into(),
        "pro" => "Pro".into(),
        "max" => "Max".into(),
        "enterprise" | "enterprise_pro" => "Enterprise".into(),
        other => other
            .split(['_', '-'])
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// Parse a cached subscription/account payload.
///
/// Returns `None` when the file has nothing recognisable, so a directory full
/// of Electron caches doesn't produce a phantom account.
pub fn parse_subscription(raw: &str) -> Option<Subscription> {
    let json: Json = serde_json::from_str(raw).ok()?;

    let plan = json
        .get("subscription_status")
        .or_else(|| json.get("subscriptionStatus"))
        .or_else(|| json.get("plan"))
        .or_else(|| json.get("tier"))
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())
        .map(pretty_plan);

    let email = json
        .get("email")
        .or_else(|| json.get("username"))
        .and_then(Json::as_str)
        .filter(|s| s.contains('@'))
        .map(str::to_string);

    // Perplexity counts "Pro searches" per day when it counts anything.
    let mut windows = Vec::new();
    for (key, value) in json.as_object()?.iter() {
        let lower = key.to_ascii_lowercase();
        if !lower.contains("quota") && !lower.contains("usage") && !lower.contains("limit") {
            continue;
        }
        let Some(entry) = value.as_object() else {
            continue;
        };

        let used = entry
            .get("used")
            .or_else(|| entry.get("count"))
            .and_then(Json::as_f64);
        let limit = entry
            .get("limit")
            .or_else(|| entry.get("max"))
            .and_then(Json::as_f64)
            .filter(|l| *l > 0.0);
        let remaining = entry.get("remaining").and_then(Json::as_f64);

        // Some payloads report what is left rather than what is spent.
        let spent = match (used, remaining, limit) {
            (Some(used), _, _) => Some(used),
            (None, Some(remaining), Some(limit)) => Some(limit - remaining),
            _ => None,
        };

        if let Some(spent) = spent {
            windows.push(
                UsageWindow::new(key.clone(), "Pro searches")
                    .with_counts(spent.max(0.0), limit)
                    .with_unit(UsageUnit::Requests)
                    .with_reset(
                        entry
                            .get("resets_at")
                            .or_else(|| entry.get("resetsAt"))
                            .and_then(parse_timestamp),
                    ),
            );
        }
    }
    windows.sort_by(|a, b| a.key.cmp(&b.key));

    let subscription = Subscription {
        plan,
        email,
        windows,
    };
    (!subscription.is_empty()).then_some(subscription)
}

/// Look through a directory's JSON for anything account-shaped.
pub fn scan_dir(dir: &Path) -> Option<Subscription> {
    if !dir.is_dir() {
        return None;
    }

    let entries = walkdir::WalkDir::new(dir)
        .max_depth(2)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .take(MAX_FILES);

    let mut best: Option<Subscription> = None;
    for entry in entries {
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        if let Some(found) = parse_subscription(&raw) {
            // Prefer whichever file actually carries numbers.
            let better = best
                .as_ref()
                .is_none_or(|b| b.windows.is_empty() && !found.windows.is_empty());
            if better {
                best = Some(found);
            }
        }
    }
    best
}

pub struct PerplexityAdapter {
    paths: PerplexityPaths,
}

impl PerplexityAdapter {
    pub fn new() -> Self {
        Self {
            paths: PerplexityPaths::detect(),
        }
    }

    pub fn with_paths(paths: PerplexityPaths) -> Self {
        Self { paths }
    }

    pub fn collect(&mut self, _now: DateTime<Utc>) -> ProviderSnapshot {
        if !self.paths.installed() {
            return ProviderSnapshot::degraded(
                ProviderId::Perplexity,
                Health::Unavailable,
                "Perplexity not found",
            );
        }

        let found = self.paths.dirs.iter().find_map(|d| scan_dir(d));

        let account = found.as_ref().and_then(|s| match (&s.plan, &s.email) {
            (Some(plan), Some(email)) => Some(format!("{plan} · {email}")),
            (Some(plan), None) => Some(plan.clone()),
            (None, Some(email)) => Some(email.clone()),
            (None, None) => None,
        });

        let mut snap = ProviderSnapshot::new(ProviderId::Perplexity)
            .with_source("app state")
            .with_account(account)
            .with_windows(found.map(|s| s.windows).unwrap_or_default());

        if snap.windows.is_empty() {
            snap.detail = Some("Perplexity keeps usage server-side; nothing local to read".into());
        }
        snap
    }
}

impl Default for PerplexityAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-01-01T12:00:00Z";

    #[test]
    fn reads_a_cached_plan_and_account() {
        let raw = r#"{"subscription_status":"pro","email":"dev@example.com"}"#;
        let sub = parse_subscription(raw).unwrap();
        assert_eq!(sub.plan.as_deref(), Some("Pro"));
        assert_eq!(sub.email.as_deref(), Some("dev@example.com"));
        assert!(sub.windows.is_empty());
    }

    #[test]
    fn a_quota_expressed_as_remaining_is_converted_to_spent() {
        let raw = r#"{"plan":"pro","quota":{"remaining":180,"limit":300}}"#;
        let sub = parse_subscription(raw).unwrap();
        assert_eq!(sub.windows.len(), 1);
        assert_eq!(sub.windows[0].used, Some(120.0));
        assert_eq!(sub.windows[0].used_pct, Some(40.0));
    }

    #[test]
    fn a_quota_expressed_as_used_is_taken_directly() {
        let raw = r#"{"usage":{"used":25,"limit":100,"resets_at":"2026-01-02T00:00:00Z"}}"#;
        let sub = parse_subscription(raw).unwrap();
        assert_eq!(sub.windows[0].used_pct, Some(25.0));
        assert_eq!(sub.windows[0].resets_at, Some(at("2026-01-02T00:00:00Z")));
    }

    #[test]
    fn electron_cache_files_are_not_mistaken_for_an_account() {
        assert!(parse_subscription(r#"{"version":3,"cache":[1,2,3]}"#).is_none());
        assert!(parse_subscription("not json").is_none());
        assert!(parse_subscription("{}").is_none());
        // An empty plan string is not a plan.
        assert!(parse_subscription(r#"{"plan":""}"#).is_none());
    }

    #[test]
    fn a_missing_install_reports_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = PerplexityAdapter::with_paths(PerplexityPaths {
            dirs: vec![dir.path().join("nope")],
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Unavailable);
    }

    #[test]
    fn an_install_with_no_account_data_says_so_rather_than_guessing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Preferences.json"), r#"{"window":{"x":1}}"#).unwrap();

        let mut adapter = PerplexityAdapter::with_paths(PerplexityPaths {
            dirs: vec![dir.path().to_path_buf()],
        });
        let snap = adapter.collect(at(NOW));

        assert_eq!(snap.health, Health::Ok);
        assert!(snap.windows.is_empty(), "no data must not become a 0% ring");
        assert!(snap.detail.unwrap().contains("server-side"));
    }

    #[test]
    fn a_cached_account_surfaces_on_the_card() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("user.json"),
            r#"{"subscription_status":"max","email":"dev@example.com"}"#,
        )
        .unwrap();

        let mut adapter = PerplexityAdapter::with_paths(PerplexityPaths {
            dirs: vec![dir.path().to_path_buf()],
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.account.as_deref(), Some("Max · dev@example.com"));
    }
}
