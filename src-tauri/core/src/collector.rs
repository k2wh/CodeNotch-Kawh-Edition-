//! Polls the adapters on their own schedules and rolls the results into one
//! [`Telemetry`] payload for the HUD.
//!
//! Each provider has its own cadence (see [`PollConfig`](crate::config::PollConfig)),
//! because hitting a rate-limited HTTP endpoint as often as a local file read
//! would be a good way to get throttled. Between polls the last snapshot is
//! reused, and once it exceeds the provider's staleness budget it is marked
//! [`Health::Stale`] rather than quietly presented as current.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::adapters::StoredBackoff;
use crate::adapters::{claude, codex, home_dir};
use crate::adapters::{
    claude::ClaudeAdapter, codex::CodexAdapter, commandcode::CommandCodeAdapter,
    copilot::CopilotAdapter, cursor::CursorAdapter, gemini::GeminiAdapter, glm::GlmAdapter,
    grok::GrokAdapter, kimi::KimiAdapter, lmstudio::LmStudioAdapter, minimax::MiniMaxAdapter,
    ollama::OllamaAdapter, opencode::OpenCodeAdapter, perplexity::PerplexityAdapter,
};
use crate::config::Config;
use crate::estimate::Estimator;
use crate::ledger::{self, Ledger, StoredLedger, Weights};
use crate::model::{
    instance_key, provider_of, staleness_budget, Activity, AlertLedger, ExtraAccount, Health,
    PlanValue, ProviderId, ProviderSnapshot, Telemetry, UsageWindow,
};

/// How far back the plan's worth is judged, at most: a billing month.
const VALUE_DAYS: u32 = 30;

/// And how little history it may rest on. An hour of hard work against an
/// hour's share of a monthly fee is a ratio, but a noisy one; five minutes of
/// it is just noise.
const MIN_VALUE_DAYS: f64 = 1.0 / 24.0;

/// A usage threshold crossing worth notifying about.
#[derive(Debug, Clone, PartialEq)]
pub struct Alert {
    pub provider: ProviderId,
    pub provider_name: String,
    pub window_label: String,
    /// The threshold crossed (80 or 100).
    pub threshold: f32,
    pub used_pct: f32,
}

impl Alert {
    pub fn title(&self) -> String {
        if self.threshold >= 100.0 {
            format!("{} limit reached", self.provider_name)
        } else {
            format!("{} at {:.0}%", self.provider_name, self.used_pct)
        }
    }

    pub fn body(&self) -> String {
        format!("{} window · {:.0}% used", self.window_label, self.used_pct)
    }
}

/// When each provider was last collected, and what it said.
#[derive(Debug, Clone)]
struct CacheEntry {
    collected_at: DateTime<Utc>,
    snapshot: ProviderSnapshot,
}

/// Owns the adapters and their schedules.
pub struct Collector {
    config: Config,
    /// Shared by every adapter that talks to the network, and by the ones
    /// built later for a second account.
    http: reqwest::Client,
    /// One adapter per Claude Code account, built when that account is first
    /// polled and dropped when it goes away or is switched off. An adapter
    /// nobody reads is transcript offsets and fitted curves sitting in memory
    /// doing nothing.
    claude: BTreeMap<String, ClaudeAdapter>,
    /// The same for Codex, which has had profile directories all along.
    codex: BTreeMap<String, CodexAdapter>,
    cursor: CursorAdapter,
    copilot: CopilotAdapter,
    gemini: GeminiAdapter,
    perplexity: PerplexityAdapter,
    grok: GrokAdapter,
    glm: GlmAdapter,
    kimi: KimiAdapter,
    opencode: OpenCodeAdapter,
    command_code: CommandCodeAdapter,
    ollama: OllamaAdapter,
    lm_studio: LmStudioAdapter,
    minimax: MiniMaxAdapter,
    cache: BTreeMap<String, CacheEntry>,
    alerts: AlertLedger,
    /// Forces every provider to refresh on the next poll.
    refresh_all: bool,
    /// Last transcript write seen per provider, so the activity pass only
    /// re-reads files that actually changed.
    activity_marks: BTreeMap<String, std::time::SystemTime>,
    /// What a provider's own hooks last reported, and when. While these keep
    /// coming they outrank the transcripts: an agent saying "I've stopped"
    /// beats a file that was written a second ago.
    reported: BTreeMap<ProviderId, (Activity, DateTime<Utc>)>,
    /// Tokens spent, per provider, so a percentage can be turned into an
    /// amount. See [`crate::ledger`] and [`crate::estimate`].
    ledgers: BTreeMap<String, Ledger>,
    /// The last percentage a provider actually vouched for, per window, and
    /// when. What the ring shows is projected from here while a reading is
    /// stale — see [`Collector::account`].
    vouched: BTreeMap<(String, String), (DateTime<Utc>, f32)>,
    estimator: Estimator,
    /// Last time the ledgers and what they taught us were written out.
    saved_at: Option<DateTime<Utc>>,
    /// Rate-limit penalties from the previous run, until the adapter that
    /// owns each one is built and takes it back.
    penalties: BTreeMap<String, StoredBackoff>,
}

