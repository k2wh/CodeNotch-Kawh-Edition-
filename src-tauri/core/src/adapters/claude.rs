//! Claude Code adapter.
//!
//! Primary source is Claude's OAuth usage endpoint, which reports the real
//! rolling-window utilisation. Credentials come from
//! `%USERPROFILE%\.claude\.credentials.json`, or from Windows Credential
//! Manager when the install opted into OS-backed storage.
//!
//! When there is no usable token (or the endpoint is rate limiting us) we fall
//! back to the local session transcripts under `%USERPROFILE%\.claude\projects\`.
//! Those give token counts and, importantly, the live activity state: whether an
//! agent is generating, finished, or parked on a `[y/N]` approval prompt.
//!
//! And when there is no Claude Code at all — someone who only has the Claude
//! desktop app — the app's own record of the plan stands in: it samples the
//! same two windows every quarter of an hour into `plan-usage-history.json`
//! beside its settings. Those are real readings, taken by the app for itself;
//! nothing of the app's session is read or used to ask for them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::{
    home_dir, newest_files, newest_mtime, parse_timestamp, project_label, tail_lines, Backoff,
};
use crate::model::{
    Activity, Health, ProviderId, ProviderSnapshot, Session, UsageUnit, UsageWindow,
};
use crate::secrets::{read_generic_credential, CLAUDE_CREDENTIAL_TARGETS};

/// Anthropic's OAuth usage endpoint.
const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// Beta header Claude Code sends with OAuth-authenticated calls.
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// How far back a transcript counts as "the current session window".
const SESSION_WINDOW_HOURS: i64 = 5;
/// Newer than this and we consider the agent to be actively generating.
const GENERATING_WITHIN_SECS: i64 = 25;
/// Newer than this and a finished session is still worth surfacing.
const DONE_WITHIN_MINS: i64 = 10;
/// Only the newest transcripts matter; scanning every project is wasteful.
const MAX_TRANSCRIPTS: usize = 12;

/// The desktop app writes a sample every quarter of an hour while it runs, so
/// anything younger than this is as current as that file ever gets.
const APP_SAMPLE_FRESH_MINS: i64 = 30;
/// Past this the five-hour window may have rolled over without the app running
/// to see it, and a number that might be from the window before is worse than
/// none: only the week, which is still a floor, carries on.
const APP_SAMPLE_SESSION_MAX_MINS: i64 = 2 * 60;
/// Past this, even the week says too little about today to show.
const APP_SAMPLE_MAX_MINS: i64 = 12 * 60;
/// Tail window per transcript. Enough for the last few exchanges.
const TRANSCRIPT_TAIL_BYTES: u64 = 256 * 1024;

/// OAuth credentials as Claude Code stores them.
#[derive(Debug, Clone, PartialEq)]
pub struct Credentials {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub subscription: Option<String>,
}

impl Credentials {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|e| now >= e)
    }
}

/// Parse `.credentials.json` (or the Credential Manager blob, which is the same
/// JSON).
///
/// Tolerant of both the nested `claudeAiOauth` envelope and a flat object,
/// because the shape has moved between releases.
pub fn parse_credentials(raw: &str) -> Option<Credentials> {
    let root: Json = serde_json::from_str(raw).ok()?;
    let obj = root
        .get("claudeAiOauth")
        .or_else(|| root.get("oauth"))
        .unwrap_or(&root);

    let access_token = obj
        .get("accessToken")
        .or_else(|| obj.get("access_token"))
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())?
        .to_string();

    Some(Credentials {
        access_token,
        expires_at: obj
            .get("expiresAt")
            .or_else(|| obj.get("expires_at"))
            .and_then(parse_timestamp),
        subscription: obj
            .get("subscriptionType")
            .or_else(|| obj.get("subscription_type"))
            .and_then(Json::as_str)
            .map(str::to_string),
    })
}

/// Turn a `subscriptionType` into something worth putting on screen.
fn plan_label(subscription: Option<&str>) -> Option<String> {
    let s = subscription?;
    Some(match s.to_ascii_lowercase().as_str() {
        "max" => "Max".to_string(),
        "pro" => "Pro".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "free" => "Free".to_string(),
        other => {
            let mut c = other.chars();
            let first = c.next()?;
            first.to_uppercase().collect::<String>() + c.as_str()
        }
    })
}

