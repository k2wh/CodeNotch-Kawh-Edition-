//! Agent hooks: being told what happened instead of inferring it.
//!
//! Reading transcripts answers "what is this agent doing?" a second or two
//! late and, for a turn that starts and ends between two reads, not at all.
//! Claude Code and Codex will both run a command on every interesting event —
//! a turn ending, a permission prompt, a tool starting — so with hooks
//! installed the notch is *told*, in the moment.
//!
//! The hook command is this same executable with `--hook <provider>`: no
//! runtime to install, nothing to keep in step with the app's version. In that
//! mode it reads the event from stdin, appends a line to a queue file and
//! exits; the running notch tails that file. A queue rather than a socket
//! because it survives the notch not running, needs no port and no token, and
//! a hook that can't reach anything must still exit promptly and quietly —
//! it is running inside the user's agent.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use codenotch_core::model::{Activity, ProviderId};

/// Marks the entries in a settings file as ours, so they can be removed
/// again without touching anyone else's.
const MARKER: &str = "--hook";

/// Claude Code events worth a state change.
const CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Notification",
    "Stop",
];

/// The same for Codex, which names them almost identically.
const CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
];

/// Queue lines older than this are dropped on read: an event nobody was
/// running to receive says nothing useful about now.
const MAX_EVENT_AGE_SECS: i64 = 120;
/// Past this, the queue is rewritten from scratch rather than appended to.
const MAX_QUEUE_BYTES: u64 = 256 * 1024;

/// One thing an agent reported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    pub at: DateTime<Utc>,
    pub provider: String,
    pub event: String,
    #[serde(default)]
    pub session_id: Option<String>,
}

impl HookEvent {
    pub fn provider_id(&self) -> Option<ProviderId> {
        match self.provider.as_str() {
            "claude" => Some(ProviderId::ClaudeCode),
            "codex" => Some(ProviderId::Codex),
            _ => None,
        }
    }

    /// What this event says the agent is doing now.
    ///
    /// `Notification` covers both "may I run this?" and "I'm waiting for
    /// you", which are the same thing as far as a ring is concerned.
    pub fn activity(&self) -> Option<Activity> {
        Some(match self.event.as_str() {
            "Stop" => Activity::Done,
            "Notification" | "PermissionRequest" => Activity::AwaitingInput,
            "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => {
                Activity::Generating
            }
            _ => return None,
        })
    }
}

/// `%USERPROFILE%\.codenotch\hook-events.jsonl`.
///
/// Deliberately not in `%APPDATA%`, where the rest of the state lives. A hook
/// runs inside whatever process tree called it, and an agent running inside a
/// packaged app — Claude Code in the Claude desktop app, say — has its
/// `%APPDATA%` writes redirected into that app's private store. The notch
/// runs outside it, so a queue there is written on one side of the wall and
/// read on the other, and the instant updates quietly stop arriving. The
/// profile root is not redirected, which is why the agents keep their own
/// state in `~/.claude` and `~/.codex` rather than in `%APPDATA%`.
pub fn queue_path() -> Result<PathBuf> {
    let dir = dirs::home_dir()
        .context("could not resolve the home directory")?
        .join(".codenotch");
    Ok(dir.join("hook-events.jsonl"))
}

/// Where the queue used to be, so an update can clear it away.
fn old_queue_path() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("CodeNotch")
            .join("hook-events.jsonl"),
    )
}

