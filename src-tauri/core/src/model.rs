//! Types shared between the collectors, the Tauri commands and the webview.
//!
//! Everything here is serialised to the frontend as camelCase JSON. The guiding
//! rule, borrowed from the macOS original, is that a failed collection must
//! degrade to a *visible* status (`stale`, `needsAuth`, `error`) rather than an
//! invented number, so [`Health`] is never optional and never silently `Ok`.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// A list of Linux executables with the terminals a CLI agent may be running
/// in spliced into the middle, most common first. Terminator is Python, so it
/// goes by its script's name (see the Linux platform layer).
#[cfg(target_os = "linux")]
macro_rules! around_terminals {
    ([$($before:literal),*], [$($after:literal),*]) => {
        &[
            $($before,)*
            "gnome-terminal-server",
            "kgx",
            "ptyxis",
            "cosmic-term",
            "tilix",
            "konsole",
            "kitty",
            "alacritty",
            "wezterm-gui",
            "terminator",
            "xfce4-terminal",
            "foot",
            "xterm",
            $($after),*
        ]
    };
}

/// The same for macOS, where an app is matched by its executable or by the
/// name the Dock shows (Warp's executable is `stable`), without case.
#[cfg(target_os = "macos")]
macro_rules! around_terminals {
    ([$($before:literal),*], [$($after:literal),*]) => {
        &[
            $($before,)*
            "Terminal",
            "iTerm2",
            "Ghostty",
            "Warp",
            "WezTerm",
            "Alacritty",
            "kitty",
            "Hyper",
            "Tabby",
            $($after),*
        ]
    };
}

/// A provider CodeNotch knows how to collect from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderId {
    ClaudeCode,
    Cursor,
    Copilot,
    Codex,
    Gemini,
    Perplexity,
    Grok,
    Glm,
    Kimi,
    OpenCode,
    CommandCode,
    MiniMax,
    Ollama,
    LmStudio,
}

