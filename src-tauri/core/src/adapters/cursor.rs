//! Cursor adapter.
//!
//! Cursor keeps its state in VS Code-style SQLite databases:
//!
//! * `%APPDATA%\Cursor\User\globalStorage\state.vscdb` — account, plan, and any
//!   cached quota counters;
//! * `%APPDATA%\Cursor\User\workspaceStorage\<hash>\state.vscdb` — per-project
//!   composer/chat sessions, alongside a `workspace.json` naming the folder.
//!
//! Both are read through [`crate::sqlite::ReadOnlyDb`], which parses the file
//! format directly and therefore cannot lock the database or disturb a running
//! Cursor — the failure mode that makes naive `mode=ro` connections unreliable
//! here. Committed WAL frames are included, so numbers are current rather than
//! stuck at the last checkpoint.
//!
//! Cursor reports most quota server-side, so local counters may simply be
//! absent. When they are, the adapter says so instead of showing a zeroed ring.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{parse_timestamp, project_label};
use crate::model::{
    Activity, Health, ProviderId, ProviderSnapshot, Session, UsageUnit, UsageWindow,
};
use crate::sqlite::ReadOnlyDb;

/// Cap on rows pulled from `ItemTable`; the table is small but not bounded.
/// Generous on purpose: `INSERT OR REPLACE` gives recently updated keys the
/// highest row ids, so a tight cap drops exactly the rows we care about.
const MAX_ITEMS: usize = 50_000;
/// Workspaces to inspect for live sessions, newest first.
const MAX_WORKSPACES: usize = 6;
/// Sessions newer than this are shown; older ones are noise.
const SESSION_WINDOW_HOURS: i64 = 12;
const GENERATING_WITHIN_SECS: i64 = 25;
const DONE_WITHIN_MINS: i64 = 10;

/// Where Cursor's state lives.
#[derive(Debug, Clone)]
pub struct CursorPaths {
    pub user_dir: PathBuf,
}

impl CursorPaths {
    /// `%APPDATA%\Cursor\User` on Windows.
    pub fn detect() -> Option<Self> {
        let base = dirs::config_dir()?;
        let dir = base.join("Cursor").join("User");
        Some(Self { user_dir: dir })
    }

    pub fn global_db(&self) -> PathBuf {
        self.user_dir.join("globalStorage").join("state.vscdb")
    }

    pub fn workspace_storage(&self) -> PathBuf {
        self.user_dir.join("workspaceStorage")
    }

    pub fn is_installed(&self) -> bool {
        self.user_dir.is_dir()
    }
}

/// Read an `ItemTable` into a map, or return an error the caller can surface.
pub fn read_item_table(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let mut db = ReadOnlyDb::open(path)?;
    db.key_value_table("ItemTable", MAX_ITEMS)
}

/// Pull the plan / membership label out of global state.
///
/// Cursor has moved this between a bare string key and a JSON blob written by
/// its reactive-storage layer, so both are checked.
pub fn extract_plan(items: &HashMap<String, String>) -> Option<String> {
    if let Some(raw) = items.get("cursorAuth/stripeMembershipType") {
        if let Some(label) = pretty_plan(raw.trim_matches('"')) {
            return Some(label);
        }
    }

    // Fall back to any reactive-storage blob carrying a membershipType.
    items
        .iter()
        .filter(|(k, _)| k.contains("reactiveStorage") || k.contains("applicationUser"))
        .filter_map(|(_, v)| serde_json::from_str::<Json>(v).ok())
        .find_map(|json| {
            json.get("membershipType")
                .or_else(|| json.get("stripeMembershipType"))
                .and_then(Json::as_str)
                .and_then(pretty_plan)
        })
}

fn pretty_plan(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    Some(match raw.to_ascii_lowercase().as_str() {
        "free" => "Free".into(),
        "free_trial" => "Free trial".into(),
        "pro" => "Pro".into(),
        "pro_plus" | "pro+" => "Pro+".into(),
        "ultra" => "Ultra".into(),
        "enterprise" | "team" => "Business".into(),
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
    })
}