/// Command-line work that isn't the app: reporting an event as a hook, or
/// installing the hooks themselves.
///
/// Returns true when the process has done its job and should exit. Every
/// failure on the hook path is silent: a hook that prints or hangs interrupts
/// the agent that called it, which is far worse than a missed event.
pub fn run_as_hook_if_asked() -> bool {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    // `--hooks on|off`: install or remove, pointing at whichever copy of the
    // app is running it. Used by the installer, and by hand after moving the
    // app, so the hooks never point at an executable that isn't there.
    if first.as_deref() == Some("--hooks") {
        let enable = !matches!(args.next().as_deref(), Some("off") | Some("false"));
        let _ = set_installed(enable);
        return true;
    }

    if first.as_deref() != Some(MARKER) {
        return false;
    }
    let provider = args.next().unwrap_or_else(|| "claude".into());

    let mut raw = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut raw);
    let json: Value = serde_json::from_str(&decode(&raw)).unwrap_or(Value::Null);

    let event = HookEvent {
        at: Utc::now(),
        provider,
        event: json
            .get("hook_event_name")
            .or_else(|| json.get("hookEventName"))
            .or_else(|| json.get("event"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        session_id: json
            .get("session_id")
            .or_else(|| json.get("sessionId"))
            .and_then(Value::as_str)
            .map(str::to_string),
    };

    let _ = append(&event);
    true
}

/// Text from whatever the caller wrote down the pipe.
///
/// Agents send UTF-8, but a shell in the middle may hand over UTF-16 (with or
/// without a byte-order mark), and an event lost to an encoding is an event
/// lost for good.
fn decode(raw: &[u8]) -> String {
    let utf16 = |bytes: &[u8], little: bool| {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|p| {
                if little {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };

    match raw {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        // Plain ASCII as UTF-16LE looks like "{\0\"\0..." — every other byte 0.
        _ if raw.len() >= 4 && raw[1] == 0 && raw[3] == 0 => utf16(raw, true),
        _ => String::from_utf8_lossy(raw).into_owned(),
    }
}

fn append(event: &HookEvent) -> Result<()> {
    let path = queue_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A queue nobody drained (the notch wasn't running) is stale anyway.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_QUEUE_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(file, "{}", serde_json::to_string(event)?)?;
    Ok(())
}

/// Read whatever has been appended since `offset`, leaving it updated.
pub fn drain(offset: &mut u64) -> Vec<HookEvent> {
    let Ok(path) = queue_path() else {
        return Vec::new();
    };
    let Ok(mut file) = File::open(&path) else {
        *offset = 0;
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    // Truncated or replaced underneath us: start again from the top.
    if len < *offset {
        *offset = 0;
    }
    if len == *offset {
        return Vec::new();
    }

    let mut text = String::new();
    if file.seek(SeekFrom::Start(*offset)).is_err() || file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    // Only whole lines; a hook may be mid-write.
    let end = text.rfind('\n').map_or(0, |i| i + 1);
    *offset += end as u64;

    let cutoff = Utc::now() - chrono::Duration::seconds(MAX_EVENT_AGE_SECS);
    text[..end]
        .lines()
        .filter_map(|line| serde_json::from_str::<HookEvent>(line).ok())
        .filter(|event| event.at >= cutoff)
        .collect()
}

/// Are our hooks installed for both providers?
pub fn installed() -> bool {
    claude_installed().unwrap_or(false) && codex_installed().unwrap_or(false)
}

/// Install or remove the hooks for both agents.
///
/// Other tools install hooks too, so entries are merged and removed by their
/// command rather than by rewriting the file: everyone else's survive.
pub fn set_installed(enabled: bool) -> Result<()> {
    let claude = write_claude(enabled);
    let codex = write_codex(enabled);
    claude.and(codex)
}

fn hook_command(provider: &str) -> Result<String> {
    let exe = std::env::current_exe()?;
    Ok(format!("\"{}\" {MARKER} {provider}", exe.display()))
}

fn is_ours(command: &str) -> bool {
    command.contains(MARKER) && command.to_lowercase().contains("codenotch")
}

/// Does this command run *this* copy of the app?
fn points_here(command: &str) -> bool {
    std::env::current_exe().is_ok_and(|exe| command.contains(&exe.display().to_string()))
}

/// Re-point installed hooks at this executable.
///
/// An update or a move leaves the agents calling a path that no longer
/// exists, and a hook that fails is a silently dead feature. Cheap enough to
/// run at every start: it rewrites only when something is actually wrong, and
/// does nothing at all when the hooks aren't installed.
pub fn repair() {
    // A queue left behind at the old location is only confusing: events in
    // it were written before the move and have long since been answered by
    // the transcript scan.
    if let Some(old) = old_queue_path() {
        if old.exists() {
            let _ = std::fs::remove_file(&old);
        }
    }

    let stale = [claude_settings(), codex_hooks()]
        .into_iter()
        .flatten()
        .any(|path| {
            commands_in(&read_json(&path))
                .iter()
                .any(|command| is_ours(command) && !points_here(command))
        });

    if stale {
        let _ = set_installed(true);
    }
}

/// Every hook command in a settings file, whoever owns it.
fn commands_in(settings: &Value) -> Vec<String> {
    let hooks = match settings.get("hooks") {
        Some(hooks) => hooks,
        None => return Vec::new(),
    };
    hooks
        .as_object()
        .into_iter()
        .flat_map(|events| events.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks")?.as_array())
        .flatten()
        .filter_map(|hook| hook.get("command")?.as_str())
        .map(str::to_string)
        .collect()
}

/// Read a JSON file, or an empty object when it isn't there yet.
fn read_json(path: &PathBuf) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}))
}

/// Write JSON out without ever leaving a half-written settings file behind.
fn write_json(path: &PathBuf, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Put our entry in (or take it out of) one event's list, leaving the rest of
/// the list as it was.
fn merge_event(list: &mut Value, entry: Value, enabled: bool) {
    let items = match list {
        Value::Array(items) => items,
        _ => {
            *list = Value::Array(Vec::new());
            list.as_array_mut().expect("just set")
        }
    };

    items.retain(|group| {
        !group
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(is_ours)
                })
            })
    });

    if enabled {
        items.push(entry);
    }
}