impl ProviderId {
    pub const ALL: [ProviderId; 14] = [
        ProviderId::ClaudeCode,
        ProviderId::Cursor,
        ProviderId::Copilot,
        ProviderId::Codex,
        ProviderId::Gemini,
        ProviderId::Perplexity,
        ProviderId::Grok,
        ProviderId::Glm,
        ProviderId::Kimi,
        ProviderId::OpenCode,
        ProviderId::CommandCode,
        ProviderId::MiniMax,
        ProviderId::Ollama,
        ProviderId::LmStudio,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::ClaudeCode => "Claude Code",
            ProviderId::Cursor => "Cursor",
            ProviderId::Copilot => "GitHub Copilot",
            ProviderId::Codex => "Codex",
            ProviderId::Gemini => "Gemini",
            ProviderId::Perplexity => "Perplexity",
            ProviderId::Grok => "Grok",
            ProviderId::Glm => "GLM",
            ProviderId::Kimi => "Kimi",
            ProviderId::OpenCode => "OpenCode",
            ProviderId::CommandCode => "Command Code",
            ProviderId::MiniMax => "MiniMax",
            ProviderId::Ollama => "Ollama",
            ProviderId::LmStudio => "LM Studio",
        }
    }

    /// Stable key used in the config file and in the frontend's sort order.
    pub fn key(self) -> &'static str {
        match self {
            ProviderId::ClaudeCode => "claudeCode",
            ProviderId::Cursor => "cursor",
            ProviderId::Copilot => "copilot",
            ProviderId::Codex => "codex",
            ProviderId::Gemini => "gemini",
            ProviderId::Perplexity => "perplexity",
            ProviderId::Grok => "grok",
            ProviderId::Glm => "glm",
            ProviderId::Kimi => "kimi",
            ProviderId::OpenCode => "opencode",
            ProviderId::CommandCode => "commandCode",
            ProviderId::MiniMax => "miniMax",
            ProviderId::Ollama => "ollama",
            ProviderId::LmStudio => "lmStudio",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        ProviderId::ALL.into_iter().find(|p| p.key() == key)
    }

    /// Executables to hunt for when the user clicks a provider card.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn focus_processes(self) -> &'static [&'static str] {
        match self {
            // Used when a session doesn't say where it runs (see
            // `host_processes`): the apps and terminals these agents live in.
            ProviderId::ClaudeCode => &[
                "claude.exe",
                "WindowsTerminal.exe",
                "wezterm-gui.exe",
                "alacritty.exe",
                "powershell.exe",
                "pwsh.exe",
                "cmd.exe",
                "Code.exe",
            ],
            ProviderId::Codex => &[
                "ChatGPT.exe",
                "Codex.exe",
                "WindowsTerminal.exe",
                "wezterm-gui.exe",
                "alacritty.exe",
                "powershell.exe",
                "pwsh.exe",
                "cmd.exe",
                "Code.exe",
            ],
            ProviderId::Gemini => &[
                "WindowsTerminal.exe",
                "wezterm-gui.exe",
                "alacritty.exe",
                "powershell.exe",
                "pwsh.exe",
                "cmd.exe",
                "Code.exe",
            ],
            ProviderId::Cursor => &["Cursor.exe"],
            ProviderId::Perplexity => &["Perplexity.exe", "Comet.exe"],
            ProviderId::Copilot => &["Code.exe", "devenv.exe", "WindowsTerminal.exe"],
            ProviderId::Grok
            | ProviderId::Glm
            | ProviderId::Kimi
            | ProviderId::OpenCode
            | ProviderId::CommandCode
            | ProviderId::MiniMax => &[
                "WindowsTerminal.exe",
                "powershell.exe",
                "pwsh.exe",
                "cmd.exe",
                "Code.exe",
            ],
            ProviderId::Ollama => &["ollama app.exe", "ollama.exe"],
            ProviderId::LmStudio => &["LM Studio.exe", "lms.exe"],
        }
    }

    /// Executables to hunt for when the user clicks a provider card: on Linux,
    /// as `/proc/<pid>/exe` names them, which for a terminal is whatever owns
    /// its windows (`gnome-terminal-server`, not `bash`).
    #[cfg(target_os = "linux")]
    pub fn focus_processes(self) -> &'static [&'static str] {
        match self {
            ProviderId::ClaudeCode => around_terminals!(["claude-desktop"], ["code"]),
            ProviderId::Codex => around_terminals!(["chatgpt", "codex"], ["code"]),
            ProviderId::Gemini => around_terminals!([], ["code"]),
            ProviderId::Cursor => &["cursor"],
            ProviderId::Perplexity => &["perplexity", "comet"],
            ProviderId::Copilot => around_terminals!(["code"], []),
            ProviderId::Grok
            | ProviderId::Glm
            | ProviderId::Kimi
            | ProviderId::OpenCode
            | ProviderId::CommandCode
            | ProviderId::MiniMax => around_terminals!([], ["code"]),
            ProviderId::Ollama => &["ollama"],
            ProviderId::LmStudio => &["lm-studio", "lm studio", "lms"],
        }
    }

    /// Apps to hunt for when the user clicks a provider card: on macOS, by
    /// executable or Dock name. Only a fallback there, after the agent's own
    /// process has been followed up to its app (see the platform layer).
    #[cfg(target_os = "macos")]
    pub fn focus_processes(self) -> &'static [&'static str] {
        match self {
            ProviderId::ClaudeCode => around_terminals!(["Claude"], ["Code"]),
            ProviderId::Codex => around_terminals!(["ChatGPT", "Codex"], ["Code"]),
            ProviderId::Gemini => around_terminals!([], ["Code"]),
            ProviderId::Cursor => &["Cursor"],
            ProviderId::Perplexity => &["Perplexity", "Comet"],
            ProviderId::Copilot => around_terminals!(["Code"], []),
            ProviderId::Grok
            | ProviderId::Glm
            | ProviderId::Kimi
            | ProviderId::OpenCode
            | ProviderId::CommandCode
            | ProviderId::MiniMax => around_terminals!([], ["Code"]),
            ProviderId::Ollama => &["Ollama"],
            ProviderId::LmStudio => &["LM Studio"],
        }
    }
}

/// The key for one ring: a provider, and which of its accounts.
///
/// `claudeCode` for the only account or the default one, `claudeCode:work`
/// for the rest. Colons don't appear in provider keys or in the directory
/// suffixes accounts are named after, so the two halves always come apart
/// again.
pub fn instance_key(id: ProviderId, account: Option<&str>) -> String {
    match account {
        Some(account) => format!("{}:{account}", id.key()),
        None => id.key().to_string(),
    }
}

/// The provider half of an instance key, for anything that needs the tool
/// rather than the account — the icon, the poll cadence, the click target.
pub fn provider_of(key: &str) -> Option<ProviderId> {
    ProviderId::from_key(key.split(':').next().unwrap_or(key))
}