/// The account's email, used as the card's subtitle when present.
pub fn extract_account_email(items: &HashMap<String, String>) -> Option<String> {
    items
        .get("cursorAuth/cachedEmail")
        .map(|s| s.trim_matches('"').to_string())
        .filter(|s| s.contains('@'))
}

/// Find any cached request-quota counters.
///
/// Cursor's usage lives server-side; some builds cache the `/usage` response
/// locally, keyed per model as `{"gpt-4": {"numRequests": n, "maxRequestUsage": m}}`.
/// We take whatever is there and report nothing when there isn't.
pub fn extract_usage(items: &HashMap<String, String>) -> Vec<UsageWindow> {
    let mut windows = Vec::new();

    for (key, raw) in items {
        let lower = key.to_ascii_lowercase();
        if !lower.contains("usage") && !lower.contains("quota") {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Json>(raw) else {
            continue;
        };
        collect_usage_entries(&json, &mut windows);
    }

    // Deterministic order, and the busiest window first.
    windows.sort_by(|a, b| {
        b.used_pct
            .unwrap_or(0.0)
            .total_cmp(&a.used_pct.unwrap_or(0.0))
            .then_with(|| a.key.cmp(&b.key))
    });
    windows.dedup_by(|a, b| a.key == b.key);
    windows
}

fn collect_usage_entries(json: &Json, out: &mut Vec<UsageWindow>) {
    let Some(map) = json.as_object() else { return };

    // Shape A: a top-level object of per-model counters.
    for (name, value) in map {
        let Some(entry) = value.as_object() else {
            continue;
        };
        let used = entry
            .get("numRequests")
            .or_else(|| entry.get("num_requests"))
            .or_else(|| entry.get("used"))
            .and_then(Json::as_f64);
        let limit = entry
            .get("maxRequestUsage")
            .or_else(|| entry.get("max_request_usage"))
            .or_else(|| entry.get("limit"))
            .and_then(Json::as_f64)
            // A null limit means "unlimited on this plan", not zero.
            .filter(|l| *l > 0.0);

        if let Some(used) = used {
            let resets = entry
                .get("startOfMonth")
                .or_else(|| entry.get("resets_at"))
                .and_then(parse_timestamp)
                .map(|start| start + chrono::Duration::days(30));

            out.push(
                UsageWindow::new(name.clone(), pretty_model(name))
                    .with_counts(used, limit)
                    .with_unit(UsageUnit::Requests)
                    .with_reset(resets),
            );
        }
    }
}

fn pretty_model(name: &str) -> String {
    match name {
        "gpt-4" => "GPT-4".into(),
        "gpt-3.5-turbo" => "GPT-3.5".into(),
        other => other.to_string(),
    }
}

/// A chat/composer session pulled out of workspace state.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposerSession {
    pub id: String,
    pub name: Option<String>,
    pub last_updated: Option<DateTime<Utc>>,
}

/// Parse `composer.composerData` (and the older chat-data key) into sessions.
pub fn parse_composer_data(raw: &str) -> Vec<ComposerSession> {
    let Ok(json) = serde_json::from_str::<Json>(raw) else {
        return Vec::new();
    };

    let list = json
        .get("allComposers")
        .or_else(|| json.get("composers"))
        .or_else(|| json.get("tabs"))
        .and_then(Json::as_array);

    let Some(list) = list else { return Vec::new() };

    list.iter()
        .filter_map(|item| {
            let id = item
                .get("composerId")
                .or_else(|| item.get("tabId"))
                .or_else(|| item.get("id"))
                .and_then(Json::as_str)?
                .to_string();

            let name = item
                .get("name")
                .or_else(|| item.get("chatTitle"))
                .or_else(|| item.get("title"))
                .and_then(Json::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty());

            let last_updated = item
                .get("lastUpdatedAt")
                .or_else(|| item.get("lastSendTime"))
                .or_else(|| item.get("createdAt"))
                .and_then(parse_timestamp);

            Some(ComposerSession {
                id,
                name,
                last_updated,
            })
        })
        .collect()
}

