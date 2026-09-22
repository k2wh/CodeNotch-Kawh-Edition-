//! Codex CLI adapter.
//!
//! Codex writes rollout transcripts under `%USERPROFILE%\.codex\sessions\`, and
//! usefully embeds the server's own rate-limit snapshot in its `token_count`
//! events. That means we get real percentages and reset windows without any
//! extra authentication — we just read the newest snapshot it recorded.
//!
//! Multiple profiles are supported the way Codex itself does it: `~/.codex` plus
//! any `~/.codex-<slug>` sibling directories.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{
    first_line, home_dir, newest_files, newest_mtime, parse_timestamp, project_label, tail_lines,
};
use crate::model::{
    Activity, Health, ProviderId, ProviderSnapshot, Session, UsageUnit, UsageWindow,
};

const MAX_TRANSCRIPTS: usize = 10;
const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;
const SESSION_WINDOW_HOURS: i64 = 12;
const GENERATING_WITHIN_SECS: i64 = 25;
const DONE_WITHIN_MINS: i64 = 10;

/// A Codex profile directory (`~/.codex`, `~/.codex-work`, ...).
#[derive(Debug, Clone)]
pub struct CodexProfile {
    pub root: PathBuf,
    /// `None` for the default profile.
    pub name: Option<String>,
}

impl CodexProfile {
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }
}