/// Whether the provider's numbers can be trusted right now.
///
/// Ordering matters: [`Health::worst_of`] keeps the most alarming state so the
/// collapsed pill can summarise every provider in a single ring colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    /// Provider isn't installed / not running. Not an error, just nothing to show.
    Unavailable,
    /// Fresh numbers straight from the source.
    Ok,
    /// We have numbers, but they're older than the provider's staleness budget.
    Stale,
    /// Upstream told us to slow down; we're backing off and showing the last value.
    RateLimited,
    /// We found the provider but no usable credentials.
    NeedsAuth,
    /// Something broke. `detail` carries the reason.
    Error,
}

impl Health {
    pub fn worst_of(a: Health, b: Health) -> Health {
        a.max(b)
    }

    /// True when the provider has nothing meaningful to contribute to the HUD.
    pub fn is_quiet(self) -> bool {
        matches!(self, Health::Unavailable)
    }
}

/// What the provider's agent is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Activity {
    /// Nothing running.
    #[default]
    Idle,
    /// A session finished recently and the user probably hasn't looked yet.
    Done,
    /// A model is producing tokens. Drives the spinning cyan arc.
    Generating,
    /// A CLI is blocked on a `[y/N]` style prompt. Drives the pulsing amber ring.
    AwaitingInput,
}

impl Activity {
    /// Precedence for rolling many sessions up into one indicator: a single
    /// blocked session outranks any amount of happily generating ones, because
    /// that is the state that actually needs the user.
    pub fn rank(self) -> u8 {
        match self {
            Activity::Idle => 0,
            Activity::Done => 1,
            Activity::Generating => 2,
            Activity::AwaitingInput => 3,
        }
    }

    pub fn merge(self, other: Activity) -> Activity {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// What a usage window is counting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageUnit {
    #[default]
    Percent,
    Tokens,
    Requests,
    Credits,
    Bytes,
}

/// One rolling limit window, e.g. Claude's 5-hour session window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    /// Stable identifier, e.g. `five_hour`.
    pub key: String,
    /// Short human label, e.g. `5h`.
    pub label: String,
    /// 0..=100. `None` when the provider exposes counts but no denominator.
    pub used_pct: Option<f32>,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub unit: UsageUnit,
    pub resets_at: Option<DateTime<Utc>>,
    /// Set when this window is an estimate rather than a reported figure, so the
    /// UI can mark it with a `~`.
    #[serde(default)]
    pub estimated: bool,
    /// Set when the percentage is context rather than a quota being consumed.
    ///
    /// Ollama's GPU-residency share is the motivating case: 93% on the GPU is
    /// *good*, and colouring it like a limit about to be hit would be a lie.
    /// Informational windows are excluded from the provider's peak and never
    /// raise a threshold alert.
    #[serde(default)]
    pub informational: bool,
    /// Set on a provider's weekly (7-day) quota, so the UI can pick it out
    /// without guessing from the label.
    #[serde(default)]
    pub weekly: bool,
    /// How long the window runs. Needed to know when it began, which is what
    /// the token ledger is asked about.
    #[serde(default)]
    pub window_minutes: Option<u32>,
    /// Input-equivalent tokens spent inside this window, and what the window
    /// is estimated to hold in full. Both are worked out locally (see
    /// `crate::estimate`), so they are approximations, marked as such.
    #[serde(default)]
    pub used_tokens: Option<f64>,
    #[serde(default)]
    pub capacity_tokens: Option<f64>,
    /// The same spending told apart: tokens new to the model this window,
    /// tokens it generated, the conversation re-read from cache, and what
    /// buying the lot at list price would have cost.
    ///
    /// Three figures rather than two because output runs five to eight times
    /// the price of input, and because cache reads are around 98% of the raw
    /// count — the whole conversation, re-read every turn. Folded together
    /// they would drown everything else and mean nothing.
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_tokens: Option<u64>,
    #[serde(default)]
    pub usd: Option<f64>,
}