/// Read the folder a workspace points at, from its `workspace.json`.
///
/// The value is a file URI (`file:///c%3A/dev/app`), so it needs both URI
/// decoding and Windows drive-letter fixing before it reads like a path.
pub fn workspace_folder_name(workspace_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(workspace_dir.join("workspace.json")).ok()?;
    let json: Json = serde_json::from_str(&raw).ok()?;
    let uri = json
        .get("folder")
        .or_else(|| json.get("workspace"))
        .and_then(Json::as_str)?;
    Some(project_label(&decode_file_uri(uri)))
}

/// Turn a `file://` URI into something path-like.
///
/// Works on bytes throughout: percent-escapes encode UTF-8 bytes (`%C3%A3` is
/// "ã"), so they have to be collected and decoded together, and slicing the
/// `str` next to a multi-byte character would panic.
fn decode_file_uri(uri: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    let trimmed = uri.strip_prefix("file:///").unwrap_or(uri);
    let bytes = trimmed.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn classify(last_updated: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Activity {
    let Some(last) = last_updated else {
        return Activity::Idle;
    };
    let age = now.signed_duration_since(last);
    if age < chrono::Duration::seconds(GENERATING_WITHIN_SECS) {
        Activity::Generating
    } else if age < chrono::Duration::minutes(DONE_WITHIN_MINS) {
        Activity::Done
    } else {
        Activity::Idle
    }
}

/// Collects Cursor state.
pub struct CursorAdapter {
    paths: Option<CursorPaths>,
}

impl CursorAdapter {
    pub fn new() -> Self {
        Self {
            paths: CursorPaths::detect(),
        }
    }

    pub fn with_paths(paths: CursorPaths) -> Self {
        Self { paths: Some(paths) }
    }

    /// Workspace database directories, newest activity first.
    fn recent_workspaces(&self) -> Vec<PathBuf> {
        let Some(paths) = &self.paths else {
            return Vec::new();
        };
        let root = paths.workspace_storage();
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Vec::new();
        };

        let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter(|e| e.path().join("state.vscdb").is_file())
            .filter_map(|e| {
                let modified = std::fs::metadata(e.path().join("state.vscdb"))
                    .ok()?
                    .modified()
                    .ok()?;
                Some((modified, e.path()))
            })
            .collect();

        dirs.sort_by_key(|a| std::cmp::Reverse(a.0));
        dirs.into_iter()
            .take(MAX_WORKSPACES)
            .map(|(_, p)| p)
            .collect()
    }

    /// Gather sessions across the most recently touched workspaces.
    pub fn scan_sessions(&self, now: DateTime<Utc>) -> Vec<Session> {
        let cutoff = now - chrono::Duration::hours(SESSION_WINDOW_HOURS);
        let mut sessions = Vec::new();

        for dir in self.recent_workspaces() {
            let project = workspace_folder_name(&dir);
            let Ok(items) = read_item_table(&dir.join("state.vscdb")) else {
                continue;
            };

            let composers = items
                .iter()
                .filter(|(k, _)| {
                    k.starts_with("composer.composerData") || k.contains("aichat.chatdata")
                })
                .flat_map(|(_, raw)| parse_composer_data(raw));

            for composer in composers {
                if composer.last_updated.is_none_or(|t| t < cutoff) {
                    continue;
                }
                let title = composer
                    .name
                    .clone()
                    .or_else(|| project.clone())
                    .unwrap_or_else(|| "Chat".into());

                sessions.push(Session {
                    id: composer.id,
                    title,
                    cwd: project.clone(),
                    model: None,
                    activity: classify(composer.last_updated, now),
                    last_activity: composer.last_updated,
                    tokens: None,
                    detail: project.clone(),
                    host: None,
                });
            }
        }

        sessions.sort_by(|a, b| {
            b.activity
                .rank()
                .cmp(&a.activity.rank())
                .then(b.last_activity.cmp(&a.last_activity))
        });
        sessions.truncate(8);
        sessions
    }

    pub fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(paths) = &self.paths else {
            return ProviderSnapshot::degraded(
                ProviderId::Cursor,
                Health::Unavailable,
                "Cursor not found",
            );
        };
        if !paths.is_installed() {
            return ProviderSnapshot::degraded(
                ProviderId::Cursor,
                Health::Unavailable,
                "Cursor not installed",
            );
        }

        let global = paths.global_db();
        let items = match read_item_table(&global) {
            Ok(items) => items,
            Err(err) => {
                return ProviderSnapshot::degraded(
                    ProviderId::Cursor,
                    Health::Error,
                    format!("could not read state.vscdb: {err}"),
                );
            }
        };

        let plan = extract_plan(&items);
        let email = extract_account_email(&items);
        let windows = extract_usage(&items);
        let sessions = self.scan_sessions(now);

        let account = match (plan, email) {
            (Some(plan), Some(email)) => Some(format!("{plan} · {email}")),
            (Some(plan), None) => Some(plan),
            (None, Some(email)) => Some(email),
            (None, None) => None,
        };

        let mut snap = ProviderSnapshot::new(ProviderId::Cursor)
            .with_source("state.vscdb")
            .with_account(account)
            .with_windows(windows)
            .with_sessions(sessions);

        if snap.windows.is_empty() {
            // Being explicit beats an empty ring the user can't interpret.
            snap.detail = Some("Cursor reports quota server-side; no local counters".into());
        }
        snap
    }
}