/// How often the ledger and estimates are written to disk. Often enough that
/// a crash costs little, rarely enough to be free.
const SAVE_EVERY_MINS: i64 = 2;

/// Read a state file, or nothing at all if it is missing or unreadable. State
/// is an optimisation, never a requirement: a bad file is replaced, not fatal.
fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Write a state file, best effort.
fn write_json<T: serde::Serialize>(path: &std::path::Path, value: &T) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(value) {
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// How long a hook's word stands before the transcripts take over again.
/// Long enough to cover a quiet spell, short enough that hooks being
/// uninstalled or broken doesn't freeze a ring.
const REPORTED_FOR_MINS: i64 = 10;

impl Collector {
    pub fn new(config: Config) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("CodeNotch/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();

        let mut collector = Self {
            http: http.clone(),
            claude: BTreeMap::new(),
            codex: BTreeMap::new(),
            cursor: CursorAdapter::new(),
            copilot: CopilotAdapter::new(),
            gemini: GeminiAdapter::new(),
            perplexity: PerplexityAdapter::new(),
            grok: GrokAdapter::new(http.clone()),
            glm: GlmAdapter::new(http.clone()),
            kimi: KimiAdapter::new(http.clone()),
            opencode: OpenCodeAdapter::new(http.clone()),
            command_code: CommandCodeAdapter::new(http.clone()),
            lm_studio: LmStudioAdapter::new(http.clone()),
            minimax: MiniMaxAdapter::new(
                http.clone(),
                crate::adapters::minimax::Region::from_setting(&config.minimax_region),
            ),
            ollama: OllamaAdapter::new(http, config.ollama_url.clone()),
            cache: BTreeMap::new(),
            alerts: AlertLedger::default(),
            refresh_all: true,
            activity_marks: BTreeMap::new(),
            reported: BTreeMap::new(),
            ledgers: BTreeMap::new(),
            vouched: BTreeMap::new(),
            estimator: Estimator::default(),
            saved_at: None,
            penalties: BTreeMap::new(),
            config,
        };
        collector.load_state(Utc::now());
        collector
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Swap in new settings; the next poll refreshes everything so the change
    /// is visible immediately rather than at the end of the slowest interval.
    pub fn set_config(&mut self, config: Config) {
        self.ollama.set_base_url(config.ollama_url.clone());
        self.minimax
            .set_region(crate::adapters::minimax::Region::from_setting(
                &config.minimax_region,
            ));
        self.config = config;
        self.refresh_all = true;

        // Drop anything the user just turned off so it can't linger on screen.
        self.cache
            .retain(|instance, _| self.config.is_enabled(instance));
    }

    /// Force a full refresh on the next poll.
    pub fn invalidate(&mut self) {
        self.refresh_all = true;
    }

    /// Whether this ring is due for collection.
    fn is_due(&self, instance: &str, id: ProviderId, now: DateTime<Utc>) -> bool {
        if self.refresh_all {
            return true;
        }
        match self.cache.get(instance) {
            None => true,
            Some(entry) => {
                let interval = chrono::Duration::seconds(self.config.poll.for_provider(id) as i64);
                now.signed_duration_since(entry.collected_at) >= interval
            }
        }
    }

    /// Every ring to draw: a provider, and which of its accounts.
    ///
    /// Claude Code and Codex keep one account per config directory, so the
    /// directories beside the default one are the other accounts and each
    /// earns a ring. Every other provider has exactly one.
    fn instances(&self) -> Vec<(String, ProviderId, Option<String>)> {
        let mut out = Vec::new();
        for id in self.config.ordered_providers() {
            match id {
                ProviderId::ClaudeCode => {
                    let accounts = claude::accounts();
                    if accounts.is_empty() {
                        out.push((id.key().to_string(), id, None));
                    }
                    for (account, _) in accounts {
                        out.push((instance_key(id, account.as_deref()), id, account));
                    }
                }
                ProviderId::Codex => {
                    let profiles = home_dir()
                        .map(|home| codex::discover_profiles(&home))
                        .unwrap_or_default();
                    if profiles.is_empty() {
                        out.push((id.key().to_string(), id, None));
                    }
                    for profile in profiles {
                        out.push((instance_key(id, profile.name.as_deref()), id, profile.name));
                    }
                }
                _ => out.push((id.key().to_string(), id, None)),
            }
        }
        out
    }

    /// Collect one ring, building its adapter the first time it is asked for.
    async fn collect_one(
        &mut self,
        instance: &str,
        id: ProviderId,
        account: Option<&str>,
        now: DateTime<Utc>,
    ) -> ProviderSnapshot {
        let snapshot = match id {
            ProviderId::ClaudeCode => {
                let adapter = match self.claude.get_mut(instance) {
                    Some(adapter) => adapter,
                    None => {
                        let paths = claude::accounts()
                            .into_iter()
                            .find(|(name, _)| name.as_deref() == account)
                            .map(|(_, paths)| paths);
                        let mut adapter = match paths {
                            Some(paths) => ClaudeAdapter::with_paths(self.http.clone(), paths),
                            None => ClaudeAdapter::new(self.http.clone()),
                        };
                        if account.is_some() {
                            adapter = adapter.own_credentials_only();
                        }
                        // Pick the penalty back up, so a relaunch during one
                        // waits it out instead of spending an attempt finding
                        // out it is still there.
                        if let Some(stored) = self.penalties.get(instance) {
                            adapter.restore_backoff(stored.clone(), now);
                        }
                        self.claude.entry(instance.to_string()).or_insert(adapter)
                    }
                };
                adapter.collect(now).await
            }
            ProviderId::Codex => {
                let adapter = match self.codex.get_mut(instance) {
                    Some(adapter) => adapter,
                    None => {
                        let profile = home_dir()
                            .map(|home| codex::discover_profiles(&home))
                            .unwrap_or_default()
                            .into_iter()
                            .find(|profile| profile.name.as_deref() == account);
                        let adapter = match profile {
                            Some(profile) => CodexAdapter::for_profile(profile),
                            None => CodexAdapter::new(),
                        };
                        self.codex.entry(instance.to_string()).or_insert(adapter)
                    }
                };
                adapter.collect(now)
            }
            ProviderId::Cursor => self.cursor.collect(now),
            ProviderId::Copilot => self.copilot.collect(now),
            ProviderId::Gemini => self.gemini.collect(now),
            ProviderId::Perplexity => self.perplexity.collect(now),
            ProviderId::Grok => self.grok.collect(now).await,
            ProviderId::Glm => self.glm.collect(now).await,
            ProviderId::Kimi => self.kimi.collect(now).await,
            ProviderId::OpenCode => self.opencode.collect(now).await,
            ProviderId::CommandCode => self.command_code.collect(now).await,
            ProviderId::Ollama => self.ollama.collect(now).await,
            ProviderId::LmStudio => self.lm_studio.collect(now).await,
            ProviderId::MiniMax => self.minimax.collect(now).await,
        };
        snapshot.for_account(account.map(str::to_string))
    }

    /// Let go of the adapters for rings that are no longer drawn — an account
    /// signed out of, or a provider switched off.
    fn drop_unused(&mut self, live: &[String]) {
        self.claude.retain(|key, _| live.iter().any(|k| k == key));
        self.codex.retain(|key, _| live.iter().any(|k| k == key));
        self.ledgers.retain(|key, _| live.iter().any(|k| k == key));
    }

    /// Read back the ledgers and what past windows taught us.
    ///
    /// Without this, every restart forgets how big a limit is and has to
    /// watch a window fill again before it can say anything.
    fn load_state(&mut self, now: DateTime<Utc>) {
        // Figures written by an older counter are not merely stale, they are
        // wrong — it counted a repeated message once per line. When that is
        // what's on disk, the sizes it taught the estimator go with it.
        let mut trustworthy = true;
        if let Some(path) = ledger::state_path("token-ledger.json") {
            if let Some(stored) = read_json::<BTreeMap<String, StoredLedger>>(&path) {
                trustworthy = stored.values().all(Ledger::is_current);
                for (key, entry) in stored {
                    // Keys used to be a provider; they are a ring now, and an
                    // old file's plain provider key is this tool's default
                    // account, which is what it always was.
                    if provider_of(&key).is_some() {
                        self.ledgers.insert(key, Ledger::from_stored(entry, now));
                    }
                }
            }
        }
        if let Some(path) = ledger::state_path("rate-limits.json") {
            if let Some(stored) = read_json::<BTreeMap<String, StoredBackoff>>(&path) {
                self.penalties = stored;
            }
        }
        if trustworthy {
            if let Some(path) = ledger::state_path("limit-estimates.json") {
                if let Some(estimator) = read_json::<Estimator>(&path) {
                    self.estimator = estimator;
                }
            }
        }
    }

    /// Write the state out, at most every couple of minutes.
    ///
    /// The telemetry goes with it. Nothing reads it back — it is there to be
    /// looked at when someone asks why a ring says what it says, which
    /// otherwise means asking the person in front of the screen to read it
    /// out. A few kilobytes, rewritten every couple of minutes.
    fn save_state(&mut self, now: DateTime<Utc>, telemetry: &Telemetry) {
        if self.saved_at.is_some_and(|at| {
            now.signed_duration_since(at) < chrono::Duration::minutes(SAVE_EVERY_MINS)
        }) {
            return;
        }
        self.saved_at = Some(now);

        let ledgers: BTreeMap<String, StoredLedger> = self
            .ledgers
            .iter()
            .map(|(instance, ledger)| (instance.clone(), ledger.stored()))
            .collect();
        if let Some(path) = ledger::state_path("token-ledger.json") {
            write_json(&path, &ledgers);
        }
        // Whatever each adapter is serving now, not what it was handed.
        for (instance, adapter) in &self.claude {
            self.penalties
                .insert(instance.clone(), adapter.backoff_state());
        }
        if let Some(path) = ledger::state_path("rate-limits.json") {
            write_json(&path, &self.penalties);
        }
        if let Some(path) = ledger::state_path("last-reading.json") {
            write_json(&path, telemetry);
        }
        if let Some(path) = ledger::state_path("limit-estimates.json") {
            write_json(&path, &self.estimator);
        }
    }

    /// Follow the transcripts for what they cost, and work out what each
    /// window is worth in tokens.
    ///
    /// The percentages come from the providers; the tokens come from disk.
    /// Neither alone answers "how much is left?" in a unit anyone can act on.
    fn account(
        &mut self,
        instance: &str,
        id: ProviderId,
        snapshot: &mut ProviderSnapshot,
        now: DateTime<Utc>,
    ) {
        if !self.config.estimate_tokens {
            // Stop watching, and forget having watched. Keeping the ledger
            // while the feature is off would leave a gap in it that nothing
            // downstream could see: it would still claim to cover the window
            // it stopped following halfway through.
            self.ledgers.clear();
            return;
        }
        // Each account's own transcripts, so one account's spending is never
        // counted against another's limit.
        let (paths, weights): (Vec<_>, _) = match id {
            ProviderId::ClaudeCode => match self.claude.get(instance) {
                Some(adapter) => (adapter.transcripts(), Weights::CLAUDE),
                None => return,
            },
            ProviderId::Codex => match self.codex.get(instance) {
                Some(adapter) => (adapter.transcripts(), Weights::CODEX),
                None => return,
            },
            _ => return,
        };

        let ledger = self.ledgers.entry(instance.to_string()).or_default();
        ledger.start(now);
        for path in paths {
            match id {
                ProviderId::ClaudeCode => ledger.follow(
                    &path,
                    weights,
                    claude::line_stamp,
                    claude::line_usage,
                    claude::line_model,
                ),
                _ => ledger.follow(
                    &path,
                    weights,
                    codex::line_stamp,
                    codex::line_usage,
                    codex::line_model,
                ),
            }
        }
        ledger.prune(now);
        if ledger.is_empty() {
            return;
        }

        let trusted = snapshot.health == Health::Ok;
        for window in &mut snapshot.windows {
            let (Some(pct), Some(started)) = (window.used_pct, window.started_at()) else {
                continue;
            };
            // The ledger has to have been watching for the whole window. Half
            // a week's spend divided by a week's percentage doesn't give a
            // rough limit, it gives one several times too small.
            if !ledger.covers(started) {
                continue;
            }
            let used = ledger.since(started);
            let key = format!("{instance}/{}", window.key);
            let vouched_key = (instance.to_string(), window.key.clone());

            // Only a reading the provider stands behind teaches the estimator
            // anything; a stale one would teach it yesterday's arithmetic.
            let estimate = if trusted {
                self.vouched.insert(vouched_key.clone(), (now, pct));
                self.estimator
                    .observe(&key, window.resets_at, pct as f64, used)
            } else {
                self.estimator
                    .capacity_for(&key)
                    .map(|capacity| crate::estimate::Estimate { used, capacity })
            };

            // What the window cost, told apart. This needs no calibration
            // against a percentage — it is simply what the transcripts say —
            // so it stands even when the capacity estimate doesn't.
            let split = ledger.split_since(started);
            window.input_tokens = Some(split.fresh());
            window.output_tokens = Some(split.output);
            window.cache_tokens = Some(split.cache_read);
            if split.usd > 0.0 {
                window.usd = Some(split.usd);
            }

            let Some(estimate) = estimate else { continue };
            window.used_tokens = Some(estimate.used);
            window.capacity_tokens = Some(estimate.capacity);

            // The reading is old — the token expired, the network went away —
            // but the tokens spent since it are known. Carry the percentage
            // forward rather than showing an hours-old number as if it were
            // now, and mark it as the estimate it is.
            if !trusted {
                if let Some((at, vouched_pct)) = self.vouched.get(&vouched_key).copied() {
                    let since = ledger.since(at);
                    let climbed = since / estimate.capacity * 100.0;
                    let projected = (vouched_pct as f64 + climbed).min(100.0);
                    if projected > vouched_pct as f64 + 0.5 {
                        window.used_pct = Some(projected as f32);
                        window.estimated = true;
                    }
                }
            }

            // A stale window whose reset time has passed rolls into the next
            // one; otherwise it counts down to "resetting" forever. A current
            // reading is left alone — its reset time is the provider's word.
            if let (false, Some(resets_at), Some(minutes)) =
                (trusted, window.resets_at, window.window_minutes)
            {
                if resets_at <= now && minutes > 0 {
                    let length = chrono::Duration::minutes(minutes as i64);
                    let mut next = resets_at;
                    while next <= now {
                        next += length;
                    }
                    window.resets_at = Some(next);
                    window.estimated = true;
                }
            }
        }
        cap_shorter_windows(&mut snapshot.windows);

        // What the subscription has returned. Priced over whatever stretch
        // the ledger covers, with the plan's fee prorated to match, so the
        // ratio means the same thing on day one as in week six.
        if let Some(plan_usd) = self
            .config
            .plan_usd
            .get(instance)
            .or_else(|| self.config.plan_usd.get(id.key()))
            .copied()
        {
            let (split, covered_days) = ledger.split_last_days(VALUE_DAYS, now);
            if covered_days >= MIN_VALUE_DAYS && split.usd > 0.0 {
                snapshot.value = Some(PlanValue {
                    usd: split.usd,
                    plan_usd: plan_usd * covered_days / 30.0,
                    covered_days,
                    input_tokens: split.fresh(),
                    output_tokens: split.output,
                });
            }
        }
    }

    /// An agent reported what it is doing through its own hook.
    ///
    /// Returns true when this changes what's on screen.
    pub fn report_activity(
        &mut self,
        id: ProviderId,
        activity: Activity,
        now: DateTime<Utc>,
    ) -> bool {
        self.reported.insert(id, (activity, now));

        // A hook says which tool it is, never which account, so its word
        // covers every ring of that tool.
        let mut changed = false;
        for (instance, entry) in self.cache.iter_mut() {
            if provider_of(instance) != Some(id) || !self.config.is_enabled(instance) {
                continue;
            }
            if entry.snapshot.activity == activity {
                continue;
            }
            entry.snapshot.activity = activity;
            changed = true;
            // The card lists sessions: move the liveliest one with the provider,
            // so a ring saying "finished" doesn't sit above a session that still
            // claims to be working.
            if let Some(session) = entry.snapshot.sessions.first_mut() {
                session.activity = activity;
            }
        }
        changed
    }

    /// Whether this provider's hooks are talking to us right now.
    fn reporting(&self, id: ProviderId, now: DateTime<Utc>) -> bool {
        self.reported.get(&id).is_some_and(|(_, at)| {
            now.signed_duration_since(*at) < chrono::Duration::minutes(REPORTED_FOR_MINS)
        })
    }

    /// Re-read what the local agents are doing, without touching the network.
    ///
    /// Usage limits come from an API and are polled minutes apart; what an
    /// agent is *doing* is in files on this machine and changes by the second.
    /// Tying the two together meant an answer that arrived between two polls
    /// was never seen — the chime and the ring both missed it. This pass is
    /// cheap: it only re-reads when a transcript has been written since the
    /// last look.
    ///
    /// Returns true when something changed.
    pub fn refresh_activity(&mut self, now: DateTime<Utc>) -> bool {
        let mut changed = false;

        let watched: Vec<(String, ProviderId)> = self
            .cache
            .keys()
            .filter_map(|instance| {
                let id = provider_of(instance)?;
                matches!(id, ProviderId::ClaudeCode | ProviderId::Codex)
                    .then(|| (instance.clone(), id))
            })
            .collect();

        for (instance, id) in watched {
            if !self.config.is_enabled(&instance) {
                continue;
            }

            let mark = match id {
                ProviderId::ClaudeCode => {
                    self.claude.get(&instance).and_then(|a| a.activity_mark())
                }
                _ => self.codex.get(&instance).and_then(|a| a.activity_mark()),
            };
            let Some(mark) = mark else { continue };
            if self.activity_marks.get(&instance) == Some(&mark) {
                continue;
            }
            self.activity_marks.insert(instance.clone(), mark);

            let sessions = match id {
                ProviderId::ClaudeCode => match self.claude.get(&instance) {
                    Some(adapter) => adapter.scan_sessions(now),
                    None => continue,
                },
                _ => match self.codex.get(&instance) {
                    Some(adapter) => adapter.scan_sessions(now),
                    None => continue,
                },
            };

            let Some(entry) = self.cache.get_mut(&instance) else {
                continue;
            };
            // With hooks reporting, the transcripts still supply titles and
            // token counts but no longer get a vote on the activity.
            let activity = if let Some((reported, _)) =
                self.reported.get(&id).copied().filter(|(_, at)| {
                    now.signed_duration_since(*at) < chrono::Duration::minutes(REPORTED_FOR_MINS)
                }) {
                reported
            } else {
                sessions
                    .iter()
                    .fold(Activity::Idle, |acc, s| acc.merge(s.activity))
            };

            if entry.snapshot.sessions != sessions || entry.snapshot.activity != activity {
                entry.snapshot.sessions = sessions;
                entry.snapshot.activity = activity;
                changed = true;
            }
        }

        changed
    }

    /// Refresh whatever is due and return the current picture plus any
    /// threshold crossings that just happened.
    pub async fn poll(&mut self, now: DateTime<Utc>) -> (Telemetry, Vec<Alert>) {
        let mut alerts = Vec::new();

        let instances = self.instances();
        let live: Vec<String> = instances
            .iter()
            .filter(|(instance, _, _)| self.config.is_enabled(instance))
            .map(|(instance, _, _)| instance.clone())
            .collect();
        // Anything not on that list keeps nothing in memory.
        self.cache
            .retain(|instance, _| live.iter().any(|k| k == instance));
        self.drop_unused(&live);

        for (instance, id, account) in instances {
            if !self.config.is_enabled(&instance) || !self.is_due(&instance, id, now) {
                continue;
            }

            let mut snapshot = self
                .collect_one(&instance, id, account.as_deref(), now)
                .await;
            self.account(&instance, id, &mut snapshot, now);

            // See `refresh_activity`: a hook's word outranks a re-read file.
            // A hook can't say which account it is, so its word covers every
            // ring of that tool.
            if let Some((reported, _)) = self.reported.get(&id).copied() {
                if self.reporting(id, now) && snapshot.health != Health::Unavailable {
                    snapshot.activity = reported;
                }
            }

            // Only alert on numbers we actually trust, and only for providers
            // that haven't been told to keep quiet.
            if self.config.notify_on_thresholds
                && snapshot.health == Health::Ok
                && !self.config.is_muted(&instance)
            {
                for window in &snapshot.windows {
                    if window.estimated {
                        continue;
                    }
                    if let Some(threshold) = self.alerts.observe(&instance, window) {
                        alerts.push(Alert {
                            provider: id,
                            provider_name: snapshot.name.clone(),
                            window_label: window.label.clone(),
                            threshold,
                            used_pct: window.used_pct.unwrap_or(threshold),
                        });
                    }
                }
            }

            self.cache.insert(
                instance,
                CacheEntry {
                    collected_at: now,
                    snapshot,
                },
            );
        }

        self.refresh_all = false;
        let telemetry = self.telemetry(now);
        self.save_state(now, &telemetry);
        (telemetry, alerts)
    }

    /// Build the payload from cache, ageing snapshots past their budget.
    pub fn telemetry(&self, now: DateTime<Utc>) -> Telemetry {
        let instances = self.instances();
        // Every extra account, on or off: a ring only exists for the ones
        // switched on, and settings has to be able to switch the rest back.
        let accounts = instances
            .iter()
            .filter_map(|(_, id, account)| Some(ExtraAccount::new(*id, account.as_deref()?)))
            .collect();

        let snapshots = instances
            .into_iter()
            .filter(|(instance, _, _)| self.config.is_enabled(instance))
            .filter_map(|(instance, id, _)| {
                let entry = self.cache.get(&instance)?;
                let mut snapshot = entry.snapshot.clone();
                let age = now.signed_duration_since(entry.collected_at);

                if age > staleness_budget(id) && snapshot.health != Health::Unavailable {
                    snapshot.mark_stale(format!(
                        "No update for {} minutes",
                        age.num_minutes().max(1)
                    ));
                }
                Some(snapshot)
            })
            .collect();

        Telemetry {
            accounts,
            ..Telemetry::from_snapshots(snapshots)
        }
    }

    /// Shortest interval across the enabled providers, which is how often the
    /// caller needs to tick.
    pub fn tick_interval_secs(&self) -> u64 {
        self.config
            .ordered_providers()
            .into_iter()
            .filter(|id| self.config.is_enabled(id.key()))
            .map(|id| self.config.poll.for_provider(id))
            .min()
            .unwrap_or(60)
            // Never spin faster than the activity pass needs.
            .max(self.config.poll.activity_secs.max(1))
    }
}

/// Stop a short window from claiming to hold more than a long one.
///
/// Each window is fitted on its own, so nothing in the arithmetic knows that
/// five hours cannot hold more than a week does. When the fits disagree it is
/// the shorter window that is wrong — it has had less spending to fit against
/// — so it takes the longer one's figure as a ceiling.
fn cap_shorter_windows(windows: &mut [UsageWindow]) {
    let mut order: Vec<usize> = (0..windows.len())
        .filter(|&i| windows[i].window_minutes.is_some() && windows[i].capacity_tokens.is_some())
        .collect();
    order.sort_by_key(|&i| std::cmp::Reverse(windows[i].window_minutes.unwrap_or(0)));

    // Walking longest to shortest, carrying the tightest ceiling so far.
    let mut ceiling: Option<(u32, f64)> = None;
    for i in order {
        let minutes = windows[i].window_minutes.unwrap_or(0);
        let capacity = windows[i].capacity_tokens.unwrap_or(0.0);
        let capped = match ceiling {
            Some((longer, limit)) if longer > minutes => capacity.min(limit),
            _ => capacity,
        };
        // Only the token figure changes here; the percentage is still the
        // provider's own and shouldn't be marked as approximate.
        if capped < capacity {
            windows[i].capacity_tokens = Some(capped);
        }
        ceiling = Some(match ceiling {
            // Equal-length windows constrain each other, not themselves.
            Some((m, limit)) if m == minutes => (minutes, limit.min(capped)),
            _ => (minutes, capped),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PollConfig;

    /// The five-hour window can't be worth more than the week it sits inside,
    /// which is what a fit against a barely-started window kept claiming.
    #[test]
    fn a_short_window_cannot_outgrow_a_long_one() {
        let mut windows = vec![
            {
                let mut w = UsageWindow::new("five_hour", "5h").lasting(5 * 60);
                w.capacity_tokens = Some(631_400_000.0);
                w
            },
            {
                let mut w = UsageWindow::new("seven_day", "7d").lasting(7 * 24 * 60);
                w.capacity_tokens = Some(196_100_000.0);
                w
            },
        ];
        cap_shorter_windows(&mut windows);
        assert_eq!(windows[0].capacity_tokens, Some(196_100_000.0));
        // The longer window is never touched.
        assert_eq!(windows[1].capacity_tokens, Some(196_100_000.0));
        // Only the token figure moved: the percentage is still the
        // provider's, so neither window is marked as an estimate.
        assert!(!windows[0].estimated && !windows[1].estimated);
    }

    #[test]
    fn a_sane_pair_is_left_alone() {
        let mut windows = vec![
            {
                let mut w = UsageWindow::new("five_hour", "5h").lasting(5 * 60);
                w.capacity_tokens = Some(40_000_000.0);
                w
            },
            {
                let mut w = UsageWindow::new("seven_day", "7d").lasting(7 * 24 * 60);
                w.capacity_tokens = Some(500_000_000.0);
                w
            },
        ];
        cap_shorter_windows(&mut windows);
        assert_eq!(windows[0].capacity_tokens, Some(40_000_000.0));
        assert_eq!(windows[1].capacity_tokens, Some(500_000_000.0));
    }

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// A config with nothing installed, so every adapter reports Unavailable
    /// quickly and deterministically.
    fn test_config() -> Config {
        Config {
            // Point Ollama at a closed port so it fails fast.
            ollama_url: "http://127.0.0.1:1".into(),
            notify_on_thresholds: true,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn poll_produces_a_snapshot_for_every_enabled_provider() {
        let mut collector = Collector::new(test_config());
        let (telemetry, alerts) = collector.poll(at("2026-01-01T12:00:00Z")).await;

        // One ring per provider at least — a machine signed into a tool
        // twice draws two, which is the point of instance keys.
        let kinds: std::collections::BTreeSet<_> =
            telemetry.providers.iter().map(|p| p.id).collect();
        assert_eq!(kinds.len(), ProviderId::ALL.len());
        assert!(telemetry.providers.iter().all(|p| !p.key.is_empty()));
        assert!(
            alerts.is_empty(),
            "nothing is installed, so nothing to alert"
        );
        // Providers sort into a stable display order.
        assert_eq!(telemetry.providers[0].id, ProviderId::ClaudeCode);
    }

    #[tokio::test]
    async fn disabled_providers_are_dropped_from_the_payload() {
        let mut config = test_config();
        config.providers.insert("ollama".into(), false);
        config.providers.insert("cursor".into(), false);

        let mut collector = Collector::new(config);
        let (telemetry, _) = collector.poll(at("2026-01-01T12:00:00Z")).await;

        let kinds: std::collections::BTreeSet<_> =
            telemetry.providers.iter().map(|p| p.id).collect();
        assert!(!kinds.contains(&ProviderId::Ollama));
        assert!(!kinds.contains(&ProviderId::Cursor));
        assert_eq!(kinds.len(), ProviderId::ALL.len() - 2);
    }

    #[tokio::test]
    async fn turning_a_provider_off_removes_it_immediately() {
        let mut collector = Collector::new(test_config());
        let now = at("2026-01-01T12:00:00Z");
        collector.poll(now).await;
        assert!(collector.telemetry(now).get(ProviderId::Ollama).is_some());

        let mut config = test_config();
        config.providers.insert("ollama".into(), false);
        collector.set_config(config);

        assert!(
            collector.telemetry(now).get(ProviderId::Ollama).is_none(),
            "a disabled provider must not survive in the cache"
        );
    }

    #[tokio::test]
    async fn providers_are_only_re_collected_when_due() {
        let mut collector = Collector::new(Config {
            poll: PollConfig {
                ollama_secs: 10,
                ..PollConfig::default()
            },
            ..test_config()
        });

        let start = at("2026-01-01T12:00:00Z");
        collector.poll(start).await;

        // Well inside every interval: nothing is due.
        let soon = start + chrono::Duration::seconds(5);
        assert!(!collector.is_due(ProviderId::Ollama.key(), ProviderId::Ollama, soon));
        assert!(!collector.is_due(ProviderId::ClaudeCode.key(), ProviderId::ClaudeCode, soon));

        // Past Ollama's 10s interval but not Claude's 90s one.
        let later = start + chrono::Duration::seconds(11);
        assert!(collector.is_due(ProviderId::Ollama.key(), ProviderId::Ollama, later));
        assert!(!collector.is_due(ProviderId::ClaudeCode.key(), ProviderId::ClaudeCode, later));
    }

    #[tokio::test]
    async fn a_config_change_forces_a_full_refresh() {
        let mut collector = Collector::new(test_config());
        let start = at("2026-01-01T12:00:00Z");
        collector.poll(start).await;

        let soon = start + chrono::Duration::seconds(1);
        assert!(!collector.is_due(ProviderId::ClaudeCode.key(), ProviderId::ClaudeCode, soon));

        collector.set_config(test_config());
        assert!(
            collector.is_due(ProviderId::ClaudeCode.key(), ProviderId::ClaudeCode, soon),
            "new settings should be reflected right away"
        );
    }

    #[tokio::test]
    async fn stale_snapshots_are_marked_rather_than_silently_shown() {
        let mut collector = Collector::new(test_config());
        let start = at("2026-01-01T12:00:00Z");

        // Seed a healthy Claude snapshot directly; the real adapter needs a
        // Claude install to produce one.
        collector.poll(start).await;
        collector.cache.insert(
            ProviderId::ClaudeCode.key().to_string(),
            CacheEntry {
                collected_at: start,
                snapshot: ProviderSnapshot::new(ProviderId::ClaudeCode).with_windows(vec![
                    crate::model::UsageWindow::new("five_hour", "5h").with_pct(40.0),
                ]),
            },
        );

        // Inside the budget: still OK.
        let fresh = collector
            .telemetry(start + chrono::Duration::minutes(5))
            .get(ProviderId::ClaudeCode)
            .unwrap()
            .clone();
        assert_eq!(fresh.health, Health::Ok);

        // Past it: marked stale, but the last numbers are kept.
        let stale = collector
            .telemetry(start + chrono::Duration::minutes(30))
            .get(ProviderId::ClaudeCode)
            .unwrap()
            .clone();
        assert_eq!(stale.health, Health::Stale);
        assert_eq!(stale.peak_pct(), Some(40.0));
        assert!(stale.detail.unwrap().contains("30 minutes"));
    }

    #[tokio::test]
    async fn unavailable_providers_are_not_aged_into_staleness() {
        let mut collector = Collector::new(test_config());
        let start = at("2026-01-01T12:00:00Z");
        collector.poll(start).await;

        let much_later = start + chrono::Duration::hours(4);
        let ollama = collector
            .telemetry(much_later)
            .get(ProviderId::Ollama)
            .unwrap()
            .clone();
        assert_eq!(
            ollama.health,
            Health::Unavailable,
            "a tool that isn't running can't go stale"
        );
    }

    #[test]
    fn tick_interval_follows_the_fastest_enabled_provider() {
        let collector = Collector::new(Config {
            poll: PollConfig {
                ollama_secs: 10,
                activity_secs: 3,
                ..PollConfig::default()
            },
            ..test_config()
        });
        assert_eq!(collector.tick_interval_secs(), 10);

        // With the local runtimes off, the next-fastest provider sets the pace.
        let mut config = test_config();
        config.providers.insert("ollama".into(), false);
        config.providers.insert("lmStudio".into(), false);
        config.poll = PollConfig {
            ollama_secs: 10,
            lm_studio_secs: 10,
            activity_secs: 3,
            ..PollConfig::default()
        };
        let collector = Collector::new(config);
        assert_eq!(collector.tick_interval_secs(), 45);
    }

    #[tokio::test]
    async fn threshold_alerts_fire_once_per_crossing() {
        let mut collector = Collector::new(test_config());
        let window = |pct: f32| crate::model::UsageWindow::new("five_hour", "5h").with_pct(pct);

        // Drive the ledger directly: it is the piece that decides.
        let snap = ProviderSnapshot::new(ProviderId::ClaudeCode);
        assert!(collector
            .alerts
            .observe(ProviderId::ClaudeCode.key(), &window(50.0))
            .is_none());
        assert_eq!(
            collector
                .alerts
                .observe(ProviderId::ClaudeCode.key(), &window(85.0)),
            Some(80.0)
        );
        assert!(collector
            .alerts
            .observe(ProviderId::ClaudeCode.key(), &window(86.0))
            .is_none());
        assert_eq!(snap.health, Health::Ok);
    }

    #[test]
    fn alert_copy_reads_sensibly() {
        let alert = Alert {
            provider: ProviderId::ClaudeCode,
            provider_name: "Claude Code".into(),
            window_label: "5h session".into(),
            threshold: 80.0,
            used_pct: 83.0,
        };
        assert_eq!(alert.title(), "Claude Code at 83%");
        assert_eq!(alert.body(), "5h session window · 83% used");

        let maxed = Alert {
            threshold: 100.0,
            used_pct: 100.0,
            ..alert
        };
        assert_eq!(maxed.title(), "Claude Code limit reached");
    }
}