impl UsageWindow {
    pub fn new(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            used_pct: None,
            used: None,
            limit: None,
            unit: UsageUnit::Percent,
            resets_at: None,
            estimated: false,
            informational: false,
            weekly: false,
            window_minutes: None,
            used_tokens: None,
            capacity_tokens: None,
            input_tokens: None,
            output_tokens: None,
            cache_tokens: None,
            usd: None,
        }
    }

    /// Say how long the window runs (see [`Self::window_minutes`]).
    pub fn lasting(mut self, minutes: u32) -> Self {
        self.window_minutes = Some(minutes);
        self
    }

    /// When this window began, if it says when it ends and how long it runs.
    pub fn started_at(&self) -> Option<DateTime<Utc>> {
        let minutes = self.window_minutes?;
        Some(self.resets_at? - chrono::Duration::minutes(minutes as i64))
    }

    /// Mark this window as the provider's weekly quota. See [`Self::weekly`].
    pub fn weekly(mut self, weekly: bool) -> Self {
        self.weekly = weekly;
        self
    }

    pub fn with_pct(mut self, pct: f32) -> Self {
        self.used_pct = Some(pct.clamp(0.0, 100.0));
        self
    }

    pub fn with_counts(mut self, used: f64, limit: Option<f64>) -> Self {
        self.used = Some(used);
        self.limit = limit;
        if let Some(limit) = limit.filter(|l| *l > 0.0) {
            self.used_pct = Some(((used / limit) * 100.0).clamp(0.0, 100.0) as f32);
        }
        self
    }

    pub fn with_unit(mut self, unit: UsageUnit) -> Self {
        self.unit = unit;
        self
    }

    pub fn with_reset(mut self, at: Option<DateTime<Utc>>) -> Self {
        self.resets_at = at;
        self
    }

    pub fn estimated(mut self) -> Self {
        self.estimated = true;
        self
    }

    /// Mark this window as context, not a quota. See [`Self::informational`].
    pub fn informational(mut self) -> Self {
        self.informational = true;
        self
    }
}

/// A single agent session (a CLI run, a Cursor composer tab, a loaded model).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    /// Usually the project folder name, which is what the user recognises.
    pub title: String,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub activity: Activity,
    pub last_activity: Option<DateTime<Utc>>,
    /// Tokens consumed by this session, when the provider exposes them.
    pub tokens: Option<u64>,
    /// Free-form line shown under the title, e.g. `8.2 GB VRAM`.
    pub detail: Option<String>,
    /// Where the session runs, as its transcript records it: Claude Code's
    /// `entrypoint` (`claude-desktop`, `claude-vscode`, `cli`) or Codex's
    /// `originator` (`Codex Desktop`, `codex_vscode`, `codex_exec`). Picks the
    /// app a click on the session brings to the front; see [`host_processes`].
    #[serde(default)]
    pub host: Option<String>,
}

impl Session {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            cwd: None,
            model: None,
            activity: Activity::Idle,
            last_activity: None,
            tokens: None,
            detail: None,
            host: None,
        }
    }
}

/// Terminals a CLI session may be running in, most specific first.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const TERMINALS: &[&str] = &[
    "WindowsTerminal.exe",
    "wezterm-gui.exe",
    "alacritty.exe",
    "pwsh.exe",
    "powershell.exe",
    "cmd.exe",
];
#[cfg(any(target_os = "linux", target_os = "macos"))]
const TERMINALS: &[&str] = around_terminals!([], []);

/// VS Code and the editors built on it, which all run the same extensions.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const VSCODE_FAMILY: &[&str] = &[
    "Code.exe",
    "Code - Insiders.exe",
    "Cursor.exe",
    "Windsurf.exe",
    "VSCodium.exe",
];
#[cfg(target_os = "linux")]
const VSCODE_FAMILY: &[&str] = &["code", "code-insiders", "cursor", "windsurf", "codium"];
#[cfg(target_os = "macos")]
const VSCODE_FAMILY: &[&str] = &[
    "Code",
    "Visual Studio Code",
    "Code - Insiders",
    "Cursor",
    "Windsurf",
    "VSCodium",
];

/// The Claude and Codex desktop apps. Neither ships for Linux; the community
/// build of Claude's goes by `claude-desktop`.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const CLAUDE_APP: &[&str] = &["claude.exe"];
#[cfg(target_os = "linux")]
const CLAUDE_APP: &[&str] = &["claude-desktop", "claude"];
#[cfg(target_os = "macos")]
const CLAUDE_APP: &[&str] = &["Claude"];
// The Codex app ships as the ChatGPT executable.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const CODEX_APP: &[&str] = &["ChatGPT.exe", "Codex.exe"];
#[cfg(target_os = "linux")]
const CODEX_APP: &[&str] = &["chatgpt", "codex"];
#[cfg(target_os = "macos")]
const CODEX_APP: &[&str] = &["ChatGPT", "Codex"];