impl Default for CursorAdapter {
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

    fn items(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn reads_the_plan_from_the_auth_key() {
        let m = items(&[("cursorAuth/stripeMembershipType", "pro")]);
        assert_eq!(extract_plan(&m).as_deref(), Some("Pro"));

        // Some builds store the value JSON-quoted.
        let m = items(&[("cursorAuth/stripeMembershipType", "\"free_trial\"")]);
        assert_eq!(extract_plan(&m).as_deref(), Some("Free trial"));

        let m = items(&[("cursorAuth/stripeMembershipType", "pro_plus")]);
        assert_eq!(extract_plan(&m).as_deref(), Some("Pro+"));
    }

    #[test]
    fn falls_back_to_the_reactive_storage_blob_for_the_plan() {
        let m = items(&[(
            "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser",
            r#"{"membershipType":"ultra","email":"dev@example.com"}"#,
        )]);
        assert_eq!(extract_plan(&m).as_deref(), Some("Ultra"));
    }

    #[test]
    fn an_unknown_plan_string_still_renders_readably() {
        let m = items(&[("cursorAuth/stripeMembershipType", "super_duper")]);
        assert_eq!(extract_plan(&m).as_deref(), Some("Super Duper"));

        // Empty values must not become an empty badge.
        let m = items(&[("cursorAuth/stripeMembershipType", "")]);
        assert_eq!(extract_plan(&m), None);
    }

    #[test]
    fn reads_the_account_email() {
        let m = items(&[("cursorAuth/cachedEmail", "\"dev@example.com\"")]);
        assert_eq!(
            extract_account_email(&m).as_deref(),
            Some("dev@example.com")
        );

        // Junk that isn't an address is ignored.
        let m = items(&[("cursorAuth/cachedEmail", "\"\"")]);
        assert_eq!(extract_account_email(&m), None);
    }

    #[test]
    fn extracts_cached_request_counters() {
        let m = items(&[(
            "cursor/usage",
            r#"{"gpt-4":{"numRequests":120,"maxRequestUsage":500,"startOfMonth":"2026-01-01T00:00:00Z"}}"#,
        )]);
        let windows = extract_usage(&m);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "GPT-4");
        assert_eq!(windows[0].used, Some(120.0));
        assert_eq!(windows[0].used_pct, Some(24.0));
        assert_eq!(windows[0].unit, UsageUnit::Requests);
        assert_eq!(windows[0].resets_at, Some(at("2026-01-31T00:00:00Z")));
    }

