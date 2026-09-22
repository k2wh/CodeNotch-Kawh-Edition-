//! On-disk settings, stored at `%APPDATA%\CodeNotch\config.json`.
//!
//! Every field has a serde default so a hand-edited or older config file still
//! loads; unknown keys are ignored rather than rejected.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::ProviderId;

/// Which screen edge the notch is pinned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Top,
    Bottom,
    Left,
    #[default]
    Right,
}

impl Edge {
    pub fn is_horizontal(self) -> bool {
        matches!(self, Edge::Top | Edge::Bottom)
    }
}

/// Overall HUD scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HudSize {
    Small,
    #[default]
    Medium,
    Large,
}

/// Logical (DPI-independent) dimensions of the notch.
///
/// The notch is a strip docked against one screen edge holding one ring per
/// provider, plus a detail popover that appears alongside it on hover. "Along"
/// is down the strip on a vertical edge, across it on a horizontal one;
/// "thickness" is the other axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HudMetrics {
    /// How far the strip reaches into the screen.
    pub strip_thickness: f64,
    /// Space one provider occupies along the strip: ring, percentage, gap.
    ///
    /// Measured off the reference art at 1.47x the strip's thickness.
    pub slot: f64,
    /// Padding at each end of the strip, before the first slot.
    ///
    /// Measured at 0.53x the thickness. This is *not* the taper length: the
    /// silhouette's curve runs 1.21x the thickness and so reaches into the
    /// first slot, exactly as the reference does — a ring is inset far enough
    /// from the strip's sides that the last percent of the taper never clips
    /// it.
    pub strip_padding: f64,
    /// Diameter of a provider's ring.
    pub ring: f64,
    /// Size of the detail popover on the axis it extends along.
    pub popover_size: f64,
    /// Gap between the popover and the strip.
    ///
    /// 0.54x the thickness, because the card's tail reaches 0.4x across it and
    /// the reference leaves the rest as clear air.
    pub popover_gap: f64,
}

impl HudMetrics {
    /// How deep a provider's column is: the ring, the percentage under it and
    /// the weekly bar under that, plus air around them.
    ///
    /// On a left/right edge that column runs *along* the strip and a slot
    /// holds it. On a top/bottom edge it runs *across* the strip instead, so
    /// the strip has to be at least this deep or the number and the weekly bar
    /// spill out of the black silhouette. The ratios mirror the type sizes the
    /// ring itself uses: 0.355x the diameter for the percentage, 0.17x for the
    /// weekly label.
    pub fn stack_depth(&self) -> f64 {
        self.ring * (1.0 + 0.355 + 0.17) + 29.0
    }

    /// Resting size of the strip holding `providers` rings, as
    /// (along the edge, into the screen).
    pub fn strip_extent(&self, providers: usize) -> (f64, f64) {
        // Never fewer than one slot: before the first poll there are no
        // providers, and a zero-height strip would simply vanish.
        let along = self.strip_padding * 2.0 + providers.max(1) as f64 * self.slot;
        (along, self.strip_thickness)
    }
}

impl HudSize {
    pub fn metrics(self) -> HudMetrics {
        match self {
            HudSize::Small => HudMetrics {
                strip_thickness: 78.0,
                slot: 115.0,
                strip_padding: 41.0,
                ring: 49.0,
                popover_size: 276.0,
                popover_gap: 42.0,
            },
            HudSize::Medium => HudMetrics {
                strip_thickness: 92.0,
                slot: 135.0,
                strip_padding: 48.0,
                ring: 58.0,
                popover_size: 320.0,
                popover_gap: 50.0,
            },
            HudSize::Large => HudMetrics {
                strip_thickness: 108.0,
                slot: 159.0,
                strip_padding: 57.0,
                ring: 68.0,
                popover_size: 368.0,
                popover_gap: 58.0,
            },
        }
    }
}

/// The chimes on offer. The webview synthesises them (see `lib/sounds.ts`);
/// this list only keeps the stored setting to one the UI knows.
pub const NOTIFY_SOUNDS: &[&str] = &[
    "chime", "ping", "bell", "marimba", "arp", "drop", "glass", "pulse",
];

/// The same list, plus silence. Only the second chime may be turned off on
/// its own: something has to play when the master switch is on.
pub const NOTIFY_SOUNDS_OR_NONE: &[&str] = &[
    "chime", "ping", "bell", "marimba", "arp", "drop", "glass", "pulse", "none",
];