/// The executables that host a session started from `host` (see
/// [`Session::host`]), or `None` for an origin we don't recognise, in which
/// case the provider's general list applies.
pub fn host_processes(host: &str) -> Option<&'static [&'static str]> {
    match host {
        "claude-desktop" => Some(CLAUDE_APP),
        "Codex Desktop" => Some(CODEX_APP),
        "claude-vscode" | "codex_vscode" => Some(VSCODE_FAMILY),
        "cli" | "sdk-cli" | "codex_cli_rs" | "codex_exec" => Some(TERMINALS),
        _ => None,
    }
}

/// The Store (MSIX) app a desktop host is installed as, by its Application
/// User Model ID, so it can be *opened* when no window of it is running. The
/// suffix is derived from the publisher's certificate, so it is the same on
/// every machine with the Store build. Windows only.
pub fn host_app_id(host: &str) -> Option<&'static str> {
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        return None;
    }
    match host {
        "claude-desktop" => Some("Claude_pzs8sxrjxfjjc!Claude"),
        "Codex Desktop" => Some("OpenAI.Codex_2p2nqsd0c76g0!App"),
        _ => None,
    }
}

/// What a subscription has returned: list price of the tokens it has been
/// used for, against what it costs.
///
/// The comparison is made over whatever stretch the token ledger actually
/// covers rather than a calendar month, and the plan's fee is prorated to the
/// same stretch — otherwise a plan bought yesterday would look like a terrible
/// deal for four weeks and a wonderful one afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanValue {
    /// List price of the tokens spent over the covered stretch, in USD.
    pub usd: f64,
    /// What the plan costs over that same stretch, in USD.
    pub plan_usd: f64,
    /// How many days of history that rests on.
    pub covered_days: f64,
    /// Tokens new to the model, and tokens it generated, over the stretch.
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Everything the HUD knows about one provider at one point in time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSnapshot {
    /// What this ring *is* — `claudeCode`, `codex`. One per tool.
    pub id: ProviderId,
    /// What this ring *is for* — `claudeCode`, or `claudeCode:work` when the
    /// tool is signed into more than one account.
    ///
    /// Everything kept per ring is keyed by this rather than by [`Self::id`]:
    /// settings rows, ring colour, mute, order, the token ledger. A tool with
    /// one account produces the plain provider key, so nothing changes for
    /// anyone who has one.
    #[serde(default)]
    pub key: String,
    /// The account's own name, when it has one: `work` for `~/.claude-work`.
    #[serde(default)]
    pub account_key: Option<String>,
    pub name: String,
    pub health: Health,
    pub activity: Activity,
    /// Human explanation of a non-`Ok` health, shown in the expanded card.
    pub detail: Option<String>,
    /// Where the numbers came from, e.g. `oauth`, `transcripts`, `state.vscdb`.
    pub source: Option<String>,
    /// Plan / account label, e.g. `Max 20x`.
    pub account: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub sessions: Vec<Session>,
    pub updated_at: DateTime<Utc>,
    /// When a rate limit is in force, when we will next try.
    pub retry_at: Option<DateTime<Utc>>,
    /// What the plan has returned so far — see [`PlanValue`]. Absent unless
    /// the estimate is switched on and a price has been set for the plan.
    #[serde(default)]
    pub value: Option<PlanValue>,
}

impl ProviderSnapshot {
    pub fn new(id: ProviderId) -> Self {
        Self {
            id,
            key: id.key().to_string(),
            account_key: None,
            name: id.display_name().to_string(),
            health: Health::Ok,
            activity: Activity::Idle,
            detail: None,
            source: None,
            account: None,
            windows: Vec::new(),
            sessions: Vec::new(),
            updated_at: Utc::now(),
            retry_at: None,
            value: None,
        }
    }

    /// Mark this snapshot as one account of a tool that has several.
    ///
    /// The account's name joins the card's title, so two rings for the same
    /// tool can be told apart without hovering both.
    pub fn for_account(mut self, account_key: Option<String>) -> Self {
        self.key = instance_key(self.id, account_key.as_deref());
        if let Some(name) = &account_key {
            self.name = account_name(self.id, name);
        }
        self.account_key = account_key;
        self
    }