    #[test]
    fn an_unlimited_plan_reports_a_count_but_no_percentage() {
        let m = items(&[(
            "cursor/usage",
            r#"{"gpt-4":{"numRequests":900,"maxRequestUsage":null}}"#,
        )]);
        let windows = extract_usage(&m);
        assert_eq!(windows[0].used, Some(900.0));
        assert_eq!(
            windows[0].used_pct, None,
            "no limit means no percentage, not 100%"
        );
    }

    #[test]
    fn busiest_counter_sorts_first() {
        let m = items(&[(
            "cursor/usage",
            r#"{"a":{"numRequests":10,"maxRequestUsage":100},
                "b":{"numRequests":90,"maxRequestUsage":100}}"#,
        )]);
        let windows = extract_usage(&m);
        assert_eq!(windows[0].key, "b");
        assert_eq!(windows[1].key, "a");
    }

    #[test]
    fn keys_without_usage_data_are_ignored() {
        let m = items(&[
            ("unrelated/key", r#"{"numRequests":5}"#),
            ("cursor/usage", "not json"),
            ("other/quota", r#"{"x":{"nothing":1}}"#),
        ]);
        assert!(extract_usage(&m).is_empty());
    }

    #[test]
    fn parses_composer_sessions() {
        let raw = r#"{"allComposers":[
            {"composerId":"c1","name":"Refactor auth","lastUpdatedAt":1767268800000},
            {"composerId":"c2","createdAt":1767265200000}
        ]}"#;
        let sessions = parse_composer_data(raw);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "c1");
        assert_eq!(sessions[0].name.as_deref(), Some("Refactor auth"));
        assert_eq!(sessions[0].last_updated, Some(at("2026-01-01T12:00:00Z")));
        // Falls back to createdAt when there's no update time.
        assert_eq!(sessions[1].last_updated, Some(at("2026-01-01T11:00:00Z")));
    }

    #[test]
    fn parses_the_older_chat_tab_shape() {
        let raw =
            r#"{"tabs":[{"tabId":"t1","chatTitle":"Fix tests","lastSendTime":1767268800000}]}"#;
        let sessions = parse_composer_data(raw);
        assert_eq!(sessions[0].id, "t1");
        assert_eq!(sessions[0].name.as_deref(), Some("Fix tests"));
    }

    #[test]
    fn malformed_composer_data_yields_nothing() {
        assert!(parse_composer_data("not json").is_empty());
        assert!(parse_composer_data("{}").is_empty());
        // Entries with no id can't be addressed, so they're dropped.
        assert!(parse_composer_data(r#"{"allComposers":[{"name":"x"}]}"#).is_empty());
    }

    #[test]
    fn decodes_windows_file_uris() {
        assert_eq!(decode_file_uri("file:///c%3A/dev/my-app"), "c:/dev/my-app");
        assert_eq!(
            decode_file_uri("file:///c%3A/dev/with%20space"),
            "c:/dev/with space"
        );
        // A malformed escape is passed through rather than dropped.
        assert_eq!(decode_file_uri("file:///c%zz/dev"), "c%zz/dev");
    }

    #[test]
    fn decodes_multi_byte_escapes_and_survives_odd_input() {
        // UTF-8 escapes decode as one character, not one per byte.
        assert_eq!(
            decode_file_uri("file:///c%3A/Users/Kawh%C3%A3/app"),
            "c:/Users/Kawhã/app"
        );
        // A trailing escape at the very end still decodes.
        assert_eq!(decode_file_uri("file:///c%3A/x%20"), "c:/x ");
        // `%` followed by a multi-byte character used to panic on a str slice.
        assert_eq!(decode_file_uri("file:///c/%ãb"), "c/%ãb");
        assert_eq!(decode_file_uri("%"), "%");
    }

    #[test]
    fn reads_the_workspace_folder_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("workspace.json"),
            r#"{"folder":"file:///c%3A/dev/my-app"}"#,
        )
        .unwrap();
        assert_eq!(workspace_folder_name(dir.path()).as_deref(), Some("my-app"));

        // Missing or malformed files are simply unknown.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(workspace_folder_name(empty.path()), None);
    }

    #[test]
    fn activity_is_derived_from_the_last_update() {
        let now = at(NOW);
        assert_eq!(
            classify(Some(now - chrono::Duration::seconds(5)), now),
            Activity::Generating
        );
        assert_eq!(
            classify(Some(now - chrono::Duration::minutes(3)), now),
            Activity::Done
        );
        assert_eq!(
            classify(Some(now - chrono::Duration::hours(3)), now),
            Activity::Idle
        );
        assert_eq!(classify(None, now), Activity::Idle);
    }

    #[test]
    fn a_missing_install_reports_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let mut adapter = CursorAdapter::with_paths(CursorPaths {
            user_dir: dir.path().join("nope"),
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Unavailable);
    }

    #[test]
    fn an_unreadable_database_degrades_to_a_visible_error() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(global.join("state.vscdb"), b"definitely not sqlite").unwrap();

        let mut adapter = CursorAdapter::with_paths(CursorPaths {
            user_dir: dir.path().to_path_buf(),
        });
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Error);
        assert!(snap.detail.unwrap().contains("state.vscdb"));
        assert!(snap.windows.is_empty(), "an error must not invent numbers");
    }

    #[test]
    fn reads_plan_and_sessions_from_real_database_files() {
        // Uses the committed fixture, which is a genuine SQLite file.
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("simple.db");

        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::copy(&fixture, global.join("state.vscdb")).unwrap();

        let mut adapter = CursorAdapter::with_paths(CursorPaths {
            user_dir: dir.path().to_path_buf(),
        });
        let snap = adapter.collect(at(NOW));

        assert_eq!(snap.health, Health::Ok);
        assert_eq!(
            snap.account.as_deref(),
            Some("Pro · dev@example.com"),
            "plan and email come from the real ItemTable rows"
        );
        // The fixture has no usage counters, so the card explains itself.
        assert!(snap.windows.is_empty());
        assert!(snap.detail.unwrap().contains("server-side"));
    }

    #[test]
    fn workspace_sessions_are_collected_and_ranked() {
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures");
        let dir = tempfile::tempdir().unwrap();

        let global = dir.path().join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::copy(fixture_dir.join("simple.db"), global.join("state.vscdb")).unwrap();

        // Two workspaces, each with its own composer database.
        let ws = dir.path().join("workspaceStorage");
        for (name, folder) in [("ws-a", "my-app"), ("ws-b", "other")] {
            let d = ws.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::copy(fixture_dir.join("composers.db"), d.join("state.vscdb")).unwrap();
            std::fs::write(
                d.join("workspace.json"),
                format!(r#"{{"folder":"file:///c%3A/dev/{folder}"}}"#),
            )
            .unwrap();
        }

        let adapter = CursorAdapter::with_paths(CursorPaths {
            user_dir: dir.path().to_path_buf(),
        });
        // The fixture's composer timestamps are anchored to this instant.
        let sessions = adapter.scan_sessions(at("2026-01-01T12:00:00Z"));

        assert!(!sessions.is_empty(), "composer sessions should be found");
        assert_eq!(
            sessions[0].activity,
            Activity::Generating,
            "the most recently updated composer ranks first"
        );
        assert!(sessions.iter().any(|s| s.cwd.as_deref() == Some("my-app")));
    }
}