/// Something that doesn't sound like the finished chime, so the two can be
/// told apart from across a room.
fn default_waiting_sound() -> String {
    "ping".to_string()
}

/// MiniMax's international service, which is the one most accounts are on.
fn default_minimax_region() -> String {
    "international".to_string()
}

/// A falling tone, which is what running out sounds like.
fn default_limit_sound() -> String {
    "drop".to_string()
}

/// And a rising one for getting it back.
fn default_recover_sound() -> String {
    "arp".to_string()
}

/// Which monitor to pin to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "index")]
pub enum MonitorChoice {
    #[default]
    Primary,
    /// Zero-based index into the enumerated monitor list.
    Index(usize),
}

/// Poll cadence per provider, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PollConfig {
    /// Claude's usage endpoint is rate limited, so we are deliberately gentle:
    /// five minutes at rest, which is what the macOS original settled on. The
    /// ring still moves between polls — what an agent is *doing* is read from
    /// files on this machine every few seconds and costs nobody a request.
    pub claude_secs: u64,
    pub cursor_secs: u64,
    pub copilot_secs: u64,
    pub codex_secs: u64,
    pub gemini_secs: u64,
    pub perplexity_secs: u64,
    pub grok_secs: u64,
    pub glm_secs: u64,
    pub kimi_secs: u64,
    pub opencode_secs: u64,
    pub command_code_secs: u64,
    pub ollama_secs: u64,
    pub lm_studio_secs: u64,
    pub mini_max_secs: u64,
    /// Cheap local file-watch pass that drives the activity ring between polls.
    pub activity_secs: u64,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            claude_secs: 300,
            cursor_secs: 45,
            copilot_secs: 120,
            codex_secs: 45,
            gemini_secs: 60,
            perplexity_secs: 120,
            grok_secs: 300,
            glm_secs: 180,
            kimi_secs: 180,
            opencode_secs: 180,
            command_code_secs: 300,
            ollama_secs: 10,
            lm_studio_secs: 10,
            mini_max_secs: 180,
            activity_secs: 3,
        }
    }
}

