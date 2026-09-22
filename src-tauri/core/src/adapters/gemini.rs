//! Gemini CLI adapter.
//!
//! The CLI keeps its state in `%USERPROFILE%\.gemini\`: `settings.json`,
//! OAuth credentials, the signed-in Google account, and per-project session
//! logs under `tmp/<project hash>/logs.json`.
//!
//! Google does not publish a local quota figure, and the CLI does not cache
//! one. So this adapter reports what can actually be read — whether it is
//! installed, who is signed in, which projects have been active and when — and
//! says plainly that there is no local quota rather than inventing a ring.
//! If a build ever does start caching a `quota`/`limit` blob, [`parse_quota`]
//! picks it up.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, newest_files, parse_timestamp, project_label};
use crate::model::{
    Activity, Health, ProviderId, ProviderSnapshot, Session, UsageUnit, UsageWindow,
};

/// Session logs newer than this are worth showing.
const SESSION_WINDOW_HOURS: i64 = 12;
const GENERATING_WITHIN_SECS: i64 = 25;
const DONE_WITHIN_MINS: i64 = 10;
const MAX_LOGS: usize = 10;
/// These files are small; refuse anything that clearly isn't a session log.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct GeminiPaths {
    pub root: PathBuf,
}

impl GeminiPaths {
    pub fn detect() -> Option<Self> {
        home_dir().map(|h| Self {
            root: h.join(".gemini"),
        })
    }