    /// A provider that produced no numbers, with the reason attached.
    pub fn degraded(id: ProviderId, health: Health, detail: impl Into<String>) -> Self {
        Self {
            health,
            detail: Some(detail.into()),
            ..Self::new(id)
        }
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_account(mut self, account: Option<String>) -> Self {
        self.account = account;
        self
    }

    pub fn with_windows(mut self, windows: Vec<UsageWindow>) -> Self {
        self.windows = windows;
        self
    }

    pub fn with_sessions(mut self, sessions: Vec<Session>) -> Self {
        self.activity = sessions
            .iter()
            .fold(self.activity, |acc, s| acc.merge(s.activity));
        self.sessions = sessions;
        self
    }

    /// Highest utilisation across this provider's *quota* windows.
    pub fn peak_pct(&self) -> Option<f32> {
        self.windows
            .iter()
            .filter(|w| !w.informational)
            .filter_map(|w| w.used_pct)
            .fold(None, |acc: Option<f32>, pct| {
                Some(acc.map_or(pct, |a| a.max(pct)))
            })
    }

    /// Mark a previously-good snapshot as stale rather than dropping it, so the
    /// HUD keeps showing the last known numbers with a visible caveat.
    pub fn mark_stale(&mut self, reason: impl Into<String>) {
        if self.health == Health::Ok {
            self.health = Health::Stale;
        }
        self.detail = Some(reason.into());
        // Live activity can't be trusted once collection stops.
        self.activity = Activity::Idle;
        for session in &mut self.sessions {
            session.activity = Activity::Idle;
        }
    }
}

/// The full HUD payload pushed to the webview on every poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Telemetry {
    pub providers: Vec<ProviderSnapshot>,
    pub generated_at: DateTime<Utc>,
    /// Worst utilisation across every provider, for the collapsed pill.
    pub peak_pct: Option<f32>,
    /// Rolled-up activity across every provider.
    pub activity: Activity,
    /// Worst health across the providers that have anything to say.
    pub health: Health,
    /// Rings (by key) that just started asking for attention while their
    /// agent's window was already the one in front: the news is on screen, so
    /// nothing announces it. Filled in by the app as it publishes, since
    /// seeing windows is its job, not the collector's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub in_front: Vec<String>,
    /// Every extra account found on this machine, switched on or off. Only
    /// the ones switched on have a ring; settings lists them all, or one
    /// switched off could never be switched back on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<ExtraAccount>,
}

/// An account of a tool beyond its default one: `~/.claude-work`, say.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraAccount {
    /// The ring's key: `claudeCode:work`.
    pub key: String,
    pub id: ProviderId,
    /// As its ring's card is titled: `Claude Code · work`.
    pub name: String,
}

impl ExtraAccount {
    pub fn new(id: ProviderId, account: &str) -> Self {
        Self {
            key: instance_key(id, Some(account)),
            id,
            name: account_name(id, account),
        }
    }
}

/// How an extra account is titled: the tool, then the account's own name.
pub fn account_name(id: ProviderId, account: &str) -> String {
    format!("{} · {account}", id.display_name())
}

impl Telemetry {
    pub fn from_snapshots(mut providers: Vec<ProviderSnapshot>) -> Self {
        providers.sort_by_key(|p| p.id);

        let peak_pct = providers
            .iter()
            .filter(|p| p.health != Health::Unavailable)
            .filter_map(|p| p.peak_pct())
            .fold(None, |acc: Option<f32>, pct| {
                Some(acc.map_or(pct, |a| a.max(pct)))
            });

        let activity = providers
            .iter()
            .fold(Activity::Idle, |acc, p| acc.merge(p.activity));

        let health = providers
            .iter()
            .filter(|p| !p.health.is_quiet())
            .fold(Health::Ok, |acc, p| Health::worst_of(acc, p.health));

        Self {
            providers,
            generated_at: Utc::now(),
            peak_pct,
            activity,
            health,
            in_front: Vec::new(),
            accounts: Vec::new(),
        }
    }

    pub fn empty() -> Self {
        Self {
            providers: Vec::new(),
            generated_at: Utc::now(),
            peak_pct: None,
            activity: Activity::Idle,
            health: Health::Ok,
            in_front: Vec::new(),
            accounts: Vec::new(),
        }
    }

    pub fn get(&self, id: ProviderId) -> Option<&ProviderSnapshot> {
        self.providers.iter().find(|p| p.id == id)
    }
}