impl PollConfig {
    pub fn for_provider(&self, id: ProviderId) -> u64 {
        let secs = match id {
            ProviderId::ClaudeCode => self.claude_secs,
            ProviderId::Cursor => self.cursor_secs,
            ProviderId::Copilot => self.copilot_secs,
            ProviderId::Codex => self.codex_secs,
            ProviderId::Gemini => self.gemini_secs,
            ProviderId::Perplexity => self.perplexity_secs,
            ProviderId::Grok => self.grok_secs,
            ProviderId::Glm => self.glm_secs,
            ProviderId::Kimi => self.kimi_secs,
            ProviderId::OpenCode => self.opencode_secs,
            ProviderId::CommandCode => self.command_code_secs,
            ProviderId::Ollama => self.ollama_secs,
            ProviderId::LmStudio => self.lm_studio_secs,
            ProviderId::MiniMax => self.mini_max_secs,
        };
        // Guard against a hand-edited config pinning a CPU core or hammering
        // the API.
        let secs = secs.clamp(5, 3600);
        match id {
            // Measured, not guessed: at 90 seconds Anthropic's usage endpoint
            // refuses roughly every other request, and the ring spends its
            // life flicking between a number and a 429. Five minutes is what
            // the macOS original settled on. A floor rather than a default,
            // because a default only reaches a config file that doesn't exist
            // yet — everyone already running has 90 written down.
            ProviderId::ClaudeCode => secs.max(300),
            _ => secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub edge: Edge,
    /// Position along the edge, 0.0 = left/top .. 1.0 = right/bottom.
    pub edge_offset: f32,
    /// Gap between the HUD and the screen's work area, in logical pixels.
    pub margin: f64,
    pub size: HudSize,
    /// Ring accent as `#rrggbb`.
    pub accent: String,
    pub monitor: MonitorChoice,

    /// Stay expanded instead of collapsing when the pointer leaves.
    pub always_expanded: bool,
    /// Hide the HUD entirely (tray icon stays).
    pub hidden: bool,
    /// Let clicks fall through to the window underneath while collapsed.
    pub click_through_when_collapsed: bool,
    /// Briefly expand when an agent finishes or starts waiting for input.
    pub peek_on_attention: bool,
    /// Seconds a peek stays open.
    pub peek_secs: u64,
    /// Desktop notification when a window crosses 80% / 100%.
    pub notify_on_thresholds: bool,
    /// Providers that announce nothing — no threshold notification, no chime —
    /// while still being polled and drawn, keyed by [`ProviderId::key`].
    ///
    /// Distinct from switching a provider off, which stops reading it
    /// altogether. This is for the one you want to watch but not hear from.
    #[serde(default)]
    pub muted_providers: Vec<String>,
    /// Chime when an agent answers or starts waiting on the user.
    pub notify_sound: bool,
    /// Pulse a halo around the ring for the same moments.
    pub notify_pulse: bool,
    /// Which chime when an agent finishes, one of [`NOTIFY_SOUNDS`].
    pub notify_sound_id: String,
    /// And which when a window runs out, one of [`NOTIFY_SOUNDS_OR_NONE`].
    ///
    /// Third because it is a third kind of news: not "come and look" but
    /// "stop planning to use this for a while".
    #[serde(default = "default_limit_sound")]
    pub notify_limit_sound_id: String,
    /// And which when it resets and the provider can be used again, one of
    /// [`NOTIFY_SOUNDS_OR_NONE`].
    #[serde(default = "default_recover_sound")]
    pub notify_recover_sound_id: String,
    /// And which when one stops to ask you something, one of
    /// [`NOTIFY_SOUNDS_OR_NONE`].
    ///
    /// A separate sound because it is separate news: finishing is over and
    /// can wait, being asked means the agent is blocked on you. Telling them
    /// apart without looking is the whole point of a sound.
    #[serde(default = "default_waiting_sound")]
    pub notify_waiting_sound_id: String,
    /// How loud the chime is, 0–100.
    pub notify_volume: u8,
    /// Show reset windows as a countdown rather than a clock time.
    pub reset_as_countdown: bool,
    /// Start CodeNotch when the user signs in (HKCU ...\Run).
    pub launch_at_login: bool,

    pub poll: PollConfig,
    /// Per-provider on/off, keyed by [`ProviderId::key`].
    pub providers: BTreeMap<String, bool>,
    /// Display order, keyed by [`ProviderId::key`]. Unlisted providers sort last.
    pub provider_order: Vec<String>,

    /// Where to reach Ollama. Kept configurable for remote/WSL daemons.
    pub ollama_url: String,

    /// Underline each ring with the provider's other window.
    pub show_weekly: bool,
    /// Swap the two: the weekly quota on the ring, the session limit on the
    /// line beneath it. Which one belongs in the bigger gauge depends on
    /// which one you are actually rationing.
    #[serde(default)]
    pub weekly_on_ring: bool,
    /// Work out what each window is worth in tokens, from what this machine
    /// has spent against the percentage the provider reports.
    pub estimate_tokens: bool,
    /// Which MiniMax to ask: `international` or `china`. They are separate
    /// services with separate accounts, and asking the wrong one gets a
    /// perfectly valid "not signed in".
    #[serde(default = "default_minimax_region")]
    pub minimax_region: String,
    /// Per-provider ring colour as `#rrggbb`, keyed by [`ProviderId::key`].
    /// Providers without an entry keep the traffic-light colours.
    pub ring_colors: BTreeMap<String, String>,
    /// What each plan costs its subscriber per month, in USD, keyed by
    /// [`ProviderId::key`] — a Max 20x is `200`. Set one and the card says
    /// what the plan has returned in tokens against what it costs; leave it
    /// out and that line stays off. Rides on `estimate_tokens`, which is what
    /// counts the tokens in the first place.
    #[serde(default)]
    pub plan_usd: BTreeMap<String, f64>,

    /// Drop behind a full-screen app in front (a game, a video, a slideshow)
    /// instead of floating over it.
    pub stay_below_fullscreen: bool,
    /// Executables (e.g. `chrome.exe`) the notch stays behind while they are
    /// the foreground app. Matched case-insensitively.
    pub stay_below_apps: Vec<String>,

    /// Interface language: `auto` (follow Windows), `en`, `pt` or `es`.
    pub language: String,

    /// New releases: `auto` looks for one and downloads it in the background,
    /// `notify` only says there is one, `off` never asks. Installing always
    /// waits for a click, whichever it is.
    #[serde(default = "default_updates")]
    pub updates: String,
}

/// Download in the background: installing still waits for the user.
fn default_updates() -> String {
    "auto".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            edge: Edge::Right,
            edge_offset: 0.5,
            margin: 0.0,
            size: HudSize::Medium,
            accent: "#22d3ee".to_string(),
            monitor: MonitorChoice::Primary,

            always_expanded: false,
            hidden: false,
            click_through_when_collapsed: true,
            peek_on_attention: true,
            peek_secs: 5,
            notify_on_thresholds: true,
            muted_providers: Vec::new(),
            notify_sound: true,
            notify_pulse: true,
            notify_sound_id: "chime".to_string(),
            notify_waiting_sound_id: default_waiting_sound(),
            notify_limit_sound_id: default_limit_sound(),
            notify_recover_sound_id: default_recover_sound(),
            notify_volume: 70,
            reset_as_countdown: true,
            launch_at_login: false,

            poll: PollConfig::default(),
            providers: ProviderId::ALL
                .iter()
                .map(|p| (p.key().to_string(), true))
                .collect(),
            provider_order: ProviderId::ALL
                .iter()
                .map(|p| p.key().to_string())
                .collect(),

            ollama_url: "http://127.0.0.1:11434".to_string(),

            show_weekly: true,
            weekly_on_ring: false,
            estimate_tokens: true,
            minimax_region: default_minimax_region(),
            ring_colors: BTreeMap::new(),
            plan_usd: BTreeMap::new(),

            // Opt-in: people keep the notch up to watch limits while gaming.
            stay_below_fullscreen: false,
            stay_below_apps: Vec::new(),

            language: "auto".to_string(),

            updates: default_updates(),
        }
    }
}