    pub fn settings_file(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn accounts_file(&self) -> PathBuf {
        self.root.join("google_accounts.json")
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.root.join("oauth_creds.json")
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    pub fn is_installed(&self) -> bool {
        self.root.is_dir()
    }
}

/// The signed-in account, from `google_accounts.json`.
///
/// Shapes seen in the wild: `{"active": "a@b.com"}` and
/// `{"accounts": ["a@b.com"]}`. Anything else yields nothing rather than a
/// guess.
pub fn parse_account(raw: &str) -> Option<String> {
    let json: Json = serde_json::from_str(raw).ok()?;

    if let Some(active) = json.get("active").and_then(Json::as_str) {
        if active.contains('@') {
            return Some(active.to_string());
        }
    }
    json.get("accounts")
        .and_then(Json::as_array)
        .and_then(|list| list.first())
        .and_then(Json::as_str)
        .filter(|s| s.contains('@'))
        .map(str::to_string)
}

/// Whether credentials exist and, if the blob says so, when they expire.
pub fn parse_credentials(raw: &str) -> Option<Option<DateTime<Utc>>> {
    let json: Json = serde_json::from_str(raw).ok()?;
    let has_token = json
        .get("access_token")
        .or_else(|| json.get("accessToken"))
        .and_then(Json::as_str)
        .is_some_and(|s| !s.is_empty());
    if !has_token {
        return None;
    }
    Some(
        json.get("expiry_date")
            .or_else(|| json.get("expiryDate"))
            .and_then(parse_timestamp),
    )
}

/// Pull a quota out of a settings/state blob, if one is ever cached there.
///
/// Nothing in the CLI writes this today; it exists so that the day a build
/// does, the ring lights up without another release.
pub fn parse_quota(raw: &str) -> Vec<UsageWindow> {
    let Ok(json) = serde_json::from_str::<Json>(raw) else {
        return Vec::new();
    };
    let Some(map) = json.as_object() else {
        return Vec::new();
    };

    let mut windows = Vec::new();
    for (key, value) in map {
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
            .or_else(|| entry.get("requests"))
            .and_then(Json::as_f64);
        let limit = entry
            .get("limit")
            .or_else(|| entry.get("max"))
            .and_then(Json::as_f64)
            .filter(|l| *l > 0.0);

        if let Some(used) = used {
            windows.push(
                UsageWindow::new(key.clone(), "Requests")
                    .with_counts(used, limit)
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
    windows
}

/// A project's session log, reduced to when it was last touched.
#[derive(Debug, Clone, PartialEq)]
pub struct LogSummary {
    pub id: String,
    pub project: Option<String>,
    pub last_activity: Option<DateTime<Utc>>,
    pub messages: usize,
}

/// Read `tmp/<hash>/logs.json`: an array of `{sessionId, type, messageId, timestamp, message}`.
pub fn summarise_log(path: &Path) -> Option<LogSummary> {
    if std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX) > MAX_LOG_BYTES {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    let json: Json = serde_json::from_str(&raw).ok()?;
    let entries = json.as_array()?;

    let last_activity = entries
        .iter()
        .filter_map(|e| e.get("timestamp").and_then(parse_timestamp))
        .max();

    let project = entries
        .iter()
        .rev()
        .find_map(|e| e.get("cwd").and_then(Json::as_str))
        .map(project_label)
        // Otherwise the folder name is a hash of the project path, which is at
        // least stable even if it isn't readable.
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|s| s.to_string_lossy().chars().take(8).collect())
        });

    Some(LogSummary {
        id: path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".into()),
        project,
        last_activity,
        messages: entries.len(),
    })
}

fn classify(last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Activity {
    let Some(last) = last else {
        return Activity::Idle;
    };
    let age = now.signed_duration_since(last);
    if age < chrono::Duration::zero() || age < chrono::Duration::seconds(GENERATING_WITHIN_SECS) {
        Activity::Generating
    } else if age < chrono::Duration::minutes(DONE_WITHIN_MINS) {
        Activity::Done
    } else {
        Activity::Idle
    }
}

pub struct GeminiAdapter {
    paths: Option<GeminiPaths>,
}

impl GeminiAdapter {
    pub fn new() -> Self {
        Self {
            paths: GeminiPaths::detect(),
        }
    }

    pub fn with_paths(paths: GeminiPaths) -> Self {
        Self { paths: Some(paths) }
    }

    fn scan_sessions(&self, now: DateTime<Utc>) -> Vec<Session> {
        let Some(paths) = &self.paths else {
            return Vec::new();
        };
        let cutoff = now - chrono::Duration::hours(SESSION_WINDOW_HOURS);

        let mut sessions: Vec<Session> = newest_files(&paths.tmp_dir(), "json", MAX_LOGS)
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "logs.json"))
            .filter_map(|p| summarise_log(&p))
            .filter(|s| s.last_activity.is_some_and(|t| t >= cutoff))
            .map(|s| Session {
                id: s.id,
                title: s.project.clone().unwrap_or_else(|| "Gemini".into()),
                cwd: s.project,
                model: None,
                activity: classify(s.last_activity, now),
                last_activity: s.last_activity,
                tokens: None,
                detail: Some(format!("{} messages", s.messages)),
                host: None,
            })
            .collect();

        sessions.sort_by(|a, b| {
            b.activity
                .rank()
                .cmp(&a.activity.rank())
                .then(b.last_activity.cmp(&a.last_activity))
        });
        sessions.truncate(6);
        sessions
    }

    pub fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(paths) = &self.paths else {
            return ProviderSnapshot::degraded(
                ProviderId::Gemini,
                Health::Unavailable,
                "Gemini CLI not found",
            );
        };
        if !paths.is_installed() {
            return ProviderSnapshot::degraded(
                ProviderId::Gemini,
                Health::Unavailable,
                "Gemini CLI not installed",
            );
        }

        let account = std::fs::read_to_string(paths.accounts_file())
            .ok()
            .and_then(|raw| parse_account(&raw));

        let credentials = std::fs::read_to_string(paths.credentials_file())
            .ok()
            .and_then(|raw| parse_credentials(&raw));

        let windows = std::fs::read_to_string(paths.settings_file())
            .map(|raw| parse_quota(&raw))
            .unwrap_or_default();

        let sessions = self.scan_sessions(now);

        let mut snap = ProviderSnapshot::new(ProviderId::Gemini)
            .with_source(".gemini")
            .with_account(account)
            .with_windows(windows)
            .with_sessions(sessions);

        match credentials {
            None => {
                snap.health = Health::NeedsAuth;
                snap.detail = Some("Run `gemini` to sign in".into());
            }
            Some(expiry) if expiry.is_some_and(|e| now >= e) => {
                snap.health = Health::NeedsAuth;
                snap.detail = Some("Sign-in expired; run `gemini` to refresh".into());
            }
            Some(_) if snap.windows.is_empty() => {
                // Being explicit beats an empty ring the user can't interpret.
                snap.detail = Some("Gemini reports quota server-side; no local counters".into());
            }
            Some(_) => {}
        }
        snap
    }
}