/// Thresholds we alert on, matching the macOS app's 80% / 100% notifications.
pub const ALERT_THRESHOLDS: [f32; 2] = [80.0, 100.0];

/// Tracks which thresholds we've already fired for, so crossing 80% notifies
/// once rather than on every poll. Reset when a window's `resets_at` passes.
#[derive(Debug, Default)]
pub struct AlertLedger {
    /// Keyed by ring rather than by tool, so two accounts of the same tool
    /// crossing 80% are two crossings.
    fired: BTreeMap<(String, String), f32>,
}

impl AlertLedger {
    /// Returns the threshold to alert on, if this reading just crossed one.
    pub fn observe(&mut self, instance: &str, window: &UsageWindow) -> Option<f32> {
        if window.informational {
            return None;
        }
        let pct = window.used_pct?;
        let key = (instance.to_string(), window.key.clone());
        let previous = self.fired.get(&key).copied().unwrap_or(0.0);

        // A drop of more than a few points means the window rolled over.
        if pct + 5.0 < previous {
            self.fired.remove(&key);
            return None;
        }

        // Highest threshold crossed by this reading wins, so a jump straight
        // past 80 to 100 reports 100 rather than nagging twice.
        let crossed = ALERT_THRESHOLDS
            .iter()
            .copied()
            .rfind(|t| pct >= *t && previous < *t)?;

        self.fired.insert(key, pct.max(crossed));
        Some(crossed)
    }
}