/// Find every Codex profile under `home`.
pub fn discover_profiles(home: &Path) -> Vec<CodexProfile> {
    let mut out = Vec::new();

    let default = home.join(".codex");
    if default.is_dir() {
        out.push(CodexProfile {
            root: default,
            name: None,
        });
    }

    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(slug) = name.strip_prefix(".codex-") {
                // A directory is a profile because Codex signed in or wrote
                // rollouts there, not because of what it is called: plugins
                // and scratch folders share the same prefix.
                let root = entry.path();
                let is_profile = root.join("auth.json").is_file() || root.join("sessions").is_dir();
                if root.is_dir() && is_profile && !slug.is_empty() {
                    out.push(CodexProfile {
                        root: entry.path(),
                        name: Some(slug.to_string()),
                    });
                }
            }
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Turn a rate-limit window's length in minutes into a short label.
fn window_label(minutes: Option<i64>) -> String {
    match minutes {
        Some(m) if m % (60 * 24) == 0 => format!("{}d", m / (60 * 24)),
        Some(m) if m % 60 == 0 => format!("{}h", m / 60),
        Some(m) => format!("{m}m"),
        None => "window".to_string(),
    }
}

/// Parse the `rate_limits` object Codex records verbatim from the API.
///
/// Shape: `{"primary": {"used_percent": 12.3, "window_minutes": 300,
/// "resets_in_seconds": 900}, "secondary": {...}}`.
pub fn parse_rate_limits(value: &Json, now: DateTime<Utc>) -> Vec<UsageWindow> {
    let Some(map) = value.as_object() else {
        return Vec::new();
    };

    let mut windows: Vec<(i64, UsageWindow)> = Vec::new();

    for (key, entry) in map {
        let Some(obj) = entry.as_object() else {
            continue;
        };

        let pct = obj
            .get("used_percent")
            .or_else(|| obj.get("usedPercent"))
            .and_then(Json::as_f64);
        let Some(pct) = pct else { continue };

        let minutes = obj
            .get("window_minutes")
            .or_else(|| obj.get("windowMinutes"))
            .and_then(Json::as_i64);

        let resets_at = obj
            .get("resets_in_seconds")
            .or_else(|| obj.get("resetsInSeconds"))
            .and_then(Json::as_i64)
            .map(|secs| now + chrono::Duration::seconds(secs))
            .or_else(|| {
                obj.get("resets_at")
                    .or_else(|| obj.get("resetsAt"))
                    .and_then(parse_timestamp)
            });

        windows.push((
            // Sort by window length so the short window leads; unknown last.
            minutes.unwrap_or(i64::MAX),
            {
                let mut window = UsageWindow::new(key.clone(), window_label(minutes))
                    .with_pct(pct as f32)
                    .with_reset(resets_at)
                    .weekly(minutes == Some(7 * 24 * 60));
                if let Some(minutes) = minutes.filter(|m| *m > 0) {
                    window = window.lasting(minutes as u32);
                }
                window
            },
        ));
    }

    windows.sort_by_key(|(minutes, _)| *minutes);
    windows.into_iter().map(|(_, w)| w).collect()
}

/// When a rollout line was written.
pub fn line_stamp(line: &str) -> Option<DateTime<Utc>> {
    let json: Json = serde_json::from_str(line).ok()?;
    json.get("timestamp").and_then(parse_timestamp)
}

/// What a rollout line cost.
///
/// `last_token_usage` is this turn's cost; `total_token_usage` beside it is
/// the session's running total, which would count everything again on every
/// line.
pub fn line_usage(line: &str) -> Option<crate::ledger::Usage> {
    let json: Json = serde_json::from_str(line).ok()?;
    let event = json
        .get("payload")
        .filter(|p| p.is_object())
        .unwrap_or(&json);
    let usage = event.get("info")?.get("last_token_usage")?;
    let count = |field: &str| usage.get(field).and_then(Json::as_u64).unwrap_or(0);

    let cached = count("cached_input_tokens");
    let spent = crate::ledger::Usage {
        // `input_tokens` is the whole prompt, cache hits included.
        input: count("input_tokens").saturating_sub(cached),
        output: count("output_tokens"),
        cache_write: count("cache_write_input_tokens"),
        // OpenAI's cache has no lifetime to choose and no separate charge.
        cache_write_1h: 0,
        cache_read: cached,
        // Rollouts report a turn's usage once, on one line; there is nothing
        // to tell apart.
        id: None,
    };
    (spent.total() > 0).then_some(spent)
}

/// The model a rollout line declares, for pricing what the session spends.
///
/// Codex names it on lines of its own — session state, turn context — never
/// on the line that reports usage, so the ledger carries the last one seen
/// forward through the file.
pub fn line_model(line: &str) -> Option<String> {
    let json: Json = serde_json::from_str(line).ok()?;
    let event = json
        .get("payload")
        .filter(|p| p.is_object())
        .unwrap_or(&json);
    let model = find_model(event, 0)?;
    (!model.is_empty() && model.len() < 64).then_some(model)
}

/// Walk an event looking for a `model` string, wherever it has been nested
/// this release. Bounded, so a deep payload can't turn into a long walk.
fn find_model(value: &Json, depth: u8) -> Option<String> {
    if depth > 4 {
        return None;
    }
    match value {
        Json::Object(map) => {
            if let Some(model) = map.get("model").and_then(Json::as_str) {
                return Some(model.to_string());
            }
            map.values().find_map(|v| find_model(v, depth + 1))
        }
        Json::Array(items) => items.iter().find_map(|v| find_model(v, depth + 1)),
        _ => None,
    }
}

/// What one rollout transcript tells us.
#[derive(Debug, Clone, PartialEq)]
pub struct RolloutSummary {
    pub id: String,
    pub project: Option<String>,
    pub model: Option<String>,
    pub total_tokens: Option<u64>,
    pub last_activity: Option<DateTime<Utc>>,
    pub activity: Activity,
    pub rate_limits: Vec<UsageWindow>,
    /// Where the session was started (`Codex Desktop`, `codex_vscode`,
    /// `codex_exec`, ...), from the `session_meta` line.
    pub originator: Option<String>,
}

/// The `session_meta` line carries the base instructions, so it can be large.
const SESSION_META_MAX_BYTES: u64 = 512 * 1024;

/// Where a rollout's session was started, from its first line.
fn rollout_originator(path: &Path) -> Option<String> {
    let line = first_line(path, SESSION_META_MAX_BYTES)?;
    let json: Json = serde_json::from_str(&line).ok()?;
    let meta = json.get("payload").unwrap_or(&json);
    meta.get("originator")
        .and_then(Json::as_str)
        .map(str::to_string)
}

/// Read a rollout transcript's tail: the newest rate-limit snapshot, the token
/// total, and whether the agent is mid-flight or waiting on an approval.
pub fn summarise_rollout(path: &Path, now: DateTime<Utc>) -> Option<RolloutSummary> {
    let lines = tail_lines(path, TRANSCRIPT_TAIL_BYTES).ok()?;
    if lines.is_empty() {
        return None;
    }

    let mut last_activity = None;
    let mut rate_limits = Vec::new();
    let mut total_tokens = None;
    let mut model = None;
    let mut project = None;
    // Tracks an approval prompt: a tool call with no matching output after it.
    let mut pending_call = false;
    let mut pending_since: Option<DateTime<Utc>> = None;
    // When the turn ended, per Codex's own `task_complete`.
    let mut turn_end: Option<DateTime<Utc>> = None;

    for line in &lines {
        let Ok(json) = serde_json::from_str::<Json>(line) else {
            continue;
        };

        let line_ts = json.get("timestamp").and_then(parse_timestamp);
        if let Some(ts) = line_ts {
            last_activity = Some(ts);
        }

        // Current Codex wraps each event: `{"type":"event_msg","payload":{"type":
        // "token_count","info":..,"rate_limits":..}}`. Older builds wrote the
        // event fields at the top level, so fall back to the line itself.
        let event = json
            .get("payload")
            .filter(|p| p.is_object())
            .unwrap_or(&json);

        if let Some(limits) = event.get("rate_limits") {
            // `resets_in_seconds` is relative to when the line was written,
            // not to this poll, or an old snapshot's countdown never moves.
            let parsed = parse_rate_limits(limits, line_ts.unwrap_or(now));
            if !parsed.is_empty() {
                rate_limits = parsed; // keep only the newest snapshot
            }
        }

        if let Some(info) = event.get("info") {
            if let Some(total) = info
                .get("total_token_usage")
                .and_then(|u| u.get("total_tokens"))
                .and_then(Json::as_u64)
            {
                total_tokens = Some(total);
            }
            if let Some(m) = info.get("model").and_then(Json::as_str) {
                model = Some(m.to_string());
            }
        }

        // Session metadata line, written when the session starts.
        if let Some(payload) = json.get("payload").or(Some(&json)) {
            if let Some(cwd) = payload.get("cwd").and_then(Json::as_str) {
                project = Some(project_label(cwd));
            }
            if let Some(m) = payload.get("model").and_then(Json::as_str) {
                model.get_or_insert_with(|| m.to_string());
            }
        }

        match event.get("type").and_then(Json::as_str) {
            Some("function_call") | Some("local_shell_call") => {
                pending_call = true;
                pending_since = line_ts.or(pending_since);
            }
            Some("function_call_output") | Some("local_shell_call_output") => {
                pending_call = false;
                pending_since = None;
            }
            // Codex says outright when a turn starts and ends, which beats
            // guessing from how long the file has been quiet.
            Some("task_started") => turn_end = None,
            Some("task_complete") | Some("turn_aborted") => turn_end = line_ts.or(last_activity),
            _ => {}
        }
    }

    let activity = classify(pending_call, pending_since, turn_end, last_activity, now);

    Some(RolloutSummary {
        id: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "session".into()),
        project,
        model,
        total_tokens,
        last_activity,
        activity,
        rate_limits,
        originator: rollout_originator(path),
    })
}