impl Default for GeminiAdapter {
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
    fn reads_the_signed_in_account_from_either_shape() {
        assert_eq!(
            parse_account(r#"{"active":"dev@example.com"}"#).as_deref(),
            Some("dev@example.com")
        );
        assert_eq!(
            parse_account(r#"{"accounts":["dev@example.com","other@example.com"]}"#).as_deref(),
            Some("dev@example.com")
        );
        assert_eq!(parse_account(r#"{"active":""}"#), None);
        assert_eq!(parse_account("not json"), None);
    }

    #[test]
    fn credentials_report_presence_and_expiry() {
        let creds = parse_credentials(r#"{"access_token":"ya29.x","expiry_date":1767272400000}"#);
        assert_eq!(creds, Some(Some(at("2026-01-01T13:00:00Z"))));

        // Present but with no expiry recorded.
        assert_eq!(
            parse_credentials(r#"{"access_token":"ya29.x"}"#),
            Some(None)
        );

        // No token at all means not signed in.
        assert_eq!(parse_credentials(r#"{"refresh_token":"x"}"#), None);
        assert_eq!(parse_credentials("{}"), None);
    }

    #[test]
    fn a_quota_blob_is_picked_up_if_one_ever_appears() {
        let windows = parse_quota(r#"{"quota":{"used":120,"limit":1000}}"#);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used, Some(120.0));
        assert_eq!(windows[0].used_pct, Some(12.0));

        // Nothing quota-shaped in a normal settings file.
        assert!(parse_quota(r#"{"theme":"dark","selectedAuthType":"oauth"}"#).is_empty());
        assert!(parse_quota("not json").is_empty());
    }

    #[test]
    fn a_quota_without_a_limit_reports_a_count_but_no_percentage() {
        let windows = parse_quota(r#"{"usage":{"used":42}}"#);
        assert_eq!(windows[0].used, Some(42.0));
        assert_eq!(windows[0].used_pct, None);
    }

    fn write_log(dir: &Path, entries: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("logs.json");
        std::fs::write(&path, entries).unwrap();
        path
    }

    #[test]
    fn summarises_a_session_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_log(
            &dir.path().join("a1b2c3d4e5"),
            r#"[
              {"sessionId":"s1","timestamp":"2026-01-01T11:00:00Z","cwd":"C:\\dev\\api"},
              {"sessionId":"s1","timestamp":"2026-01-01T11:59:50Z","cwd":"C:\\dev\\api"}
            ]"#,
        );
        let summary = summarise_log(&path).unwrap();
        assert_eq!(summary.project.as_deref(), Some("api"));
        assert_eq!(summary.messages, 2);
        assert_eq!(summary.last_activity, Some(at("2026-01-01T11:59:50Z")));
        assert_eq!(
            classify(summary.last_activity, at(NOW)),
            Activity::Generating
        );
    }

    #[test]
    fn a_log_without_a_cwd_falls_back_to_the_project_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_log(
            &dir.path().join("deadbeefcafe"),
            r#"[{"timestamp":"2026-01-01T11:00:00Z"}]"#,
        );
        assert_eq!(
            summarise_log(&path).unwrap().project.as_deref(),
            Some("deadbeef")
        );
    }

    #[test]
    fn a_malformed_log_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_log(&dir.path().join("x"), "{not an array}");
        assert!(summarise_log(&path).is_none());
    }

    #[test]
    fn a_missing_install_reports_unavailable() {
        let home = tempfile::tempdir().unwrap();
        let mut adapter = GeminiAdapter::with_paths(GeminiPaths {
            root: home.path().join(".gemini"),
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Unavailable);
        assert!(snap.windows.is_empty());
    }

    #[test]
    fn an_installed_but_signed_out_cli_asks_for_sign_in() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("settings.json"), r#"{"theme":"dark"}"#).unwrap();

        let mut adapter = GeminiAdapter::with_paths(GeminiPaths {
            root: root.path().to_path_buf(),
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::NeedsAuth);
        assert!(snap.windows.is_empty(), "no auth must not invent numbers");
    }

    #[test]
    fn a_signed_in_cli_reports_the_account_and_says_why_there_is_no_ring() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("oauth_creds.json"),
            r#"{"access_token":"ya29.x","expiry_date":1798761600000}"#,
        )
        .unwrap();
        std::fs::write(
            root.path().join("google_accounts.json"),
            r#"{"active":"dev@example.com"}"#,
        )
        .unwrap();

        let mut adapter = GeminiAdapter::with_paths(GeminiPaths {
            root: root.path().to_path_buf(),
        });
        let snap = adapter.collect(at(NOW));

        assert_eq!(snap.health, Health::Ok);
        assert_eq!(snap.account.as_deref(), Some("dev@example.com"));
        assert!(snap.windows.is_empty());
        assert!(snap.detail.unwrap().contains("server-side"));
    }

    #[test]
    fn expired_credentials_ask_for_a_refresh() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("oauth_creds.json"),
            r#"{"access_token":"ya29.x","expiry_date":1767225600000}"#,
        )
        .unwrap();

        let mut adapter = GeminiAdapter::with_paths(GeminiPaths {
            root: root.path().to_path_buf(),
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::NeedsAuth);
        assert!(snap.detail.unwrap().contains("expired"));
    }
}