/// Friendly labels for the windows the endpoint is known to return. Anything
/// unrecognised still renders, using a prettified version of its key.
fn window_label(key: &str) -> String {
    match key {
        "five_hour" => "5h session".to_string(),
        "seven_day" => "7d all models".to_string(),
        "seven_day_opus" => "7d Opus".to_string(),
        "seven_day_sonnet" => "7d Sonnet".to_string(),
        "seven_day_overage_included" => "7d + overage".to_string(),
        "seven_day_cowork" => "7d cowork".to_string(),
        "seven_day_oauth_apps" => "7d apps".to_string(),
        other => other
            .split(['_', '-'])
            .filter(|s| !s.is_empty())
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

/// Parse a `limits` array, when a reply carries one.
///
/// Each entry says what it is rather than what it is called internally:
/// `session`, `weekly_all`, and `weekly_scoped` with the model's own display
/// name beside it. Anything else is left alone — a kind this build has never
/// heard of is better skipped than guessed at.
pub fn parse_limits(raw: &str, now: DateTime<Utc>) -> Vec<UsageWindow> {
    let Ok(root) = serde_json::from_str::<Json>(raw) else {
        return Vec::new();
    };
    // The same payload turns up bare and wrapped, depending on the caller.
    let limits = root
        .get("limits")
        .or_else(|| root.get("utilization").and_then(|u| u.get("limits")))
        .and_then(Json::as_array);

    let mut windows = Vec::new();
    for entry in limits.into_iter().flatten() {
        let Some(percent) = entry.get("percent").and_then(Json::as_f64) else {
            continue;
        };
        let model = entry
            .get("scope")
            .and_then(|scope| scope.get("model"))
            .and_then(|model| model.get("display_name"))
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());

        let (key, label, minutes, weekly) = match entry.get("kind").and_then(Json::as_str) {
            Some("session") => (
                "five_hour".to_string(),
                "5h session".to_string(),
                5 * 60,
                false,
            ),
            Some("weekly_all") => (
                "seven_day".to_string(),
                "7d all models".to_string(),
                7 * 24 * 60,
                true,
            ),
            Some("weekly_scoped") => {
                let model = model.unwrap_or("scoped");
                (
                    format!("seven_day_{}", model.to_ascii_lowercase().replace(' ', "_")),
                    format!("7d {model}"),
                    7 * 24 * 60,
                    true,
                )
            }
            _ => continue,
        };

        let mut window = UsageWindow::new(key, label)
            .with_pct(percent.clamp(0.0, 100.0) as f32)
            .lasting(minutes)
            .weekly(weekly);
        window.resets_at = entry.get("resets_at").and_then(parse_timestamp);
        windows.push(window);
    }

    // A reset already behind us is a window that rolled over between the
    // reading and now; the percentage is what matters, not the stale date.
    for window in &mut windows {
        if window.resets_at.is_some_and(|at| at <= now) {
            window.resets_at = None;
        }
    }
    windows.sort_by_key(|w| window_rank(&w.key));
    windows
}

/// When a transcript line was written.
pub fn line_stamp(line: &str) -> Option<DateTime<Utc>> {
    let json: Json = serde_json::from_str(line).ok()?;
    json.get("timestamp").and_then(parse_timestamp)
}

/// What a transcript line cost.
///
/// Only assistant turns carry a usage block, which is the whole cost of the
/// exchange that produced them.
pub fn line_usage(line: &str) -> Option<crate::ledger::Usage> {
    let json: Json = serde_json::from_str(line).ok()?;
    let usage = json.get("message")?.get("usage")?;
    let count = |field: &str| usage.get(field).and_then(Json::as_u64).unwrap_or(0);

    // Cache writes come in two lifetimes at different prices. Newer
    // transcripts break them out; older ones report only the total, which is
    // then taken as the cheaper five-minute kind rather than guessed upward.
    let detail = usage.get("cache_creation").filter(|d| d.is_object());
    let (cache_write, cache_write_1h) = match detail {
        Some(detail) => {
            let of = |field: &str| detail.get(field).and_then(Json::as_u64).unwrap_or(0);
            (
                of("ephemeral_5m_input_tokens"),
                of("ephemeral_1h_input_tokens"),
            )
        }
        None => (count("cache_creation_input_tokens"), 0),
    };

    let spent = crate::ledger::Usage {
        input: count("input_tokens"),
        output: count("output_tokens"),
        cache_write,
        cache_write_1h,
        cache_read: count("cache_read_input_tokens"),
        // One line per content block, each repeating the whole usage block:
        // this is what tells them apart from separate turns.
        id: json
            .get("message")
            .and_then(|m| m.get("id"))
            .and_then(Json::as_str)
            .map(str::to_string),
    };
    (spent.total() > 0).then_some(spent)
}

/// The model a transcript line says produced it, for pricing what it spent.
pub fn line_model(line: &str) -> Option<String> {
    let json: Json = serde_json::from_str(line).ok()?;
    let model = json.get("message")?.get("model")?.as_str()?;
    (!model.is_empty()).then(|| model.to_string())
}

/// Parse the usage endpoint's body into windows.
///
/// The endpoint is undocumented and its shape has changed more than once, so
/// this walks whatever object it gets and picks up any entry that looks like a
/// window, rather than binding to one exact schema. An unparseable body yields
/// an empty list, which the caller surfaces as an error instead of a number.
pub fn parse_usage(raw: &str) -> Vec<UsageWindow> {
    let Ok(root) = serde_json::from_str::<Json>(raw) else {
        return Vec::new();
    };

    // Accept `{...}`, `{"usage": {...}}` and `{"windows": [...]}`.
    let mut candidates: Vec<(String, &Json)> = Vec::new();
    let container = root.get("usage").unwrap_or(&root);

    if let Some(list) = container.get("windows").and_then(Json::as_array) {
        for (i, item) in list.iter().enumerate() {
            let key = item
                .get("key")
                .or_else(|| item.get("name"))
                .and_then(Json::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("window_{i}"));
            candidates.push((key, item));
        }
    } else if let Some(map) = container.as_object() {
        for (key, value) in map {
            if value.is_object() {
                candidates.push((key.clone(), value));
            }
        }
    }

    let mut windows: Vec<UsageWindow> = candidates
        .into_iter()
        .filter_map(|(key, value)| parse_window(&key, value))
        .collect();

    // Stable, meaningful order: shortest window first is what a user scans for.
    windows.sort_by_key(|w| window_rank(&w.key));
    windows
}

fn window_rank(key: &str) -> u8 {
    match key {
        "five_hour" => 0,
        "seven_day" => 1,
        "seven_day_opus" => 2,
        "seven_day_sonnet" => 3,
        "seven_day_overage_included" => 4,
        "seven_day_oauth_apps" => 5,
        _ => 6,
    }
}

fn parse_window(key: &str, value: &Json) -> Option<UsageWindow> {
    // Every weekly window the endpoint returns is keyed `seven_day*`.
    let weekly = key.starts_with("seven_day");
    let mut window = UsageWindow::new(key, window_label(key)).weekly(weekly);
    // The endpoint doesn't say how long a window runs; its key does.
    if weekly {
        window = window.lasting(7 * 24 * 60);
    } else if key.starts_with("five_hour") {
        window = window.lasting(5 * 60);
    }

    let pct = value
        .get("utilization")
        .or_else(|| value.get("used_percent"))
        .or_else(|| value.get("percent"))
        .or_else(|| value.get("percentage"))
        .and_then(Json::as_f64);

    let used = value
        .get("used")
        .or_else(|| value.get("used_tokens"))
        .and_then(Json::as_f64);
    let limit = value
        .get("limit")
        .or_else(|| value.get("total"))
        .and_then(Json::as_f64);

    match (pct, used) {
        (Some(p), _) => {
            window = window.with_pct(p as f32);
            // Keep raw counts alongside a reported percentage when both exist.
            if let Some(u) = used {
                window.used = Some(u);
                window.limit = limit;
            }
        }
        (None, Some(u)) => {
            window = window.with_counts(u, limit).with_unit(UsageUnit::Tokens);
        }
        // Neither a percentage nor a count: nothing to show, so skip it rather
        // than rendering an empty ring.
        (None, None) => return None,
    }

    let resets = value
        .get("resets_at")
        .or_else(|| value.get("reset_at"))
        .or_else(|| value.get("resetsAt"))
        .and_then(parse_timestamp);

    Some(window.with_reset(resets))
}

/// One line of a Claude Code transcript, reduced to what we care about.
#[derive(Debug, Default, Clone)]
struct TranscriptEntry {
    timestamp: Option<DateTime<Utc>>,
    cwd: Option<String>,
    /// Which Claude Code front end wrote the line (`claude-desktop`, ...).
    entrypoint: Option<String>,
    model: Option<String>,
    tokens: u64,
    /// `user` / `assistant`: a line that moves the conversation, as opposed to
    /// the bookkeeping records interleaved with them.
    message: bool,
    /// Claude Code's end-of-turn record (`system` / `turn_duration`): the turn
    /// is over, said outright rather than inferred from silence.
    turn_end: bool,
    /// Tools this line asked to run, as (id, name).
    tool_uses: Vec<(String, String)>,
    /// Tool results this line carried back, by tool id.
    tool_results: Vec<String>,
    /// An assistant turn that only said something, with no tool call. These
    /// get no end-of-turn record, so they need the silence rule.
    text_only: bool,
}

fn parse_entry(line: &str) -> Option<TranscriptEntry> {
    let json: Json = serde_json::from_str(line).ok()?;
    let mut entry = TranscriptEntry {
        timestamp: json.get("timestamp").and_then(parse_timestamp),
        cwd: json.get("cwd").and_then(Json::as_str).map(str::to_string),
        entrypoint: json
            .get("entrypoint")
            .and_then(Json::as_str)
            .map(str::to_string),
        ..Default::default()
    };

    let kind = json.get("type").and_then(Json::as_str);
    entry.message = matches!(kind, Some("user") | Some("assistant"));
    entry.turn_end = kind == Some("system")
        && json.get("subtype").and_then(Json::as_str) == Some("turn_duration");

    if let Some(message) = json.get("message") {
        entry.model = message
            .get("model")
            .and_then(Json::as_str)
            .map(str::to_string);

        if let Some(usage) = message.get("usage") {
            // Cache reads are charged differently but still count towards the
            // window, so include every bucket the transcript reports.
            for field in [
                "input_tokens",
                "output_tokens",
                "cache_creation_input_tokens",
                "cache_read_input_tokens",
            ] {
                entry.tokens += usage.get(field).and_then(Json::as_u64).unwrap_or(0);
            }
        }

        if let Some(content) = message.get("content").and_then(Json::as_array) {
            let mut text = false;
            for block in content {
                match block.get("type").and_then(Json::as_str) {
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string();
                        entry.tool_uses.push((id, name));
                    }
                    Some("tool_result") => entry.tool_results.push(
                        block
                            .get("tool_use_id")
                            .and_then(Json::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    ),
                    Some("text") => text = true,
                    _ => {}
                }
            }
            entry.text_only = kind == Some("assistant") && text && entry.tool_uses.is_empty();
        }
    }

    Some(entry)
}

/// What a single transcript tells us.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSummary {
    pub session_id: String,
    pub project: String,
    pub cwd: Option<String>,
    /// The front end the session runs in (see `Session::host`).
    pub entrypoint: Option<String>,
    pub model: Option<String>,
    pub tokens: u64,
    pub last_activity: Option<DateTime<Utc>>,
    pub activity: Activity,
}