impl Config {
    /// Whether a ring is switched on, by instance key.
    ///
    /// A second account inherits the tool's own setting until it is given one
    /// of its own: turning Claude Code off turns off every account of it, and
    /// turning one account off afterwards leaves the others alone.
    pub fn is_enabled(&self, instance: &str) -> bool {
        if let Some(explicit) = self.providers.get(instance) {
            return *explicit;
        }
        if let Some((base, _)) = instance.split_once(':') {
            if let Some(inherited) = self.providers.get(base) {
                return *inherited;
            }
        }
        // Default to on: a provider added in a later version shouldn't be
        // invisible just because an old config file predates it.
        true
    }

    /// Whether this ring has been told to keep quiet — see
    /// [`Config::muted_providers`]. Inherited the same way as `is_enabled`.
    pub fn is_muted(&self, instance: &str) -> bool {
        if self.muted_providers.iter().any(|key| key == instance) {
            return true;
        }
        match instance.split_once(':') {
            Some((base, _)) => self.muted_providers.iter().any(|key| key == base),
            None => false,
        }
    }

    pub fn metrics(&self) -> HudMetrics {
        let mut metrics = self.size.metrics();
        if self.edge.is_horizontal() {
            // A ring's column stacks across a top/bottom strip, so the strip
            // has to be deep enough to hold all of it. See `stack_depth`.
            metrics.strip_thickness = metrics.stack_depth().max(metrics.strip_thickness);
        }
        metrics
    }