/// How long a snapshot may go without a refresh before it is called stale.
pub fn staleness_budget(id: ProviderId) -> Duration {
    match id {
        // The OAuth usage endpoint is polled slowly and is rate limited.
        ProviderId::ClaudeCode => Duration::minutes(15),
        ProviderId::Cursor
        | ProviderId::Copilot
        | ProviderId::Codex
        | ProviderId::Gemini
        | ProviderId::Perplexity => Duration::minutes(10),
        // Its billing period is a week; a reading holds up for a while.
        ProviderId::Grok
        | ProviderId::Glm
        | ProviderId::Kimi
        | ProviderId::OpenCode
        | ProviderId::CommandCode
        | ProviderId::MiniMax => Duration::minutes(20),
        // Local, cheap, and interesting only while they are live.
        ProviderId::Ollama | ProviderId::LmStudio => Duration::minutes(2),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extra_account_is_listed_as_its_ring_is_keyed_and_titled() {
        // Settings switches an account by the key its row carries, and the
        // collector switches rings by theirs: the two have to be one key.
        let ring = ProviderSnapshot::new(ProviderId::ClaudeCode).for_account(Some("work".into()));
        let listed = ExtraAccount::new(ProviderId::ClaudeCode, "work");
        assert_eq!(listed.key, ring.key);
        assert_eq!(listed.name, ring.name);
        assert_eq!(listed.key, "claudeCode:work");
    }

    #[test]
    fn activity_precedence_puts_blocked_sessions_first() {
        assert_eq!(
            Activity::Generating.merge(Activity::AwaitingInput),
            Activity::AwaitingInput
        );
        assert_eq!(
            Activity::AwaitingInput.merge(Activity::Generating),
            Activity::AwaitingInput
        );
        assert_eq!(Activity::Idle.merge(Activity::Done), Activity::Done);
        assert_eq!(Activity::Done.merge(Activity::Idle), Activity::Done);
    }

    #[test]
    fn health_worst_of_prefers_the_alarming_state() {
        assert_eq!(
            Health::worst_of(Health::Ok, Health::NeedsAuth),
            Health::NeedsAuth
        );
        assert_eq!(
            Health::worst_of(Health::Error, Health::Stale),
            Health::Error
        );
        assert_eq!(
            Health::worst_of(Health::Ok, Health::Unavailable),
            Health::Ok
        );
    }

    #[test]
    fn usage_window_derives_percentage_from_counts() {
        let w = UsageWindow::new("five_hour", "5h").with_counts(25.0, Some(100.0));
        assert_eq!(w.used_pct, Some(25.0));

        // No denominator means no invented percentage.
        let w = UsageWindow::new("tokens", "Tokens").with_counts(1234.0, None);
        assert_eq!(w.used_pct, None);

        // Over-limit readings clamp instead of overflowing the ring.
        let w = UsageWindow::new("x", "x").with_counts(150.0, Some(100.0));
        assert_eq!(w.used_pct, Some(100.0));
    }

    #[test]
    fn telemetry_rolls_up_peak_activity_and_health() {
        let a = ProviderSnapshot::new(ProviderId::ClaudeCode).with_windows(vec![UsageWindow::new(
            "five_hour",
            "5h",
        )
        .with_pct(42.0)]);
        let mut b = ProviderSnapshot::new(ProviderId::Cursor)
            .with_windows(vec![UsageWindow::new("month", "Mo").with_pct(88.0)]);
        b.activity = Activity::Generating;
        let c = ProviderSnapshot::degraded(ProviderId::Codex, Health::NeedsAuth, "no token");

        let t = Telemetry::from_snapshots(vec![b, c, a]);
        assert_eq!(t.peak_pct, Some(88.0));
        assert_eq!(t.activity, Activity::Generating);
        assert_eq!(t.health, Health::NeedsAuth);
        // Sorted into a stable display order regardless of collection order.
        assert_eq!(t.providers[0].id, ProviderId::ClaudeCode);
    }

    #[test]
    fn informational_windows_are_context_not_quota() {
        let snap = ProviderSnapshot::new(ProviderId::Ollama).with_windows(vec![
            UsageWindow::new("vram", "On GPU")
                .with_pct(93.0)
                .informational(),
            UsageWindow::new("quota", "Quota").with_pct(10.0),
        ]);
        assert_eq!(
            snap.peak_pct(),
            Some(10.0),
            "a 93%-on-GPU reading must not present as a nearly-exhausted limit"
        );

        let mut ledger = AlertLedger::default();
        let gpu = UsageWindow::new("vram", "On GPU")
            .with_pct(100.0)
            .informational();
        assert_eq!(ledger.observe(ProviderId::Ollama.key(), &gpu), None);
    }

    #[test]
    fn unavailable_providers_do_not_drag_down_overall_health() {
        let ok = ProviderSnapshot::new(ProviderId::ClaudeCode);
        let missing =
            ProviderSnapshot::degraded(ProviderId::Ollama, Health::Unavailable, "not running");
        let t = Telemetry::from_snapshots(vec![ok, missing]);
        assert_eq!(t.health, Health::Ok);
    }

    #[test]
    fn alert_ledger_fires_once_per_threshold_and_resets_on_rollover() {
        let mut ledger = AlertLedger::default();
        let win = |pct: f32| UsageWindow::new("five_hour", "5h").with_pct(pct);

        assert_eq!(
            ledger.observe(ProviderId::ClaudeCode.key(), &win(50.0)),
            None
        );
        assert_eq!(
            ledger.observe(ProviderId::ClaudeCode.key(), &win(81.0)),
            Some(80.0)
        );
        // Still above 80 but already alerted.
        assert_eq!(
            ledger.observe(ProviderId::ClaudeCode.key(), &win(85.0)),
            None
        );
        assert_eq!(
            ledger.observe(ProviderId::ClaudeCode.key(), &win(100.0)),
            Some(100.0)
        );
        // Window rolls over -> ledger clears and can alert again.
        assert_eq!(
            ledger.observe(ProviderId::ClaudeCode.key(), &win(3.0)),
            None
        );
        assert_eq!(
            ledger.observe(ProviderId::ClaudeCode.key(), &win(80.0)),
            Some(80.0)
        );
    }

    #[test]
    fn jumping_straight_past_both_thresholds_reports_the_higher_one() {
        let mut ledger = AlertLedger::default();
        let win = UsageWindow::new("week", "7d").with_pct(100.0);
        assert_eq!(ledger.observe(ProviderId::Cursor.key(), &win), Some(100.0));
    }

    #[test]
    fn marking_stale_keeps_numbers_but_clears_live_activity() {
        let mut snap = ProviderSnapshot::new(ProviderId::ClaudeCode)
            .with_windows(vec![UsageWindow::new("five_hour", "5h").with_pct(60.0)])
            .with_sessions(vec![Session {
                activity: Activity::Generating,
                ..Session::new("s1", "api")
            }]);
        assert_eq!(snap.activity, Activity::Generating);

        snap.mark_stale("collector timed out");
        assert_eq!(snap.health, Health::Stale);
        assert_eq!(snap.activity, Activity::Idle);
        assert_eq!(snap.sessions[0].activity, Activity::Idle);
        assert_eq!(snap.peak_pct(), Some(60.0));
    }
}