/// Read one transcript's tail and classify what the session is doing.
///
/// The approval heuristic is the interesting part: when the last thing in the
/// transcript is an assistant turn requesting a tool, and no tool result has
/// followed it, Claude Code is sitting at a permission prompt waiting for the
/// user. That is the state worth interrupting someone for.
pub fn summarise_transcript(path: &Path, now: DateTime<Utc>) -> Option<TranscriptSummary> {
    let lines = tail_lines(path, TRANSCRIPT_TAIL_BYTES).ok()?;
    let entries: Vec<TranscriptEntry> = lines.iter().filter_map(|l| parse_entry(l)).collect();
    if entries.is_empty() {
        return None;
    }

    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into());

    let cwd = entries.iter().rev().find_map(|e| e.cwd.clone());
    let project = cwd
        .as_deref()
        .map(project_label)
        // Fall back to the encoded directory name Claude Code derives from cwd.
        .or_else(|| {
            path.parent()
                .and_then(|p| p.file_name())
                .map(|s| decode_project_dir(&s.to_string_lossy()))
        })
        .unwrap_or_else(|| "session".into());

    let window_start = now - chrono::Duration::hours(SESSION_WINDOW_HOURS);
    let tokens = entries
        .iter()
        .filter(|e| e.timestamp.is_none_or(|t| t >= window_start))
        .map(|e| e.tokens)
        .sum();

    let last_activity = entries.iter().rev().find_map(|e| e.timestamp);
    let model = entries.iter().rev().find_map(|e| e.model.clone());
    let entrypoint = entries.iter().rev().find_map(|e| e.entrypoint.clone());

    let activity = classify(&entries, last_activity, now);

    Some(TranscriptSummary {
        session_id,
        project,
        cwd,
        entrypoint,
        model,
        tokens,
        last_activity,
        activity,
    })
}

/// Tools that routinely take a while without anyone being asked anything:
/// treating a slow one as a permission prompt would cry wolf.
const EXEMPT_TOOLS: &[&str] = &["Task", "Agent", "AskUserQuestion"];
/// How long an unanswered tool call runs before it reads as a prompt.
const PERMISSION_AFTER_SECS: i64 = 7;
/// Silence after a talk-only turn before it counts as finished. Those turns
/// get no end-of-turn record, so this is the one case that needs a timer.
const TEXT_IDLE_SECS: i64 = 5;

/// What the transcript says the session is doing.
///
/// The order matters, and mirrors what the signals actually mean:
///
/// 1. **The turn ended**, because Claude Code said so (`turn_duration`) and
///    nothing has been said since. This is the reliable one — the previous
///    version inferred "finished" from a quiet file, which called a long tool
///    call finished and missed short answers entirely.
/// 2. **Waiting on you**: a tool call with no result, old enough that it isn't
///    just slow, and not one of the tools that are always slow.
/// 3. **Working**: something was written moments ago.
/// 4. **Finished / idle**: by age, including the silence rule for talk-only
///    turns, which produce no end-of-turn record.
fn classify(
    entries: &[TranscriptEntry],
    last_activity: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Activity {
    let Some(last_activity) = last_activity else {
        return Activity::Idle;
    };
    let age = now.signed_duration_since(last_activity);
    // A timestamp in the future means a clock skew we shouldn't act on.
    if age < chrono::Duration::zero() {
        return Activity::Generating;
    }

    let since = |at: DateTime<Utc>| now.signed_duration_since(at);
    let settled = |at: DateTime<Utc>| {
        if since(at) < chrono::Duration::minutes(DONE_WITHIN_MINS) {
            Activity::Done
        } else {
            Activity::Idle
        }
    };

    let last_message = entries
        .iter()
        .rev()
        .find(|e| e.message)
        .and_then(|e| e.timestamp);
    let turn_end = entries
        .iter()
        .rev()
        .find(|e| e.turn_end)
        .and_then(|e| e.timestamp);

    // 1. The turn ended and nothing has happened since.
    if let Some(end) = turn_end {
        if last_message.is_none_or(|msg| msg <= end) {
            return settled(end);
        }
    }

    // 2. A tool call still waiting on an answer: every call, minus the ones
    //    whose result came back, matched by tool id.
    let mut pending: Vec<(&str, &str, DateTime<Utc>)> = Vec::new();
    for entry in entries {
        let at = entry.timestamp.unwrap_or(last_activity);
        for (id, name) in &entry.tool_uses {
            pending.push((id.as_str(), name.as_str(), at));
        }
        for done in &entry.tool_results {
            // A result normally names the call it answers; when it doesn't,
            // it answers the oldest call still open.
            if done.is_empty() {
                if !pending.is_empty() {
                    pending.remove(0);
                }
            } else {
                pending.retain(|(id, _, _)| id != done);
            }
        }
    }
    if let Some((_, _, at)) = pending
        .iter()
        .find(|(_, name, _)| !EXEMPT_TOOLS.contains(name))
    {
        let waited = since(*at);
        if waited >= chrono::Duration::seconds(PERMISSION_AFTER_SECS)
            && waited < chrono::Duration::hours(1)
        {
            return Activity::AwaitingInput;
        }
    }

    // 3. Still writing.
    if age < chrono::Duration::seconds(GENERATING_WITHIN_SECS) {
        // A talk-only turn is over as soon as it goes quiet for a moment.
        let text_only = entries
            .iter()
            .rev()
            .find(|e| e.message)
            .is_some_and(|e| e.text_only);
        if !(text_only && age >= chrono::Duration::seconds(TEXT_IDLE_SECS)) {
            return Activity::Generating;
        }
    }

    // 4. Finished a moment ago, or long enough ago to be idle.
    settled(last_activity)
}

/// Claude Code names project folders after the cwd with separators replaced by
/// dashes (`C:\Users\dev\app` -> `C--Users-dev-app`). We can't reverse that
/// unambiguously, so just take the trailing segment.
fn decode_project_dir(name: &str) -> String {
    name.rsplit('-')
        .find(|s| !s.is_empty())
        .unwrap_or(name)
        .to_string()
}

/// Filesystem locations the adapter reads. Injectable so tests don't need a
/// real `%USERPROFILE%`.
#[derive(Debug, Clone)]
pub struct ClaudePaths {
    pub root: PathBuf,
}

impl ClaudePaths {
    pub fn detect() -> Option<Self> {
        home_dir().map(|h| Self {
            root: h.join(".claude"),
        })
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn credentials_file(&self) -> PathBuf {
        self.root.join(".credentials.json")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }

    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    /// Claude Code's own settings file, which sits beside the directory
    /// rather than inside it: `~/.claude` has `~/.claude.json`.
    pub fn settings_file(&self) -> PathBuf {
        self.root.with_extension("json")
    }
}

/// What the Claude desktop app has seen of the plan.
///
/// The app keeps its own history of the two windows — `fh` for the five-hour
/// one, `sd` for the week — sampled every quarter of an hour while it is
/// running. For someone who has the app and not Claude Code, this is the only
/// reading of their limits on the machine, and it is the app's own: no session
/// of theirs is borrowed to produce it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppReading {
    pub taken_at: DateTime<Utc>,
    pub five_hour_pct: Option<f32>,
    pub seven_day_pct: Option<f32>,
}

/// Where the desktop app keeps that file: `%APPDATA%\Claude` on Windows,
/// `~/Library/Application Support/Claude` on a Mac.
pub fn app_usage_file() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("Claude").join("plan-usage-history.json"))
}

/// The newest sample in the app's history, whatever its age.
pub fn parse_app_usage(raw: &str) -> Option<AppReading> {
    let root: Json = serde_json::from_str(raw).ok()?;
    let samples = root.get("samples")?.as_array()?;
    let newest = samples
        .iter()
        .filter(|sample| sample.get("t").and_then(Json::as_i64).is_some())
        .max_by_key(|sample| sample.get("t").and_then(Json::as_i64).unwrap_or(0))?;

    let taken_at = DateTime::from_timestamp_millis(newest.get("t")?.as_i64()?)?;
    let pct = |field: &str| {
        newest
            .get("u")
            .and_then(|usage| usage.get(field))
            .and_then(Json::as_f64)
            .map(|value| value.clamp(0.0, 100.0) as f32)
    };
    Some(AppReading {
        taken_at,
        five_hour_pct: pct("fh"),
        seven_day_pct: pct("sd"),
    })
}

/// The app's reading as windows, dropping what has gone too stale to mean
/// anything. `None` when nothing is left worth drawing.
fn app_windows(reading: AppReading, now: DateTime<Utc>) -> Option<(Vec<UsageWindow>, i64)> {
    let age = now.signed_duration_since(reading.taken_at).num_minutes();
    // A sample from the future is a clock that disagrees, not a reading; a
    // little ahead is fine, hours ahead is not.
    if !(-APP_SAMPLE_FRESH_MINS..=APP_SAMPLE_MAX_MINS).contains(&age) {
        return None;
    }

    let mut windows = Vec::new();
    if age <= APP_SAMPLE_SESSION_MAX_MINS {
        if let Some(pct) = reading.five_hour_pct {
            windows.push(
                UsageWindow::new("five_hour", "5h session")
                    .with_pct(pct)
                    .lasting(5 * 60)
                    .weekly(false),
            );
        }
    }
    if let Some(pct) = reading.seven_day_pct {
        windows.push(
            UsageWindow::new("seven_day", "7d all models")
                .with_pct(pct)
                .lasting(7 * 24 * 60)
                .weekly(true),
        );
    }
    (!windows.is_empty()).then_some((windows, age.max(0)))
}