fn claude_settings() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join(".claude")
        .join("settings.json"))
}

fn write_claude(enabled: bool) -> Result<()> {
    let path = claude_settings()?;
    let mut settings = read_json(&path);
    let command = hook_command("claude")?;

    let hooks = settings
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    for event in CLAUDE_EVENTS {
        let entry = json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": command, "timeout": 5 }],
        });
        merge_event(
            hooks
                .as_object_mut()
                .context("hooks is not an object")?
                .entry(*event)
                .or_insert_with(|| json!([])),
            entry,
            enabled,
        );
    }

    write_json(&path, &settings)
}

fn claude_installed() -> Result<bool> {
    let settings = read_json(&claude_settings()?);
    Ok(settings
        .get("hooks")
        .and_then(|h| h.get("Stop"))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(is_ours)
                        })
                    })
            })
        }))
}

fn codex_hooks() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join(".codex")
        .join("hooks.json"))
}

fn write_codex(enabled: bool) -> Result<()> {
    let path = codex_hooks()?;
    let mut file = read_json(&path);
    let command = hook_command("codex")?;

    let hooks = file
        .as_object_mut()
        .context("hooks.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    for event in CODEX_EVENTS {
        // `async` so Codex never waits on us; `Stop` is the one we want
        // promptly, and it is cheap either way.
        let entry = json!({
            "hooks": [{ "type": "command", "command": command, "timeout": 5, "async": true }],
        });
        merge_event(
            hooks
                .as_object_mut()
                .context("hooks is not an object")?
                .entry(*event)
                .or_insert_with(|| json!([])),
            entry,
            enabled,
        );
    }

    write_json(&path, &file)
}

fn codex_installed() -> Result<bool> {
    let file = read_json(&codex_hooks()?);
    Ok(file
        .get("hooks")
        .and_then(|h| h.get("Stop"))
        .and_then(Value::as_array)
        .is_some_and(|groups| {
            groups.iter().any(|group| {
                group
                    .get("hooks")
                    .and_then(Value::as_array)
                    .is_some_and(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(is_ours)
                        })
                    })
            })
        }))
}