/// How long a tool call runs unanswered before it reads as an approval
/// prompt rather than a slow command.
const PERMISSION_AFTER_SECS: i64 = 7;

/// What the rollout says the session is doing, most telling signal first:
/// the turn's own end record, then an unanswered tool call, then how long
/// the file has been quiet.
fn classify(
    pending_call: bool,
    pending_since: Option<DateTime<Utc>>,
    turn_end: Option<DateTime<Utc>>,
    last_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Activity {
    let Some(last) = last_activity else {
        return Activity::Idle;
    };
    let age = now.signed_duration_since(last);
    if age < chrono::Duration::zero() {
        return Activity::Generating;
    }

    let settled = |at: DateTime<Utc>| {
        if now.signed_duration_since(at) < chrono::Duration::minutes(DONE_WITHIN_MINS) {
            Activity::Done
        } else {
            Activity::Idle
        }
    };

    // Codex said the turn was over, and `task_started` would have cleared it.
    if let Some(end) = turn_end {
        return settled(end);
    }

    if pending_call {
        let waited = pending_since.map_or(age, |at| now.signed_duration_since(at));
        if waited >= chrono::Duration::seconds(PERMISSION_AFTER_SECS)
            && waited < chrono::Duration::hours(1)
        {
            return Activity::AwaitingInput;
        }
    }

    if age < chrono::Duration::seconds(GENERATING_WITHIN_SECS) {
        Activity::Generating
    } else {
        settled(last)
    }
}

/// Collects Codex usage.
pub struct CodexAdapter {
    home: Option<PathBuf>,
    /// One profile's worth, when this adapter speaks for a single account.
    /// `None` means every profile it can find, which is what a machine with
    /// one account wants and what the tests exercise.
    only: Option<CodexProfile>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        Self {
            home: home_dir(),
            only: None,
        }
    }

    /// An adapter for one account, so two accounts draw two rings instead of
    /// one ring with both their sessions in it.
    pub fn for_profile(profile: CodexProfile) -> Self {
        Self {
            home: home_dir(),
            only: Some(profile),
        }
    }

    /// The profiles this adapter speaks for.
    fn profiles(&self) -> Vec<CodexProfile> {
        if let Some(only) = &self.only {
            return vec![only.clone()];
        }
        self.home
            .as_ref()
            .map(|home| discover_profiles(home))
            .unwrap_or_default()
    }

    /// The rollouts worth following for token spending.
    pub fn transcripts(&self) -> Vec<PathBuf> {
        self.profiles()
            .iter()
            .flat_map(|p| newest_files(&p.sessions_dir(), "jsonl", MAX_TRANSCRIPTS))
            .collect()
    }

    /// When a rollout was last written; see `ClaudeAdapter::activity_mark`.
    pub fn activity_mark(&self) -> Option<std::time::SystemTime> {
        self.profiles()
            .iter()
            .filter_map(|p| newest_mtime(&p.sessions_dir(), "jsonl"))
            .max()
    }

    pub fn with_home(home: PathBuf) -> Self {
        Self {
            home: Some(home),
            only: None,
        }
    }

    /// Sessions only, for the frequent activity pass. Reads the same rollouts
    /// as `collect`, which is all local: no network, no account.
    pub fn scan_sessions(&self, now: DateTime<Utc>) -> Vec<Session> {
        let Some(home) = &self.home else {
            return Vec::new();
        };
        self.scan(&discover_profiles(home), now).0
    }

    /// Read every profile's recent rollouts into sessions, and the freshest
    /// rate-limit snapshot across them.
    fn scan(
        &self,
        profiles: &[CodexProfile],
        now: DateTime<Utc>,
    ) -> (Vec<Session>, Vec<UsageWindow>) {
        let cutoff = now - chrono::Duration::hours(SESSION_WINDOW_HOURS);
        let mut windows: Vec<UsageWindow> = Vec::new();
        let mut newest_limits_at: Option<DateTime<Utc>> = None;
        let mut sessions = Vec::new();

        for profile in profiles {
            let transcripts = newest_files(&profile.sessions_dir(), "jsonl", MAX_TRANSCRIPTS);
            for path in transcripts {
                let Some(summary) = summarise_rollout(&path, now) else {
                    continue;
                };

                // Keep the rate limits from whichever transcript is freshest:
                // they're account-wide, so the newest snapshot is the truth.
                if !summary.rate_limits.is_empty() && summary.last_activity > newest_limits_at {
                    newest_limits_at = summary.last_activity;
                    windows = summary.rate_limits.clone();
                }

                if summary.last_activity.is_none_or(|t| t < cutoff) {
                    continue;
                }

                let title = match (&summary.project, &profile.name) {
                    (Some(p), Some(n)) => format!("{p} ({n})"),
                    (Some(p), None) => p.clone(),
                    (None, Some(n)) => format!("Codex ({n})"),
                    (None, None) => "Codex".to_string(),
                };

                sessions.push(Session {
                    id: summary.id,
                    title,
                    cwd: summary.project.clone(),
                    model: summary.model,
                    activity: summary.activity,
                    last_activity: summary.last_activity,
                    tokens: summary.total_tokens,
                    detail: None,
                    host: summary.originator,
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
        (sessions, windows)
    }

    pub fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let profiles = self.profiles();
        if profiles.is_empty() {
            return ProviderSnapshot::degraded(
                ProviderId::Codex,
                Health::Unavailable,
                "Codex not found",
            );
        }

        let (sessions, mut windows) = self.scan(&profiles, now);

        // A total token count is worth showing even with no rate-limit snapshot,
        // but it's derived locally so it's flagged as an estimate.
        if windows.is_empty() {
            let tokens: u64 = sessions.iter().filter_map(|s| s.tokens).sum();
            if tokens > 0 {
                windows.push(
                    UsageWindow::new("tokens", "Tokens")
                        .with_counts(tokens as f64, None)
                        .with_unit(UsageUnit::Tokens)
                        .estimated(),
                );
            }
        }

        let account = profiles
            .iter()
            .filter_map(|p| p.name.clone())
            .collect::<Vec<_>>()
            .join(", ");

        let mut snap = ProviderSnapshot::new(ProviderId::Codex)
            .with_source("sessions")
            .with_account(Some(account).filter(|s| !s.is_empty()))
            .with_windows(windows)
            .with_sessions(sessions);

        if snap.windows.is_empty() {
            snap.health = Health::Stale;
            snap.detail = Some("No recent Codex activity to read limits from".into());
        }
        snap
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-01-01T12:00:00Z";

    #[test]
    fn parses_the_rate_limit_snapshot() {
        let json: Json = serde_json::from_str(
            r#"{
              "primary":   {"used_percent": 12.5, "window_minutes": 300,   "resets_in_seconds": 900},
              "secondary": {"used_percent": 60.0, "window_minutes": 10080, "resets_in_seconds": 3600}
            }"#,
        )
        .unwrap();

        let windows = parse_rate_limits(&json, at(NOW));
        assert_eq!(windows.len(), 2);

        // Shortest window first.
        assert_eq!(windows[0].key, "primary");
        assert_eq!(windows[0].label, "5h");
        assert_eq!(windows[0].used_pct, Some(12.5));
        assert_eq!(windows[0].resets_at, Some(at("2026-01-01T12:15:00Z")));

        assert_eq!(windows[1].label, "7d");
        assert_eq!(windows[1].used_pct, Some(60.0));
    }

    #[test]
    fn window_labels_read_naturally() {
        assert_eq!(window_label(Some(300)), "5h");
        assert_eq!(window_label(Some(10080)), "7d");
        assert_eq!(window_label(Some(45)), "45m");
        assert_eq!(window_label(None), "window");
    }

    #[test]
    fn rate_limits_without_a_percentage_are_skipped() {
        let json: Json = serde_json::from_str(r#"{"primary":{"window_minutes":300}}"#).unwrap();
        assert!(parse_rate_limits(&json, at(NOW)).is_empty());
        assert!(parse_rate_limits(&Json::Null, at(NOW)).is_empty());
    }

    fn write_rollout(dir: &Path, name: &str, lines: &[String]) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        path
    }

    fn ts(offset: i64) -> String {
        (at(NOW) + chrono::Duration::seconds(offset)).to_rfc3339()
    }

    #[test]
    fn reads_the_newest_rate_limits_from_a_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "rollout-1.jsonl",
            &[
                format!(
                    r#"{{"type":"token_count","timestamp":"{}","rate_limits":{{"primary":{{"used_percent":10,"window_minutes":300,"resets_in_seconds":600}}}}}}"#,
                    ts(-600)
                ),
                format!(
                    r#"{{"type":"token_count","timestamp":"{}","rate_limits":{{"primary":{{"used_percent":33,"window_minutes":300,"resets_in_seconds":300}}}},"info":{{"total_token_usage":{{"total_tokens":4242}},"model":"gpt-5-codex"}}}}"#,
                    ts(-30)
                ),
            ],
        );

        let s = summarise_rollout(&path, at(NOW)).unwrap();
        assert_eq!(s.rate_limits.len(), 1);
        assert_eq!(
            s.rate_limits[0].used_pct,
            Some(33.0),
            "the later snapshot must win"
        );
        assert_eq!(s.total_tokens, Some(4242));
        assert_eq!(s.model.as_deref(), Some("gpt-5-codex"));
    }

    #[test]
    fn reads_rate_limits_nested_under_payload() {
        // The shape current Codex CLI writes: every event wrapped in `payload`,
        // with `resets_at` as a Unix timestamp.
        let dir = tempfile::tempdir().unwrap();
        let reset = at(NOW).timestamp() + 3600;
        let path = write_rollout(
            dir.path(),
            "rollout-new.jsonl",
            &[format!(
                r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":28677}}}},"rate_limits":{{"limit_id":"codex","primary":{{"used_percent":12.0,"window_minutes":300,"resets_at":{reset}}},"secondary":{{"used_percent":40.0,"window_minutes":10080,"resets_at":{reset}}},"credits":{{"has_credits":false}}}}}}}}"#,
                ts(-30)
            )],
        );

        let s = summarise_rollout(&path, at(NOW)).unwrap();
        assert_eq!(s.rate_limits.len(), 2);
        assert_eq!(s.rate_limits[0].used_pct, Some(12.0));
        assert!(!s.rate_limits[0].weekly);
        assert_eq!(s.rate_limits[1].used_pct, Some(40.0));
        assert!(
            s.rate_limits[1].weekly,
            "the 7-day window is flagged weekly"
        );
        assert_eq!(s.total_tokens, Some(28677));
    }

    #[test]
    fn reads_where_the_session_was_started_from_its_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "rollout-desktop.jsonl",
            &[
                format!(
                    r#"{{"timestamp":"{}","type":"session_meta","payload":{{"id":"x","cwd":"C:\\dev\\api","originator":"Codex Desktop","source":"vscode"}}}}"#,
                    ts(-60)
                ),
                format!(
                    r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"total_tokens":5}}}}}}}}"#,
                    ts(-30)
                ),
            ],
        );
        let s = summarise_rollout(&path, at(NOW)).unwrap();
        assert_eq!(s.originator.as_deref(), Some("Codex Desktop"));
    }

    #[test]
    fn a_relative_reset_counts_from_the_line_not_the_poll() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "rollout-old.jsonl",
            &[format!(
                r#"{{"type":"token_count","timestamp":"{}","rate_limits":{{"primary":{{"used_percent":5,"window_minutes":300,"resets_in_seconds":600}}}}}}"#,
                ts(-300)
            )],
        );

        let s = summarise_rollout(&path, at(NOW)).unwrap();
        // Written 5 minutes ago with 10 minutes to go: 5 minutes left now.
        assert_eq!(
            s.rate_limits[0].resets_at,
            Some(at(NOW) + chrono::Duration::seconds(300))
        );
    }

    #[test]
    fn detects_a_pending_tool_call_as_waiting_for_approval() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "r.jsonl",
            &[format!(
                r#"{{"type":"function_call","timestamp":"{}","name":"shell"}}"#,
                ts(-40)
            )],
        );
        assert_eq!(
            summarise_rollout(&path, at(NOW)).unwrap().activity,
            Activity::AwaitingInput
        );
    }

    #[test]
    fn a_completed_tool_call_is_not_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "r.jsonl",
            &[
                format!(r#"{{"type":"function_call","timestamp":"{}"}}"#, ts(-60)),
                format!(
                    r#"{{"type":"function_call_output","timestamp":"{}"}}"#,
                    ts(-55)
                ),
            ],
        );
        assert_eq!(
            summarise_rollout(&path, at(NOW)).unwrap().activity,
            Activity::Done
        );
    }

    #[test]
    fn activity_reflects_recency() {
        let now = at(NOW);
        let ago = |secs: i64| Some(now - chrono::Duration::seconds(secs));

        assert_eq!(
            classify(false, None, None, ago(3), now),
            Activity::Generating
        );
        assert_eq!(classify(false, None, None, ago(120), now), Activity::Done);
        assert_eq!(
            classify(false, None, None, ago(86_400), now),
            Activity::Idle
        );
        // A tool call from yesterday isn't a live prompt.
        assert_eq!(
            classify(true, ago(86_400), None, ago(86_400), now),
            Activity::Idle
        );
        // One that started seconds ago is a command running, not a prompt.
        assert_eq!(
            classify(true, ago(2), None, ago(2), now),
            Activity::Generating
        );
        assert_eq!(
            classify(true, ago(30), None, ago(30), now),
            Activity::AwaitingInput
        );
    }

    #[test]
    fn the_turn_end_record_wins_over_silence() {
        let now = at(NOW);
        let ago = |secs: i64| Some(now - chrono::Duration::seconds(secs));

        // Codex said the turn finished a second ago: finished, not "working"
        // just because the file was written to a moment ago.
        assert_eq!(classify(false, None, ago(1), ago(1), now), Activity::Done);
        // A new turn clears it (`task_started` resets `turn_end` to None).
        assert_eq!(
            classify(false, None, None, ago(1), now),
            Activity::Generating
        );
    }

    #[test]
    fn discovers_the_default_and_named_profiles() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".codex/sessions")).unwrap();
        std::fs::create_dir_all(home.path().join(".codex-work/sessions")).unwrap();
        std::fs::create_dir_all(home.path().join(".codex-oss/sessions")).unwrap();
        // Not a profile.
        std::fs::create_dir_all(home.path().join(".codexfoo")).unwrap();

        let profiles = discover_profiles(home.path());
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[0].name, None, "default profile sorts first");
        let names: Vec<_> = profiles.iter().filter_map(|p| p.name.clone()).collect();
        assert_eq!(names, vec!["oss", "work"]);
    }

    #[test]
    fn a_missing_install_reports_unavailable() {
        let home = tempfile::tempdir().unwrap();
        let mut adapter = CodexAdapter::with_home(home.path().to_path_buf());
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Unavailable);
    }

    #[test]
    fn collects_limits_and_sessions_across_profiles() {
        let home = tempfile::tempdir().unwrap();
        let sessions = home.path().join(".codex/sessions/2026/01/01");
        write_rollout(
            &sessions,
            "rollout-a.jsonl",
            &[format!(
                r#"{{"type":"token_count","timestamp":"{}","cwd":"C:\\dev\\api","rate_limits":{{"primary":{{"used_percent":75,"window_minutes":300,"resets_in_seconds":1800}}}},"info":{{"total_token_usage":{{"total_tokens":100}}}}}}"#,
                ts(-10)
            )],
        );

        let mut adapter = CodexAdapter::with_home(home.path().to_path_buf());
        let snap = adapter.collect(at(NOW));

        assert_eq!(snap.health, Health::Ok);
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].used_pct, Some(75.0));
        assert!(!snap.windows[0].estimated, "API figures aren't estimates");
        assert_eq!(snap.activity, Activity::Generating);
        assert_eq!(snap.sessions[0].title, "api");
    }

    #[test]
    fn falls_back_to_a_token_estimate_when_no_snapshot_exists() {
        let home = tempfile::tempdir().unwrap();
        write_rollout(
            &home.path().join(".codex/sessions"),
            "r.jsonl",
            &[format!(
                r#"{{"type":"token_count","timestamp":"{}","info":{{"total_token_usage":{{"total_tokens":777}}}}}}"#,
                ts(-30)
            )],
        );

        let mut adapter = CodexAdapter::with_home(home.path().to_path_buf());
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].used, Some(777.0));
        assert!(snap.windows[0].estimated);
        assert_eq!(snap.windows[0].used_pct, None);
    }

    #[test]
    fn an_installed_but_idle_codex_is_marked_stale_not_wrong() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".codex/sessions")).unwrap();

        let mut adapter = CodexAdapter::with_home(home.path().to_path_buf());
        let snap = adapter.collect(at(NOW));
        assert_eq!(snap.health, Health::Stale);
        assert!(snap.windows.is_empty());
        assert!(snap.detail.is_some());
    }
}