/// Every Claude Code account signed in on this machine.
///
/// Claude Code keeps one account per config directory, and switching between
/// them is a matter of pointing `CLAUDE_CONFIG_DIR` somewhere else — so the
/// directories beside the default one are the other accounts. Returns
/// `(account name, paths)`, the default account first with no name, the rest
/// in a stable order.
///
/// An empty result means Claude Code isn't installed, which the caller shows
/// as one absent provider rather than none at all.
pub fn accounts() -> Vec<(Option<String>, ClaudePaths)> {
    accounts_in(home_dir(), std::env::var("CLAUDE_CONFIG_DIR").ok())
}

/// The testable half: where to look, and what the environment says.
pub fn accounts_in(
    home: Option<PathBuf>,
    configured: Option<String>,
) -> Vec<(Option<String>, ClaudePaths)> {
    let mut found: Vec<(Option<String>, ClaudePaths)> = Vec::new();

    // What the environment points at is the default account, whatever it is
    // called: it is the one the agents in this session are using.
    if let Some(dir) = configured
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        found.push((None, ClaudePaths::at(PathBuf::from(dir))));
    }

    let Some(home) = home else { return found };
    let default = home.join(".claude");
    if found.is_empty() && default.is_dir() {
        found.push((None, ClaudePaths::at(default.clone())));
    }

    // `~/.claude-work` is the `work` account. Sorted, so the rings don't
    // reshuffle between polls.
    let mut extra: Vec<(String, PathBuf)> = std::fs::read_dir(&home)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            let account = name.strip_prefix(".claude-")?;
            // `.claude-mem` and friends are plugin data, not accounts. What
            // makes a directory an account is that Claude Code signed in or
            // worked there, so that is what is checked rather than the name.
            let signed_in = path.join(".credentials.json").is_file();
            let worked_in = path.join("projects").is_dir();
            (!account.is_empty() && (signed_in || worked_in))
                .then(|| (account.to_string(), path.clone()))
        })
        .collect();
    extra.sort();

    for (account, path) in extra {
        found.push((Some(account), ClaudePaths::at(path)));
    }
    found
}

/// Collects Claude Code usage.
pub struct ClaudeAdapter {
    http: reqwest::Client,
    backoff: Backoff,
    paths: Option<ClaudePaths>,
    /// Last good reading, reused while we're backing off so the HUD keeps
    /// showing real numbers instead of blanking.
    last_good: Option<(DateTime<Utc>, Vec<UsageWindow>, Option<String>)>,
    /// An access token the endpoint answered 401/403 to. It won't start
    /// working by itself, so it isn't sent again until Claude Code writes a
    /// new one.
    rejected_token: Option<String>,
    /// Why the backoff is running, so a network outage isn't reported as a
    /// rate limit.
    backoff_health: Health,
    /// Whether Credential Manager may stand in for a missing credentials file.
    /// The entry there is the default account's, so an extra account never
    /// reads it: it would show the default account's usage under its own name,
    /// and ask the endpoint for it twice.
    shared_store: bool,
    /// The desktop app's own record of the plan, which stands in when there is
    /// no Claude Code login here. It belongs to whichever account the app is
    /// signed into, so like Credential Manager it is the default account's.
    app_usage: Option<PathBuf>,
}

