//! GitHub Copilot adapter.
//!
//! Copilot's editor plugins and CLI cache the account's quota snapshot on disk
//! after talking to `/copilot_internal/user`. Reading that cache gives plan and
//! quota without asking the user for another token — which is the point: this
//! adapter never authenticates and never makes a network call.
//!
//! Locations checked, in order:
//!
//! * `%LOCALAPPDATA%\github-copilot\` — editor plugin state (`apps.json`,
//!   `versions.json`, and any cached user/quota payloads);
//! * `%USERPROFILE%\.config\github-copilot\` — the XDG-style path some builds use;
//! * `%USERPROFILE%\.copilot\` — the Copilot CLI.
//!
//! When no quota cache exists we report the sign-in state rather than a number.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, parse_timestamp};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageUnit, UsageWindow};

/// Cap on files inspected per directory; these folders are small.
const MAX_FILES: usize = 40;
/// Don't slurp anything huge looking for a quota blob.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Directories that may hold Copilot state.
#[derive(Debug, Clone, Default)]
pub struct CopilotPaths {
    pub dirs: Vec<PathBuf>,
}

impl CopilotPaths {
    pub fn detect() -> Self {
        let mut dirs = Vec::new();
        // %LOCALAPPDATA% on Windows; the closest equivalent elsewhere.
        if let Some(local) = dirs::data_local_dir() {
            dirs.push(local.join("github-copilot"));
        }
        if let Some(home) = home_dir() {
            dirs.push(home.join(".config").join("github-copilot"));
            dirs.push(home.join(".copilot"));
        }
        Self { dirs }
    }

    pub fn installed(&self) -> bool {
        self.dirs.iter().any(|d| d.is_dir())
    }
}

/// One quota bucket as Copilot reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Quota {
    pub name: String,
    pub entitlement: Option<f64>,
    pub remaining: Option<f64>,
    pub percent_remaining: Option<f64>,
    pub unlimited: bool,
}

/// What we could learn from the caches on disk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CopilotState {
    pub plan: Option<String>,
    pub user: Option<String>,
    pub quotas: Vec<Quota>,
    pub quota_resets_at: Option<DateTime<Utc>>,
    pub signed_in: bool,
}