    /// Normalised accent, falling back to the default when the string is junk.
    pub fn accent_hex(&self) -> String {
        let s = self.accent.trim();
        let valid =
            s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit());
        if valid {
            s.to_lowercase()
        } else {
            "#22d3ee".to_string()
        }
    }

    pub fn path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("could not resolve %APPDATA% (config dir)")?
            .join("CodeNotch");
        Ok(dir.join("config.json"))
    }

    /// Load from disk, falling back to defaults when the file is missing.
    ///
    /// A corrupt file is *not* fatal: we log and carry on with defaults so the
    /// HUD still starts. The broken file is kept as `config.json.bad` first, so
    /// the next save doesn't silently wipe out a hand edit with a typo in it.
    pub fn load() -> Self {
        match Self::try_load() {
            Ok(Some(cfg)) => cfg,
            Ok(None) => Self::default(),
            Err(err) => {
                tracing::warn!(%err, "config unreadable, falling back to defaults");
                if let Ok(path) = Self::path() {
                    let _ = std::fs::copy(&path, path.with_extension("json.bad"));
                }
                Self::default()
            }
        }
    }

    fn try_load() -> Result<Option<Self>> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(cfg.sanitised()))
    }

    /// Clamp anything that could push the HUD off-screen or spin the poller.
    fn sanitised(mut self) -> Self {
        self.edge_offset = self.edge_offset.clamp(0.0, 1.0);
        self.margin = self.margin.clamp(0.0, 400.0);
        self.peek_secs = self.peek_secs.clamp(1, 60);
        self.accent = self.accent_hex();
        if self.ollama_url.trim().is_empty() {
            self.ollama_url = Config::default().ollama_url;
        }
        if !matches!(self.minimax_region.as_str(), "international" | "china") {
            self.minimax_region = default_minimax_region();
        }
        if !matches!(self.language.as_str(), "auto" | "en" | "pt" | "es") {
            self.language = "auto".to_string();
        }
        if !matches!(self.updates.as_str(), "auto" | "notify" | "off") {
            self.updates = default_updates();
        }
        if !NOTIFY_SOUNDS.contains(&self.notify_sound_id.as_str()) {
            self.notify_sound_id = Config::default().notify_sound_id;
        }
        if !NOTIFY_SOUNDS_OR_NONE.contains(&self.notify_waiting_sound_id.as_str()) {
            self.notify_waiting_sound_id = default_waiting_sound();
        }
        if !NOTIFY_SOUNDS_OR_NONE.contains(&self.notify_limit_sound_id.as_str()) {
            self.notify_limit_sound_id = default_limit_sound();
        }
        if !NOTIFY_SOUNDS_OR_NONE.contains(&self.notify_recover_sound_id.as_str()) {
            self.notify_recover_sound_id = default_recover_sound();
        }
        self.notify_volume = self.notify_volume.min(100);
        self.ring_colors.retain(|_, colour| {
            let c = colour.trim();
            c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit())
        });
        // A plan costs something, and not a fortune. Zero would divide, and a
        // typo with an extra digit would quietly make the card meaningless.
        self.plan_usd
            .retain(|_, usd| usd.is_finite() && *usd > 0.0 && *usd <= 100_000.0);
        self
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;

        // Write-then-rename so a crash mid-write can't leave a truncated config.
        // Each save gets its own temp name: overlapping saves (a dragged slider)
        // sharing one file would trip over each other's rename.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = path.with_extension(format!("json.{}.{seq}.tmp", std::process::id()));
        std::fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// Provider display order: configured keys first, then anything new.
    pub fn ordered_providers(&self) -> Vec<ProviderId> {
        let mut out: Vec<ProviderId> = self
            .provider_order
            .iter()
            .filter_map(|k| ProviderId::from_key(k))
            .collect();
        for id in ProviderId::ALL {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_files_fill_in_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"edge":"bottom","size":"large"}"#).unwrap();
        assert_eq!(cfg.edge, Edge::Bottom);
        assert_eq!(cfg.size, HudSize::Large);
        // Untouched fields keep their defaults.
        assert_eq!(cfg.accent, "#22d3ee");
        assert_eq!(cfg.poll.claude_secs, 300);
        assert!(cfg.peek_on_attention);
    }

    #[test]
    fn unknown_keys_are_ignored_rather_than_fatal() {
        let cfg: Config =
            serde_json::from_str(r#"{"edge":"left","somethingFromTheFuture":123}"#).unwrap();
        assert_eq!(cfg.edge, Edge::Left);
    }

    #[test]
    fn roundtrips_through_json() {
        let mut cfg = Config {
            edge: Edge::Right,
            always_expanded: true,
            ..Default::default()
        };
        cfg.providers.insert("cursor".into(), false);
        let back: Config = serde_json::from_str(&serde_json::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(cfg, back);
        assert!(!back.is_enabled(ProviderId::Cursor.key()));
        assert!(back.is_enabled(ProviderId::ClaudeCode.key()));
    }

    #[test]
    fn accent_falls_back_when_malformed() {
        let mut cfg = Config {
            accent: "not-a-colour".into(),
            ..Default::default()
        };
        assert_eq!(cfg.accent_hex(), "#22d3ee");
        cfg.accent = "#FF00AA".into();
        assert_eq!(cfg.accent_hex(), "#ff00aa");
        cfg.accent = "#f0a".into(); // shorthand isn't accepted
        assert_eq!(cfg.accent_hex(), "#22d3ee");
    }

    #[test]
    fn sanitise_clamps_out_of_range_values() {
        let cfg: Config =
            serde_json::from_str(r#"{"edgeOffset": 4.5, "margin": -20, "peekSecs": 9999}"#)
                .unwrap();
        let cfg = cfg.sanitised();
        assert_eq!(cfg.edge_offset, 1.0);
        assert_eq!(cfg.margin, 0.0);
        assert_eq!(cfg.peek_secs, 60);
    }

    #[test]
    fn updates_download_in_the_background_unless_told_otherwise() {
        // A config written before updates existed downloads them, like a new one.
        let old: Config = serde_json::from_str(r#"{"edge":"right"}"#).unwrap();
        assert_eq!(old.updates, "auto");
        assert_eq!(Config::default().updates, "auto");

        let off: Config = serde_json::from_str(r#"{"updates":"off"}"#).unwrap();
        assert_eq!(off.sanitised().updates, "off");
        let odd: Config = serde_json::from_str(r#"{"updates":"always"}"#).unwrap();
        assert_eq!(odd.sanitised().updates, "auto");
    }

    #[test]
    fn poll_interval_is_clamped_to_something_sane() {
        let poll = PollConfig {
            claude_secs: 0,
            cursor_secs: 0,
            ollama_secs: 100_000,
            ..PollConfig::default()
        };
        assert_eq!(poll.for_provider(ProviderId::Cursor), 5);
        assert_eq!(poll.for_provider(ProviderId::Ollama), 3600);
        // Claude's endpoint rate limits below five minutes, and a config file
        // written when the default was ninety seconds still says ninety.
        assert_eq!(poll.for_provider(ProviderId::ClaudeCode), 300);
        assert_eq!(
            PollConfig {
                claude_secs: 90,
                ..PollConfig::default()
            }
            .for_provider(ProviderId::ClaudeCode),
            300
        );
    }

    #[test]
    fn provider_order_appends_unknown_providers() {
        let cfg = Config {
            provider_order: vec!["ollama".into(), "bogus".into()],
            ..Default::default()
        };
        let order = cfg.ordered_providers();
        assert_eq!(order[0], ProviderId::Ollama);
        assert_eq!(order.len(), ProviderId::ALL.len());
    }

    #[test]
    fn strip_grows_by_one_slot_per_provider() {
        let m = HudSize::Medium.metrics();
        let (one, thickness) = m.strip_extent(1);
        let (three, _) = m.strip_extent(3);
        assert_eq!(three - one, m.slot * 2.0);
        assert_eq!(thickness, m.strip_thickness);
    }

    #[test]
    fn an_empty_strip_still_holds_one_slot() {
        // Before the first poll there are no providers; a zero-length strip
        // would disappear off screen entirely.
        let m = HudSize::Medium.metrics();
        assert_eq!(m.strip_extent(0), m.strip_extent(1));
    }

    #[test]
    fn every_size_keeps_the_reference_proportions() {
        // The silhouette is derived from the thickness, so everything laid out
        // against it has to scale with the thickness too -- otherwise the
        // small and large strips stop looking like the same object.
        for size in [HudSize::Small, HudSize::Medium, HudSize::Large] {
            let m = size.metrics();
            for (name, ratio, want) in [
                ("slot", m.slot / m.strip_thickness, 1.47),
                ("padding", m.strip_padding / m.strip_thickness, 0.53),
                ("ring", m.ring / m.strip_thickness, 0.63),
            ] {
                assert!(
                    (ratio - want).abs() < 0.02,
                    "{size:?} {name} is {ratio:.2}x thickness, expected ~{want:.2}x"
                );
            }
        }
    }

    #[test]
    fn every_size_keeps_the_ring_inside_its_slot() {
        for size in [HudSize::Small, HudSize::Medium, HudSize::Large] {
            let m = size.metrics();
            assert!(
                m.ring < m.strip_thickness,
                "{size:?} ring overflows the strip"
            );
            assert!(m.slot > m.ring, "{size:?} slot is smaller than its ring");
            assert!(
                m.popover_size > m.strip_thickness,
                "{size:?} popover too narrow"
            );
        }
    }
}