impl ClaudeAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            backoff: Backoff::new(Duration::from_secs(60), Duration::from_secs(15 * 60)),
            paths: ClaudePaths::detect(),
            last_good: None,
            rejected_token: None,
            backoff_health: Health::RateLimited,
            shared_store: true,
            app_usage: app_usage_file(),
        }
    }

    /// Sign in with this account's own credentials file or not at all (see
    /// `shared_store`), as every account but the default one does.
    pub fn own_credentials_only(mut self) -> Self {
        self.shared_store = false;
        self
    }

    /// The rate-limit penalty this adapter is serving, for writing out.
    pub fn backoff_state(&self) -> crate::adapters::StoredBackoff {
        self.backoff.stored()
    }

    /// Resume a penalty a previous run was serving.
    pub fn restore_backoff(&mut self, stored: crate::adapters::StoredBackoff, now: DateTime<Utc>) {
        self.backoff.restore(stored, now);
    }

    /// An adapter reading one account's directory, rather than the default.
    pub fn with_paths(http: reqwest::Client, paths: ClaudePaths) -> Self {
        Self {
            http,
            backoff: Backoff::new(Duration::from_secs(60), Duration::from_secs(15 * 60)),
            paths: Some(paths),
            last_good: None,
            rejected_token: None,
            backoff_health: Health::RateLimited,
            shared_store: true,
            app_usage: app_usage_file(),
        }
    }

    /// Read the desktop app's history from here rather than from where it
    /// lives, for tests.
    pub fn with_app_usage(mut self, path: Option<PathBuf>) -> Self {
        self.app_usage = path;
        self
    }

    /// Find credentials: the file first, then Credential Manager.
    pub fn credentials(&self) -> Option<Credentials> {
        if let Some(paths) = &self.paths {
            if let Ok(raw) = std::fs::read_to_string(paths.credentials_file()) {
                if let Some(creds) = parse_credentials(&raw) {
                    return Some(creds);
                }
            }
        }
        if !self.shared_store {
            return None;
        }
        CLAUDE_CREDENTIAL_TARGETS
            .iter()
            .filter_map(|target| read_generic_credential(target))
            .find_map(|raw| parse_credentials(&raw))
    }

    /// Fetch usage, honouring the back-off schedule.
    ///
    /// Returns `Ok(None)` when we're deliberately holding off.
    async fn fetch_usage(
        &mut self,
        token: &str,
        now: DateTime<Utc>,
    ) -> anyhow::Result<Option<Vec<UsageWindow>>> {
        if !self.backoff.ready_at(now) {
            return Ok(None);
        }

        // The OAuth endpoint, and only it. The organisation's own endpoint
        // carries a `limits` array naming each window's model — which is
        // where the per-model weekly row comes from — but it answers 403 to
        // this token: it wants the desktop app's session, not a bearer. Asked
        // anyway, the refusal looks exactly like a dead token and parks the
        // provider on "sign in again".
        let response = self
            .http
            .get(USAGE_URL)
            .bearer_auth(token)
            .header("anthropic-beta", OAUTH_BETA)
            .header("accept", "application/json")
            .timeout(Duration::from_secs(10))
            .send()
            .await;

        let response = match response {
            Ok(r) => r,
            Err(err) => {
                self.backoff_health = Health::Error;
                self.backoff
                    .record_failure(now, None, rand::random::<f64>());
                return Err(err.into());
            }
        };

        let status = response.status();

        if status.as_u16() == 429 || status.is_server_error() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            self.backoff_health = if status.as_u16() == 429 {
                Health::RateLimited
            } else {
                Health::Error
            };
            self.backoff
                .record_failure(now, retry_after, rand::random::<f64>());
            anyhow::bail!("usage endpoint returned {status}");
        }

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            // Not a rate-limit problem, and re-sending the same token won't
            // help: park it until Claude Code refreshes the credentials.
            self.rejected_token = Some(token.to_string());
            anyhow::bail!("unauthorized ({status})");
        }

        if !status.is_success() {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            anyhow::bail!("usage endpoint returned {status}");
        }

        let body = response.text().await?;
        self.backoff.record_success();

        // The named limits when the endpoint gives them, the flat map when
        // it doesn't.
        let windows = match parse_limits(&body, now) {
            named if !named.is_empty() => named,
            _ => parse_usage(&body),
        };
        if windows.is_empty() {
            anyhow::bail!("usage response had no recognisable windows");
        }
        Ok(Some(windows))
    }

    /// Scan local transcripts for sessions and their live state.
    /// The transcripts worth following for token spending.
    pub fn transcripts(&self) -> Vec<PathBuf> {
        self.paths
            .as_ref()
            .map(|paths| newest_files(&paths.projects_dir(), "jsonl", MAX_TRANSCRIPTS))
            .unwrap_or_default()
    }

    /// When a transcript was last written, for deciding whether a re-scan
    /// would tell us anything new.
    pub fn activity_mark(&self) -> Option<std::time::SystemTime> {
        let paths = self.paths.as_ref()?;
        newest_mtime(&paths.projects_dir(), "jsonl")
    }

    pub fn scan_sessions(&self, now: DateTime<Utc>) -> Vec<Session> {
        let Some(paths) = &self.paths else {
            return Vec::new();
        };
        let cutoff = now - chrono::Duration::hours(SESSION_WINDOW_HOURS);

        let mut sessions: Vec<Session> =
            newest_files(&paths.projects_dir(), "jsonl", MAX_TRANSCRIPTS)
                .into_iter()
                .filter_map(|p| summarise_transcript(&p, now))
                .filter(|s| s.last_activity.is_none_or(|t| t >= cutoff))
                .map(|s| Session {
                    id: s.session_id,
                    title: s.project,
                    cwd: s.cwd,
                    model: s.model,
                    activity: s.activity,
                    last_activity: s.last_activity,
                    tokens: Some(s.tokens).filter(|t| *t > 0),
                    detail: None,
                    host: s.entrypoint,
                })
                .collect();

        // Most interesting first: blocked, then generating, then most recent.
        sessions.sort_by(|a, b| {
            b.activity
                .rank()
                .cmp(&a.activity.rank())
                .then(b.last_activity.cmp(&a.last_activity))
        });
        sessions
    }

    /// Estimate the session window from transcript tokens when the API is
    /// unavailable. Always flagged `estimated` so the UI can mark it.
    fn transcript_windows(sessions: &[Session]) -> Vec<UsageWindow> {
        let tokens: u64 = sessions.iter().filter_map(|s| s.tokens).sum();
        if tokens == 0 {
            return Vec::new();
        }
        vec![UsageWindow::new("five_hour_tokens", "5h tokens")
            .with_counts(tokens as f64, None)
            .with_unit(UsageUnit::Tokens)
            .estimated()]
    }

    /// What the desktop app last saw of the plan, if it is worth showing.
    ///
    /// Only for the account the app is signed into, which is the default one —
    /// the same reason an extra account never reads Credential Manager.
    fn app_reading(&self, now: DateTime<Utc>) -> Option<(Vec<UsageWindow>, i64)> {
        if !self.shared_store {
            return None;
        }
        let raw = std::fs::read_to_string(self.app_usage.as_ref()?).ok()?;
        app_windows(parse_app_usage(&raw)?, now)
    }

    /// Build the snapshot for this poll.
    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let installed = self.paths.as_ref().is_some_and(|p| p.exists());
        let sessions = self.scan_sessions(now);

        let Some(creds) = self.credentials() else {
            // Nothing signed in here, but the desktop app may be: it keeps its
            // own reading of the same two windows, which is exactly what
            // someone who has the app and not Claude Code is asking to see.
            if let Some((windows, age)) = self.app_reading(now) {
                let fresh = age <= APP_SAMPLE_FRESH_MINS;
                let mut snap = ProviderSnapshot::new(ProviderId::ClaudeCode)
                    .with_source("claude app")
                    .with_windows(windows)
                    .with_sessions(sessions);
                snap.health = if fresh { Health::Ok } else { Health::Stale };
                snap.detail = Some(if fresh {
                    "Read by the Claude app, which watches the same limits".into()
                } else {
                    "The Claude app isn't running; these are the last figures it took".into()
                });
                return snap;
            }

            if !installed && sessions.is_empty() {
                return ProviderSnapshot::degraded(
                    ProviderId::ClaudeCode,
                    Health::Unavailable,
                    "Claude Code not found",
                );
            }
            // No token, but we can still report local activity and tokens.
            let windows = Self::transcript_windows(&sessions);
            let mut snap = ProviderSnapshot::new(ProviderId::ClaudeCode)
                .with_source("transcripts")
                .with_windows(windows)
                .with_sessions(sessions);
            snap.health = Health::NeedsAuth;
            snap.detail = Some("Sign in with `claude` to show usage limits".into());
            return snap;
        };

        // Claude Code only refreshes its access token while it runs, so an
        // expired one just means it hasn't been used lately. Keep the last
        // real figures on screen rather than dropping to a red "sign in".
        if creds.is_expired(now) {
            return self.degraded_with_last_good(
                Health::NeedsAuth,
                "Token expired; run `claude` to refresh",
                sessions,
                now,
            );
        }

        if self.rejected_token.as_deref() == Some(creds.access_token.as_str()) {
            return self.degraded_with_last_good(
                Health::NeedsAuth,
                "Token rejected; run `claude` to sign in again",
                sessions,
                now,
            );
        }

        let account = plan_label(creds.subscription.as_deref());

        match self.fetch_usage(&creds.access_token, now).await {
            Ok(Some(windows)) => {
                self.rejected_token = None;
                self.backoff_health = Health::RateLimited;
                self.last_good = Some((now, windows.clone(), account.clone()));
                ProviderSnapshot::new(ProviderId::ClaudeCode)
                    .with_source("oauth")
                    .with_account(account)
                    .with_windows(windows)
                    .with_sessions(sessions)
            }
            // Backing off: keep showing the last good numbers, flagged.
            Ok(None) => {
                let (health, detail) = if self.backoff_health == Health::RateLimited {
                    (Health::RateLimited, "Rate limited; retrying shortly")
                } else {
                    (Health::Error, "Can't reach Claude; retrying shortly")
                };
                self.degraded_with_last_good(health, detail, sessions, now)
            }
            Err(err) => {
                tracing::debug!(%err, "claude usage fetch failed");
                let health = if self.rejected_token.is_some() {
                    Health::NeedsAuth
                } else if self.backoff.is_backing_off() {
                    self.backoff_health
                } else {
                    Health::Error
                };
                self.degraded_with_last_good(health, err.to_string(), sessions, now)
            }
        }
    }

    /// Reuse the last good reading rather than blanking the HUD, marking the
    /// snapshot so the user can see the numbers aren't live.
    fn degraded_with_last_good(
        &self,
        health: Health,
        detail: impl Into<String>,
        sessions: Vec<Session>,
        now: DateTime<Utc>,
    ) -> ProviderSnapshot {
        let mut detail = detail.into();
        let (windows, account, source) = match &self.last_good {
            Some((at, windows, account)) => {
                let age = now.signed_duration_since(*at);
                detail = format!("{detail} (showing figures from {} ago)", human_age(age));
                (windows.clone(), account.clone(), "oauth (cached)")
            }
            // Nothing of our own to fall back on: the desktop app's reading of
            // the same windows beats a blank ring, and beats token counts that
            // answer a different question.
            None => match self.app_reading(now) {
                Some((windows, _)) => {
                    detail = format!("{detail} (showing the Claude app's reading)");
                    (windows, None, "claude app")
                }
                None => (Self::transcript_windows(&sessions), None, "transcripts"),
            },
        };

        let mut snap = ProviderSnapshot::new(ProviderId::ClaudeCode)
            .with_source(source)
            .with_account(account)
            .with_windows(windows)
            .with_sessions(sessions);
        snap.health = health;
        snap.detail = Some(detail);
        snap.retry_at = self.backoff.next_attempt();
        snap
    }
}

/// "3m", "2h" — compact enough for a one-line status.
fn human_age(age: chrono::Duration) -> String {
    let secs = age.num_seconds().max(0);
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h", s / 3600),
    }
}