/// Parse a cached `/copilot_internal/user` style payload.
pub fn parse_user_payload(raw: &str) -> Option<CopilotState> {
    let json: Json = serde_json::from_str(raw).ok()?;

    let plan = json
        .get("copilot_plan")
        .or_else(|| json.get("copilotPlan"))
        .and_then(Json::as_str)
        .map(pretty_plan);

    let snapshots = json
        .get("quota_snapshots")
        .or_else(|| json.get("quotaSnapshots"))
        .and_then(Json::as_object);

    let quotas = snapshots
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| {
                    let obj = value.as_object()?;
                    Some(Quota {
                        name: name.clone(),
                        entitlement: obj.get("entitlement").and_then(Json::as_f64),
                        remaining: obj.get("remaining").and_then(Json::as_f64),
                        percent_remaining: obj
                            .get("percent_remaining")
                            .or_else(|| obj.get("percentRemaining"))
                            .and_then(Json::as_f64),
                        unlimited: obj
                            .get("unlimited")
                            .and_then(Json::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // A payload with neither plan nor quota isn't the file we're looking for.
    if plan.is_none() && quotas.is_empty() {
        return None;
    }

    Some(CopilotState {
        plan,
        user: json
            .get("login")
            .or_else(|| json.get("user"))
            .and_then(Json::as_str)
            .map(str::to_string),
        quotas,
        quota_resets_at: json
            .get("quota_reset_date")
            .or_else(|| json.get("quotaResetDate"))
            .and_then(parse_date_or_timestamp),
        signed_in: true,
    })
}

/// `quota_reset_date` is a bare `YYYY-MM-DD`, unlike every other time field.
fn parse_date_or_timestamp(value: &Json) -> Option<DateTime<Utc>> {
    if let Some(s) = value.as_str() {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return date.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
        }
    }
    parse_timestamp(value)
}

fn pretty_plan(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "free" => "Free".into(),
        "individual" | "copilot_individual" => "Individual".into(),
        "business" | "copilot_business" => "Business".into(),
        "enterprise" | "copilot_enterprise" => "Enterprise".into(),
        "individual_pro" | "pro" => "Pro".into(),
        "pro_plus" => "Pro+".into(),
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

/// Detect an authenticated Copilot from `apps.json` / `hosts.json`.
///
/// Only the presence of an entry is read; the tokens inside are deliberately
/// left alone — CodeNotch has no use for them.
pub fn parse_apps_file(raw: &str) -> Option<String> {
    let json: Json = serde_json::from_str(raw).ok()?;
    let map = json.as_object()?;
    map.values()
        .filter_map(|v| v.get("user").and_then(Json::as_str))
        .map(str::to_string)
        .next()
        // An entry with no user still proves a sign-in happened.
        .or_else(|| (!map.is_empty()).then(|| "signed in".to_string()))
}

/// Scan a directory's JSON files for a quota cache.
pub fn scan_dir(dir: &Path) -> Option<CopilotState> {
    if !dir.is_dir() {
        return None;
    }

    let mut state: Option<CopilotState> = None;
    let mut signed_in_as: Option<String> = None;

    let entries = walkdir::WalkDir::new(dir)
        .max_depth(2)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .take(MAX_FILES);

    for entry in entries {
        let path = entry.path();
        let is_json = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"));
        if !is_json {
            continue;
        }
        if entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };

        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name == "apps.json" || name == "hosts.json" {
            signed_in_as = signed_in_as.or_else(|| parse_apps_file(&raw));
            continue;
        }

        if let Some(parsed) = parse_user_payload(&raw) {
            // Prefer whichever cache actually carries quota numbers.
            let better = state
                .as_ref()
                .is_none_or(|existing| existing.quotas.is_empty() && !parsed.quotas.is_empty());
            if better {
                state = Some(parsed);
            }
        }
    }

    match (state, signed_in_as) {
        (Some(mut s), user) => {
            s.user = s.user.or(user);
            Some(s)
        }
        (None, Some(user)) => Some(CopilotState {
            user: Some(user),
            signed_in: true,
            ..Default::default()
        }),
        (None, None) => None,
    }
}

/// Convert quota buckets into usage windows.
///
/// Copilot reports what's *left*, so it has to be inverted into "used" for a
/// ring that fills up as you burn through the allowance.
pub fn quota_windows(state: &CopilotState) -> Vec<UsageWindow> {
    let mut windows: Vec<UsageWindow> = state
        .quotas
        .iter()
        .filter(|q| !q.unlimited)
        .filter_map(|q| {
            let label = pretty_quota_name(&q.name);
            let mut window = UsageWindow::new(q.name.clone(), label).with_unit(UsageUnit::Requests);

            match (q.entitlement, q.remaining) {
                (Some(entitlement), Some(remaining)) if entitlement > 0.0 => {
                    window = window.with_counts(entitlement - remaining, Some(entitlement));
                }
                _ => {
                    // Only a percentage is available: invert "remaining".
                    let pct = q.percent_remaining?;
                    window = window.with_pct((100.0 - pct) as f32);
                }
            }
            Some(window.with_reset(state.quota_resets_at))
        })
        .collect();

    windows.sort_by(|a, b| {
        b.used_pct
            .unwrap_or(0.0)
            .total_cmp(&a.used_pct.unwrap_or(0.0))
    });
    windows
}

fn pretty_quota_name(raw: &str) -> String {
    match raw {
        "chat" => "Chat".into(),
        "completions" => "Completions".into(),
        "premium_interactions" => "Premium".into(),
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

/// Collects Copilot state.
pub struct CopilotAdapter {
    paths: CopilotPaths,
}

impl CopilotAdapter {
    pub fn new() -> Self {
        Self {
            paths: CopilotPaths::detect(),
        }
    }

    pub fn with_paths(paths: CopilotPaths) -> Self {
        Self { paths }
    }

    pub fn collect(&mut self, _now: DateTime<Utc>) -> ProviderSnapshot {
        if !self.paths.installed() {
            return ProviderSnapshot::degraded(
                ProviderId::Copilot,
                Health::Unavailable,
                "Copilot not found",
            );
        }

        let state = self.paths.dirs.iter().find_map(|d| scan_dir(d));

        let Some(state) = state else {
            return ProviderSnapshot::degraded(
                ProviderId::Copilot,
                Health::NeedsAuth,
                "Sign in to Copilot in your editor",
            );
        };

        let windows = quota_windows(&state);
        let account = match (&state.plan, &state.user) {
            (Some(plan), Some(user)) => Some(format!("{plan} · {user}")),
            (Some(plan), None) => Some(plan.clone()),
            (None, Some(user)) => Some(user.clone()),
            (None, None) => None,
        };

        let mut snap = ProviderSnapshot::new(ProviderId::Copilot)
            .with_source("github-copilot cache")
            .with_account(account)
            .with_windows(windows);

        if snap.windows.is_empty() {
            let unlimited = state.quotas.iter().any(|q| q.unlimited);
            snap.detail = Some(if unlimited {
                "Unlimited on this plan".into()
            } else {
                "No cached quota; open Copilot Chat to refresh it".into()
            });
        }
        snap
    }
}

impl Default for CopilotAdapter {
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

    const USER_PAYLOAD: &str = r#"{
      "copilot_plan": "individual",
      "login": "octocat",
      "quota_reset_date": "2026-02-01",
      "quota_snapshots": {
        "chat":        {"entitlement": 300, "remaining": 210, "percent_remaining": 70.0, "unlimited": false},
        "completions": {"entitlement": 0,   "remaining": 0,   "percent_remaining": 100.0, "unlimited": true},
        "premium_interactions": {"entitlement": 50, "remaining": 5, "percent_remaining": 10.0, "unlimited": false}
      }
    }"#;

    #[test]
    fn parses_a_cached_user_payload() {
        let state = parse_user_payload(USER_PAYLOAD).expect("should parse");
        assert_eq!(state.plan.as_deref(), Some("Individual"));
        assert_eq!(state.user.as_deref(), Some("octocat"));
        assert_eq!(state.quotas.len(), 3);
        assert_eq!(state.quota_resets_at, Some(at("2026-02-01T00:00:00Z")));
    }

    #[test]
    fn quota_windows_invert_remaining_into_used() {
        let state = parse_user_payload(USER_PAYLOAD).unwrap();
        let windows = quota_windows(&state);

        // Unlimited buckets are dropped; the rest sort busiest-first.
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "Premium");
        assert_eq!(windows[0].used, Some(45.0), "50 entitlement - 5 remaining");
        assert_eq!(windows[0].used_pct, Some(90.0));

        assert_eq!(windows[1].label, "Chat");
        assert_eq!(windows[1].used, Some(90.0));
        assert_eq!(windows[1].used_pct, Some(30.0));
        assert_eq!(windows[1].resets_at, Some(at("2026-02-01T00:00:00Z")));
    }

    #[test]
    fn a_percentage_only_quota_still_works() {
        let raw =
            r#"{"copilot_plan":"business","quota_snapshots":{"chat":{"percent_remaining":25.0}}}"#;
        let state = parse_user_payload(raw).unwrap();
        let windows = quota_windows(&state);
        assert_eq!(windows[0].used_pct, Some(75.0));
        assert_eq!(windows[0].used, None, "no counts were reported");
    }

    #[test]
    fn plan_names_are_humanised() {
        assert_eq!(pretty_plan("copilot_business"), "Business");
        assert_eq!(pretty_plan("individual"), "Individual");
        assert_eq!(pretty_plan("brand_new_tier"), "Brand New Tier");
    }

    #[test]
    fn irrelevant_json_is_not_mistaken_for_a_quota_cache() {
        assert!(parse_user_payload(r#"{"version":"1.2.3"}"#).is_none());
        assert!(parse_user_payload("not json").is_none());
        assert!(parse_user_payload("{}").is_none());
    }

    #[test]
    fn apps_file_reports_the_signed_in_user_without_touching_tokens() {
        let raw = r#"{"github.com:Iv1.abc123":{"user":"octocat","oauth_token":"gho_secret"}}"#;
        assert_eq!(parse_apps_file(raw).as_deref(), Some("octocat"));

        // An entry without a user still proves sign-in.
        let raw = r#"{"github.com:Iv1.abc":{"oauth_token":"x"}}"#;
        assert_eq!(parse_apps_file(raw).as_deref(), Some("signed in"));

        assert_eq!(parse_apps_file("{}"), None);
    }

    #[test]
    fn scans_a_directory_for_the_quota_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("versions.json"), r#"{"version":"1.0"}"#).unwrap();
        std::fs::write(
            dir.path().join("apps.json"),
            r#"{"github.com:Iv1.x":{"user":"octocat"}}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("user.json"), USER_PAYLOAD).unwrap();

        let state = scan_dir(dir.path()).expect("state found");
        assert_eq!(state.plan.as_deref(), Some("Individual"));
        assert_eq!(state.quotas.len(), 3);
        assert!(state.signed_in);
    }

    #[test]
    fn a_signed_in_install_with_no_cache_still_reports_the_user() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("apps.json"),
            r#"{"github.com:Iv1.x":{"user":"octocat"}}"#,
        )
        .unwrap();

        let state = scan_dir(dir.path()).unwrap();
        assert_eq!(state.user.as_deref(), Some("octocat"));
        assert!(state.quotas.is_empty());

        let mut adapter = CopilotAdapter::with_paths(CopilotPaths {
            dirs: vec![dir.path().to_path_buf()],
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Ok);
        assert_eq!(snap.account.as_deref(), Some("octocat"));
        assert!(snap.windows.is_empty());
        assert!(snap.detail.unwrap().contains("No cached quota"));
    }

    #[test]
    fn a_missing_install_reports_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = CopilotAdapter::with_paths(CopilotPaths {
            dirs: vec![dir.path().join("nope")],
        });
        assert_eq!(adapter.collect(at(NOW)).health, Health::Unavailable);
    }

    #[test]
    fn an_installed_but_signed_out_copilot_asks_for_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("versions.json"), r#"{"version":"1.0"}"#).unwrap();

        let mut adapter = CopilotAdapter::with_paths(CopilotPaths {
            dirs: vec![dir.path().to_path_buf()],
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::NeedsAuth);
        assert!(snap.windows.is_empty());
    }

    #[test]
    fn an_entirely_unlimited_plan_says_so() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("user.json"),
            r#"{"copilot_plan":"enterprise","quota_snapshots":{"chat":{"unlimited":true}}}"#,
        )
        .unwrap();

        let mut adapter = CopilotAdapter::with_paths(CopilotPaths {
            dirs: vec![dir.path().to_path_buf()],
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.account.as_deref(), Some("Enterprise"));
        assert_eq!(snap.detail.as_deref(), Some("Unlimited on this plan"));
    }
}