/// Aggregate per-provider token counts keyed by model, for the expanded card.
pub fn tokens_by_model(sessions: &[Session]) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    for session in sessions {
        if let (Some(model), Some(tokens)) = (&session.model, session.tokens) {
            *out.entry(model.clone()).or_insert(0) += tokens;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The two cache lifetimes are priced differently — 1.25x base input
    /// against 2x — so a line that spends on the hour-long one must not be
    /// reported as having spent on the cheap one.
    #[test]
    fn the_two_cache_lifetimes_are_kept_apart() {
        let line = r#"{"message":{"model":"claude-opus-5","usage":{
            "input_tokens":10,"output_tokens":20,
            "cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":547},
            "cache_creation_input_tokens":547,"cache_read_input_tokens":99}}}"#;
        let usage = line_usage(line).expect("a usage block");
        assert_eq!(usage.cache_write, 0);
        assert_eq!(usage.cache_write_1h, 547);
        assert_eq!(usage.cache_read, 99);
        assert_eq!(line_model(line).as_deref(), Some("claude-opus-5"));
    }

    /// Transcripts written before the breakdown existed report one total.
    /// Taking it for the cheaper kind understates rather than invents.
    #[test]
    fn an_older_transcript_still_counts_its_cache_writes() {
        let line = r#"{"message":{"usage":{
            "input_tokens":1,"output_tokens":2,"cache_creation_input_tokens":300}}}"#;
        let usage = line_usage(line).expect("a usage block");
        assert_eq!(usage.cache_write, 300);
        assert_eq!(usage.cache_write_1h, 0);
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-01-01T12:00:00Z";

    #[test]
    fn parses_the_nested_credentials_envelope() {
        let raw = r#"{
          "claudeAiOauth": {
            "accessToken": "sk-ant-oat01-abc",
            "refreshToken": "sk-ant-ort01-xyz",
            "expiresAt": 1798761600000,
            "scopes": ["user:inference"],
            "subscriptionType": "max"
          }
        }"#;
        let creds = parse_credentials(raw).expect("should parse");
        assert_eq!(creds.access_token, "sk-ant-oat01-abc");
        assert_eq!(creds.subscription.as_deref(), Some("max"));
        assert_eq!(creds.expires_at, Some(at("2027-01-01T00:00:00Z")));
    }

    #[test]
    fn parses_a_flat_credentials_object_too() {
        let raw = r#"{"access_token":"tok","expires_at":"2027-01-01T00:00:00Z"}"#;
        let creds = parse_credentials(raw).unwrap();
        assert_eq!(creds.access_token, "tok");
        assert_eq!(creds.expires_at, Some(at("2027-01-01T00:00:00Z")));
    }

    #[test]
    fn rejects_credentials_without_a_token() {
        assert!(parse_credentials(r#"{"claudeAiOauth":{"refreshToken":"x"}}"#).is_none());
        assert!(parse_credentials(r#"{"claudeAiOauth":{"accessToken":""}}"#).is_none());
        assert!(parse_credentials("not json").is_none());
        assert!(parse_credentials("").is_none());
    }

    #[test]
    fn expiry_is_checked_against_the_current_time() {
        let creds = Credentials {
            access_token: "t".into(),
            expires_at: Some(at("2026-01-01T11:00:00Z")),
            subscription: None,
        };
        assert!(creds.is_expired(at(NOW)));
        assert!(!creds.is_expired(at("2026-01-01T10:00:00Z")));

        // No expiry recorded means we optimistically try the token.
        let creds = Credentials {
            expires_at: None,
            ..creds
        };
        assert!(!creds.is_expired(at(NOW)));
    }

    #[test]
    fn parses_the_documented_usage_shape() {
        let raw = r#"{
          "five_hour":  {"utilization": 42,   "resets_at": "2026-01-01T15:00:00Z"},
          "seven_day":  {"utilization": 12.5, "resets_at": "2026-01-05T00:00:00Z"},
          "seven_day_opus": {"utilization": 0, "resets_at": "2026-01-05T00:00:00Z"}
        }"#;
        let windows = parse_usage(raw);
        assert_eq!(windows.len(), 3);

        // Shortest window first.
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].label, "5h session");
        assert_eq!(windows[0].used_pct, Some(42.0));
        assert_eq!(windows[0].resets_at, Some(at("2026-01-01T15:00:00Z")));

        assert_eq!(windows[1].key, "seven_day");
        assert_eq!(windows[1].used_pct, Some(12.5));
        assert!(!windows[0].estimated, "API figures are not estimates");
    }

    #[test]
    fn parses_used_limit_pairs_when_no_percentage_is_given() {
        let raw = r#"{"five_hour": {"used": 25000, "limit": 100000}}"#;
        let windows = parse_usage(raw);
        assert_eq!(windows[0].used_pct, Some(25.0));
        assert_eq!(windows[0].used, Some(25000.0));
        assert_eq!(windows[0].unit, UsageUnit::Tokens);
    }

    #[test]
    fn survives_schema_drift_in_the_undocumented_endpoint() {
        // Wrapped in `usage`, list-shaped, alternative field names.
        let raw = r#"{"usage": {"windows": [
            {"key": "five_hour", "used_percent": 80, "resetsAt": 1767279600}
        ]}}"#;
        let windows = parse_usage(raw);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_pct, Some(80.0));
        assert_eq!(windows[0].resets_at, Some(at("2026-01-01T15:00:00Z")));

        // A brand-new window key still renders with a readable label.
        let windows = parse_usage(r#"{"thirty_day_sonnet": {"utilization": 5}}"#);
        assert_eq!(windows[0].label, "Thirty Day Sonnet");
    }

    #[test]
    fn unparseable_or_empty_usage_yields_nothing_rather_than_zeroes() {
        assert!(parse_usage("not json").is_empty());
        assert!(parse_usage("{}").is_empty());
        // An object with no usable numbers must not become a 0% ring.
        assert!(parse_usage(r#"{"five_hour": {"resets_at": "2026-01-01T15:00:00Z"}}"#).is_empty());
    }

    #[test]
    fn plan_labels_are_title_cased() {
        assert_eq!(plan_label(Some("max")).as_deref(), Some("Max"));
        assert_eq!(plan_label(Some("pro")).as_deref(), Some("Pro"));
        assert_eq!(
            plan_label(Some("something_new")).as_deref(),
            Some("Something_new")
        );
        assert_eq!(plan_label(None), None);
    }

    /// Build a transcript file from `(type, offset_secs, extra_json)` tuples.
    fn write_transcript(dir: &Path, name: &str, entries: &[(&str, i64, &str)]) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        for (kind, offset, extra) in entries {
            let ts = (at(NOW) + chrono::Duration::seconds(*offset)).to_rfc3339();
            let extra = if extra.is_empty() {
                String::new()
            } else {
                format!(",{extra}")
            };
            writeln!(
                f,
                r#"{{"type":"{kind}","timestamp":"{ts}","cwd":"C:\\dev\\myapp"{extra}}}"#
            )
            .unwrap();
        }
        path
    }

    #[test]
    fn records_which_front_end_a_session_runs_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "desktop.jsonl",
            &[
                ("user", -60, r#""entrypoint":"claude-desktop""#),
                ("assistant", -30, r#""entrypoint":"claude-desktop""#),
            ],
        );
        let summary = summarise_transcript(&path, at(NOW)).unwrap();
        assert_eq!(summary.entrypoint.as_deref(), Some("claude-desktop"));
        let (app, editor) = if cfg!(target_os = "linux") {
            ("claude-desktop", "code")
        } else if cfg!(target_os = "macos") {
            ("Claude", "Code")
        } else {
            ("claude.exe", "Code.exe")
        };
        assert!(crate::model::host_processes("claude-desktop")
            .unwrap()
            .contains(&app));
        assert!(crate::model::host_processes("claude-vscode")
            .unwrap()
            .contains(&editor));
    }

    /// The real shape, from Claude Code's own cache of the same reply: three
    /// entries, and the third names its model rather than hiding behind a
    /// codename.
    #[test]
    fn the_named_limits_include_the_per_model_week() {
        let raw = r#"{"limits": [
          {"kind": "session", "group": "session", "percent": 3,
           "resets_at": "2027-09-19T21:10:00.192302+00:00", "scope": null},
          {"kind": "weekly_all", "group": "weekly", "percent": 1,
           "resets_at": "2027-09-26T07:00:00.192321+00:00", "scope": null},
          {"kind": "weekly_scoped", "group": "weekly", "percent": 0,
           "resets_at": "2027-09-26T07:00:00+00:00",
           "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}}
        ]}"#;
        let windows = parse_limits(raw, at("2026-01-01T00:00:00Z"));

        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].used_pct, Some(3.0));
        assert_eq!(windows[1].key, "seven_day");
        assert!(windows[1].weekly);
        // Named by the vendor, not deduced from an internal key.
        assert_eq!(windows[2].key, "seven_day_fable");
        assert_eq!(windows[2].label, "7d Fable");
        assert!(windows[2].weekly);
        assert_eq!(windows[2].window_minutes, Some(7 * 24 * 60));
    }

    /// A kind this build has never met is skipped rather than drawn wrong.
    #[test]
    fn an_unknown_kind_of_limit_is_left_alone() {
        let raw = r#"{"limits": [
          {"kind": "monthly_whatever", "percent": 50},
          {"kind": "session", "percent": 10}
        ]}"#;
        let windows = parse_limits(raw, at("2026-01-01T00:00:00Z"));
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, "five_hour");
    }

    /// `.claude-mem` is a plugin's data directory, not a second account, and
    /// drawing a ring for it would be inventing an account the user doesn't
    /// have. What makes a directory an account is that Claude Code signed in
    /// or worked there.
    #[test]
    fn only_directories_that_look_like_accounts_count_as_accounts() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        std::fs::create_dir_all(root.join(".claude/projects")).unwrap();
        std::fs::create_dir_all(root.join(".claude-work/projects")).unwrap();
        std::fs::create_dir_all(root.join(".claude-mem/logs")).unwrap();
        std::fs::create_dir_all(root.join(".claude-old")).unwrap();
        std::fs::write(root.join(".claude-old/.credentials.json"), "{}").unwrap();

        let found = accounts_in(Some(root.to_path_buf()), None);
        let names: Vec<_> = found.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            names,
            vec![None, Some("old".to_string()), Some("work".to_string())],
            "the default account leads, the rest are sorted, and the plugin is not one"
        );
    }

    /// What the environment points at is the account the agents in this
    /// session are using, whatever it is called.
    #[test]
    fn a_configured_directory_is_the_default_account() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".claude/projects")).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();

        let found = accounts_in(
            Some(home.path().to_path_buf()),
            Some(elsewhere.path().to_string_lossy().to_string()),
        );
        assert_eq!(found[0].0, None);
        assert_eq!(found[0].1.root, elsewhere.path());
    }

    #[test]
    fn detects_a_session_waiting_for_tool_approval() {
        let dir = tempfile::tempdir().unwrap();
        // Assistant asked to run a tool 30s ago and nothing has come back.
        let path = write_transcript(
            dir.path(),
            "s1.jsonl",
            &[
                ("user", -120, r#""message":{"role":"user","content":[]}"#),
                (
                    "assistant",
                    -30,
                    r#""message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}"#,
                ),
            ],
        );
        let summary = summarise_transcript(&path, at(NOW)).unwrap();
        assert_eq!(summary.activity, Activity::AwaitingInput);
        assert_eq!(summary.project, "myapp");
    }

    #[test]
    fn the_end_of_turn_record_ends_the_turn() {
        // Claude Code writes `system` / `turn_duration` when a turn is over.
        // Inferring it from silence instead meant a reply that landed between
        // two polls was never seen as finished.
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "ended.jsonl",
            &[
                (
                    "assistant",
                    -3,
                    r#""message":{"role":"assistant","content":[{"type":"text","text":"done"}]}"#,
                ),
                ("system", -2, r#""subtype":"turn_duration""#),
            ],
        );
        let summary = summarise_transcript(&path, at(NOW)).unwrap();
        assert_eq!(summary.activity, Activity::Done, "ended 2s ago");
    }

    #[test]
    fn a_slow_tool_is_working_until_it_looks_like_a_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let call = r#""message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash"}]}"#;

        // Two seconds in, Bash is just running.
        let fresh = write_transcript(dir.path(), "fresh.jsonl", &[("assistant", -2, call)]);
        assert_eq!(
            summarise_transcript(&fresh, at(NOW)).unwrap().activity,
            Activity::Generating
        );

        // Still unanswered much later: that's a permission prompt.
        let stuck = write_transcript(dir.path(), "stuck.jsonl", &[("assistant", -40, call)]);
        assert_eq!(
            summarise_transcript(&stuck, at(NOW)).unwrap().activity,
            Activity::AwaitingInput
        );
    }

    #[test]
    fn a_slow_subagent_is_not_a_prompt() {
        // Task spawns a subagent and takes minutes; it isn't asking anything.
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "task.jsonl",
            &[(
                "assistant",
                -300,
                r#""message":{"role":"assistant","content":[{"type":"tool_use","id":"t9","name":"Task"}]}"#,
            )],
        );
        assert_ne!(
            summarise_transcript(&path, at(NOW)).unwrap().activity,
            Activity::AwaitingInput
        );
    }

    #[test]
    fn a_talk_only_turn_settles_after_a_moment() {
        // No end-of-turn record follows a reply with no tool calls, so a short
        // silence is what ends it.
        let dir = tempfile::tempdir().unwrap();
        let text = r#""message":{"role":"assistant","content":[{"type":"text","text":"hi"}]}"#;

        let just_now = write_transcript(dir.path(), "talking.jsonl", &[("assistant", -1, text)]);
        assert_eq!(
            summarise_transcript(&just_now, at(NOW)).unwrap().activity,
            Activity::Generating
        );

        let settled = write_transcript(dir.path(), "talked.jsonl", &[("assistant", -8, text)]);
        assert_eq!(
            summarise_transcript(&settled, at(NOW)).unwrap().activity,
            Activity::Done
        );
    }

    #[test]
    fn an_approved_tool_call_is_not_treated_as_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "s2.jsonl",
            &[
                (
                    "assistant",
                    -60,
                    r#""message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash"}]}"#,
                ),
                (
                    "user",
                    -55,
                    r#""message":{"role":"user","content":[{"type":"tool_result"}]}"#,
                ),
            ],
        );
        let summary = summarise_transcript(&path, at(NOW)).unwrap();
        assert_eq!(summary.activity, Activity::Done);
    }

    #[test]
    fn a_very_recent_turn_counts_as_generating() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "s3.jsonl",
            &[(
                "assistant",
                -5,
                r#""message":{"role":"assistant","content":[]}"#,
            )],
        );
        assert_eq!(
            summarise_transcript(&path, at(NOW)).unwrap().activity,
            Activity::Generating
        );
    }

    #[test]
    fn activity_decays_through_done_to_idle() {
        let dir = tempfile::tempdir().unwrap();
        let cases = [(-60, Activity::Done), (-3600, Activity::Idle)];
        for (offset, expected) in cases {
            let path = write_transcript(
                dir.path(),
                &format!("s{offset}.jsonl"),
                &[(
                    "assistant",
                    offset,
                    r#""message":{"role":"assistant","content":[]}"#,
                )],
            );
            assert_eq!(
                summarise_transcript(&path, at(NOW)).unwrap().activity,
                expected,
                "offset {offset}s"
            );
        }
    }

    #[test]
    fn a_stale_pending_tool_call_stops_nagging() {
        // A tool_use from days ago is an abandoned session, not a live prompt.
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "old.jsonl",
            &[(
                "assistant",
                -86_400,
                r#""message":{"role":"assistant","content":[{"type":"tool_use"}]}"#,
            )],
        );
        assert_eq!(
            summarise_transcript(&path, at(NOW)).unwrap().activity,
            Activity::Idle
        );
    }

    #[test]
    fn sums_every_token_bucket_in_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_transcript(
            dir.path(),
            "tokens.jsonl",
            &[
                (
                    "assistant",
                    -300,
                    r#""message":{"model":"claude-opus-5","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":1000,"cache_creation_input_tokens":25}}"#,
                ),
                (
                    "assistant",
                    -100,
                    r#""message":{"model":"claude-opus-5","usage":{"input_tokens":10,"output_tokens":5}}"#,
                ),
                // Outside the 5h window: must not be counted.
                (
                    "assistant",
                    -60 * 60 * 9,
                    r#""message":{"usage":{"input_tokens":999999}}"#,
                ),
            ],
        );
        let s = summarise_transcript(&path, at(NOW)).unwrap();
        assert_eq!(s.tokens, 100 + 50 + 1000 + 25 + 10 + 5);
        assert_eq!(s.model.as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn malformed_transcript_lines_are_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.jsonl");
        let ts = at(NOW).to_rfc3339();
        std::fs::write(
            &path,
            format!(
                "garbage not json\n\
                 {{\"type\":\"assistant\",\"timestamp\":\"{ts}\",\"cwd\":\"C:\\\\dev\\\\app\"}}\n\
                 {{unclosed\n"
            ),
        )
        .unwrap();
        let s = summarise_transcript(&path, at(NOW)).expect("valid lines still parse");
        assert_eq!(s.project, "app");
    }

    #[test]
    fn an_empty_transcript_yields_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.jsonl");
        std::fs::write(&path, "").unwrap();
        assert!(summarise_transcript(&path, at(NOW)).is_none());
    }

    #[test]
    fn project_name_falls_back_to_the_encoded_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("C--Users-dev-cool-app");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("s.jsonl");
        std::fs::write(
            &path,
            format!(
                r#"{{"type":"assistant","timestamp":"{}"}}"#,
                at(NOW).to_rfc3339()
            ),
        )
        .unwrap();
        assert_eq!(summarise_transcript(&path, at(NOW)).unwrap().project, "app");
    }

    #[test]
    fn scan_orders_blocked_sessions_ahead_of_busy_ones() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        write_transcript(
            &projects,
            "busy.jsonl",
            &[(
                "assistant",
                -3,
                r#""message":{"role":"assistant","content":[]}"#,
            )],
        );
        write_transcript(
            &projects,
            "blocked.jsonl",
            &[(
                "assistant",
                -40,
                r#""message":{"role":"assistant","content":[{"type":"tool_use"}]}"#,
            )],
        );

        let adapter = ClaudeAdapter::with_paths(
            reqwest::Client::new(),
            ClaudePaths {
                root: dir.path().to_path_buf(),
            },
        );
        let sessions = adapter.scan_sessions(at(NOW));
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "blocked");
        assert_eq!(sessions[0].activity, Activity::AwaitingInput);
        assert_eq!(sessions[1].activity, Activity::Generating);
    }

    #[tokio::test]
    async fn a_missing_install_reports_unavailable_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing of Claude Code's, and no desktop app either.
        let mut adapter = ClaudeAdapter::with_paths(
            reqwest::Client::new(),
            ClaudePaths {
                root: dir.path().join("does-not-exist"),
            },
        )
        .with_app_usage(None);
        let snap = adapter.collect(at(NOW)).await;
        assert_eq!(snap.health, Health::Unavailable);
        assert!(snap.windows.is_empty());
    }

    /// A history file shaped like the desktop app's, taken `mins` ago.
    fn write_app_usage(dir: &Path, mins: i64, five_hour: i64, weekly: i64) -> PathBuf {
        let path = dir.join("plan-usage-history.json");
        let taken = (at(NOW) - chrono::Duration::minutes(mins)).timestamp_millis();
        // With an older sample on either side of it, so the newest is what is
        // read rather than the first or the last in the list.
        let raw = format!(
            r#"{{"version":2,"samples":[
                 {{"t":{older},"org":"an-org","u":{{"fh":99,"sd":99}}}},
                 {{"t":{taken},"org":"an-org","u":{{"fh":{five_hour},"sd":{weekly},"xu":0}}}},
                 {{"t":{older},"org":"an-org","u":{{"fh":98,"sd":98}}}}
               ]}}"#,
            older = taken - 900_000,
        );
        std::fs::write(&path, raw).unwrap();
        path
    }

    /// An adapter with no Claude Code of its own, reading the app's history.
    fn app_only_adapter(dir: &Path, usage: PathBuf) -> ClaudeAdapter {
        ClaudeAdapter::with_paths(
            reqwest::Client::new(),
            ClaudePaths {
                root: dir.join("no-claude-code"),
            },
        )
        .with_app_usage(Some(usage))
    }

    #[test]
    fn the_newest_sample_of_the_app_history_is_the_one_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_app_usage(dir.path(), 10, 41, 36);
        let reading = parse_app_usage(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(reading.five_hour_pct, Some(41.0));
        assert_eq!(reading.seven_day_pct, Some(36.0));
        assert_eq!(reading.taken_at, at(NOW) - chrono::Duration::minutes(10));
    }

    #[tokio::test]
    async fn the_desktop_app_stands_in_where_claude_code_never_signed_in() {
        // Someone with the Claude app and not Claude Code: no directory of its
        // own, no credentials, no transcripts -- and limits to show anyway.
        let dir = tempfile::tempdir().unwrap();
        let usage = write_app_usage(dir.path(), 10, 41, 36);
        let mut adapter = app_only_adapter(dir.path(), usage);

        let snap = adapter.collect(at(NOW)).await;
        assert_eq!(snap.health, Health::Ok);
        assert_eq!(snap.source.as_deref(), Some("claude app"));
        assert_eq!(snap.windows.len(), 2);
        assert_eq!(snap.windows[0].key, "five_hour");
        assert_eq!(snap.windows[0].used_pct, Some(41.0));
        assert_eq!(snap.windows[1].key, "seven_day");
        assert_eq!(snap.windows[1].used_pct, Some(36.0));
        // Not an estimate of ours: the app read these where the endpoint would.
        assert!(!snap.windows[0].estimated);
    }

    #[tokio::test]
    async fn an_app_that_stopped_watching_keeps_only_what_still_holds() {
        let dir = tempfile::tempdir().unwrap();
        // Three hours old: the five-hour window may have rolled over unseen,
        // while the week only ever grows, so it stays as a floor.
        let usage = write_app_usage(dir.path(), 180, 41, 36);
        let mut adapter = app_only_adapter(dir.path(), usage);

        let snap = adapter.collect(at(NOW)).await;
        assert_eq!(snap.health, Health::Stale);
        assert_eq!(snap.windows.len(), 1);
        assert_eq!(snap.windows[0].key, "seven_day");
    }

    #[tokio::test]
    async fn a_day_old_app_reading_says_nothing_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let usage = write_app_usage(dir.path(), 24 * 60, 41, 36);
        let mut adapter = app_only_adapter(dir.path(), usage);

        let snap = adapter.collect(at(NOW)).await;
        assert_eq!(snap.health, Health::Unavailable);
        assert!(snap.windows.is_empty());
    }

    #[tokio::test]
    async fn an_extra_account_never_borrows_the_app_account_s_figures() {
        // The app is signed into one account; a second Claude Code account is
        // not it, and would be showing someone else's limits under its name.
        let dir = tempfile::tempdir().unwrap();
        let usage = write_app_usage(dir.path(), 5, 41, 36);
        let mut adapter = app_only_adapter(dir.path(), usage).own_credentials_only();

        let snap = adapter.collect(at(NOW)).await;
        assert_eq!(snap.health, Health::Unavailable);
    }

    #[tokio::test]
    async fn an_installed_but_signed_out_claude_still_reports_activity() {
        let dir = tempfile::tempdir().unwrap();
        let projects = dir.path().join("projects");
        std::fs::create_dir_all(&projects).unwrap();
        write_transcript(
            &projects,
            "s.jsonl",
            &[(
                "assistant",
                -10,
                r#""message":{"role":"assistant","usage":{"output_tokens":42},"content":[]}"#,
            )],
        );

        let mut adapter = ClaudeAdapter::with_paths(
            reqwest::Client::new(),
            ClaudePaths {
                root: dir.path().to_path_buf(),
            },
        )
        .with_app_usage(None);
        let snap = adapter.collect(at(NOW)).await;
        assert_eq!(snap.health, Health::NeedsAuth);
        assert_eq!(snap.activity, Activity::Generating);
        assert_eq!(snap.source.as_deref(), Some("transcripts"));
        // The token figure is derived locally, so it must be marked estimated.
        assert_eq!(snap.windows.len(), 1);
        assert!(snap.windows[0].estimated);
        assert_eq!(snap.windows[0].used, Some(42.0));
        assert_eq!(
            snap.windows[0].used_pct, None,
            "no denominator, no percentage"
        );
    }

    #[test]
    fn tokens_by_model_aggregates_across_sessions() {
        let sessions = vec![
            Session {
                model: Some("opus".into()),
                tokens: Some(10),
                ..Session::new("a", "a")
            },
            Session {
                model: Some("opus".into()),
                tokens: Some(5),
                ..Session::new("b", "b")
            },
            Session {
                model: Some("haiku".into()),
                tokens: Some(1),
                ..Session::new("c", "c")
            },
            Session {
                model: None,
                tokens: Some(99),
                ..Session::new("d", "d")
            },
        ];
        let totals = tokens_by_model(&sessions);
        assert_eq!(totals.get("opus"), Some(&15));
        assert_eq!(totals.get("haiku"), Some(&1));
        assert_eq!(totals.len(), 2);
    }

    #[test]
    fn human_age_is_compact() {
        assert_eq!(human_age(chrono::Duration::seconds(5)), "5s");
        assert_eq!(human_age(chrono::Duration::minutes(3)), "3m");
        assert_eq!(human_age(chrono::Duration::hours(2)), "2h");
    }
}
