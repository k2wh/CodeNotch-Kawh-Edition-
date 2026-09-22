//! What an agent has actually spent, minute by minute.
//!
//! The providers report a *percentage* of a limit; the transcripts on disk
//! report *tokens*. Putting the two together is what lets the notch say how
//! big the limit is — but only if the token side covers the same window the
//! percentage does, and a tail read of a busy transcript doesn't.
//!
//! So this keeps a ledger: each pass reads only what has been appended to each
//! transcript since the last look and records what it cost, tagged with when.
//! Asking "how much since 3pm?" is then a sum, and it stays right no matter
//! how long the window or how busy the session.
//!
//! Not every token costs the same, which is the other half of the problem: a
//! cached read is a tenth of a fresh one, and an output token several times
//! more than an input. Everything is therefore recorded in *input-equivalent*
//! tokens, using the providers' own price ratios as the best available proxy
//! for how they weigh a limit.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Ledger entries older than this are dropped: nothing looks further back
/// than the seven-day window, with a day's slack.
const KEEP_DAYS: i64 = 8;

/// Per-day totals are kept this long — two months, so a plan's worth can be
/// judged over a full billing cycle and the one before it. A row a day costs
/// nothing to keep.
const ROLLUP_DAYS: i64 = 62;

/// A transcript first seen mid-session is read from this far back rather than
/// from the beginning: enough to cover a busy hour, without parsing tens of
/// megabytes of history the moment the app starts.
const FIRST_READ_BYTES: u64 = 1024 * 1024;

/// How token kinds weigh against each other, as input-equivalents.
///
/// These are price ratios — Anthropic and OpenAI don't publish how their rate
/// limits weigh a request, and price is the closest public proxy. Being a
/// little wrong here shifts every estimate by the same factor, which the
/// calibration against reported percentages then absorbs.
#[derive(Debug, Clone, Copy)]
pub struct Weights {
    pub input: f64,
    pub output: f64,
    pub cache_write: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

impl Weights {
    /// Claude: output is 5x input, a five-minute cache write 1.25x, an
    /// hour-long one 2x, a cache read 0.1x.
    pub const CLAUDE: Weights = Weights {
        input: 1.0,
        output: 5.0,
        cache_write: 1.25,
        cache_write_1h: 2.0,
        cache_read: 0.1,
    };

    /// Codex/GPT: output is 8x input, cached input 0.1x. OpenAI has no
    /// separate cache-write charge and its transcripts name no lifetime, so
    /// writes weigh the same as input and the hour-long column goes unused.
    pub const CODEX: Weights = Weights {
        input: 1.0,
        output: 8.0,
        cache_write: 1.0,
        cache_write_1h: 1.0,
        cache_read: 0.1,
    };
}

/// Tokens by kind, and what they would have cost to buy.
///
/// Kept apart rather than summed because that is the question a subscriber
/// asks: output tokens are five to eight times the price of input ones, so
/// two sessions with the same total can differ severalfold in worth.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Split {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub cache_write: u64,
    /// Cache writes with the one-hour lifetime, which cost more — see
    /// [`crate::prices::Price::cache_write_1h`].
    #[serde(default)]
    pub cache_write_1h: u64,
    #[serde(default)]
    pub cache_read: u64,
    /// List price for the above, in USD, at the rates of the model that
    /// produced them. Zero for models [`crate::prices`] can't place.
    #[serde(default)]
    pub usd: f64,
}

impl Split {
    fn add(&mut self, other: &Split) {
        self.input += other.input;
        self.output += other.output;
        self.cache_write += other.cache_write;
        self.cache_write_1h += other.cache_write_1h;
        self.cache_read += other.cache_read;
        self.usd += other.usd;
    }

    /// Tokens that were new to the model: what was typed, read or written to
    /// the cache this turn.
    ///
    /// Cache reads are left out deliberately. They are 98% of the raw count —
    /// the whole conversation re-read on every turn — so adding them in gives
    /// a "tokens sent" figure in the hundreds of millions that answers no
    /// question anyone is asking. They are still paid for, so they stay in
    /// [`Split::usd`], and are reported on their own.
    pub fn fresh(&self) -> u64 {
        self.input + self.cache_write + self.cache_write_1h
    }
}

/// What one line of a transcript says it spent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub cache_read: u64,
    /// The provider's id for the message this line belongs to, where it has
    /// one. Claude Code writes a line per content block of an assistant
    /// message and repeats the whole usage block on each — up to fourteen
    /// times for one message that was billed once. Counting them all
    /// overstated this machine's spending by a factor of two and a half.
    pub id: Option<String>,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache_write + self.cache_write_1h + self.cache_read
    }
}

/// One moment's spending.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Entry {
    pub at: DateTime<Utc>,
    /// Input-equivalent tokens, which is what the limit estimator fits
    /// against. See [`Weights`].
    pub tokens: f64,
    #[serde(default, flatten)]
    pub split: Split,
    /// Hash of the message id this came from, so the same message isn't
    /// counted twice — see [`Usage::id`]. Stored rather than kept only in
    /// memory so a restart doesn't recount what it re-reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

/// Where each transcript was last read up to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Marks {
    /// Path -> offset read to.
    files: BTreeMap<String, u64>,
    /// Path -> the model last declared in it. Codex names the model once per
    /// session, on a line of its own, so a usage line on its own says nothing
    /// about what it cost; this carries the answer forward, across restarts.
    #[serde(default)]
    models: BTreeMap<String, String>,
}

/// A running account of what one provider has spent.
#[derive(Debug, Default)]
pub struct Ledger {
    entries: Vec<Entry>,
    /// Per-day totals, kept long after the entries behind them are pruned:
    /// the limit windows need minutes, but "what is this plan returning?" is
    /// a question about the billing month.
    days: BTreeMap<chrono::NaiveDate, Split>,
    marks: Marks,
    began: Option<DateTime<Utc>>,
    /// Message ids already counted, as hashes — see [`Usage::id`]. Derived
    /// from the entries, and rebuilt whenever they change.
    seen: std::collections::HashSet<u64>,
}

impl Ledger {
    /// Whether the ledger was already watching at `since`.
    ///
    /// It only knows what it has seen. A week of transcripts runs to hundreds
    /// of files and over a gigabyte, so the first pass reads recent history,
    /// not all of it — which means a total "since last Tuesday" would be
    /// missing most of the week and, divided into a percentage, would claim
    /// the limit was a fraction of its real size. A window reaching back
    /// before the ledger started doesn't give a rough estimate; it gives a
    /// wrong one.
    pub fn covers(&self, since: DateTime<Utc>) -> bool {
        self.began.is_some_and(|began| began <= since)
    }

    /// Mark the ledger as watching from now, if it wasn't already.
    ///
    /// Called before the first read rather than after it, because the first
    /// read reaches back into history that the ledger cannot claim to have
    /// watched — and the day totals behind [`Ledger::split_last_days`] have
    /// to leave that history out or a plan's worth is inflated by whatever
    /// happened to be in the last megabyte of a transcript.
    pub fn start(&mut self, now: DateTime<Utc>) {
        self.began.get_or_insert(now);
    }

    /// Total spent since `since`, in input-equivalent tokens.
    pub fn since(&self, since: DateTime<Utc>) -> f64 {
        self.entries
            .iter()
            .filter(|e| e.at >= since)
            .map(|e| e.tokens)
            .sum()
    }

    /// Tokens by kind, and their list price, since `since`.
    pub fn split_since(&self, since: DateTime<Utc>) -> Split {
        let mut total = Split::default();
        for entry in self.entries.iter().filter(|e| e.at >= since) {
            total.add(&entry.split);
        }
        total
    }

    /// The same, over whole days, which reaches back further than the entries
    /// do — see [`Ledger::days`]. Returns the totals and how many days of
    /// history they actually rest on, which is rarely the number asked for.
    pub fn split_last_days(&self, days: u32, now: DateTime<Utc>) -> (Split, f64) {
        let first = (now - Duration::days(days as i64)).date_naive();
        let mut total = Split::default();
        for (_, day) in self.days.range(first..) {
            total.add(day);
        }

        let covered = match self.began {
            Some(began) if began > now - Duration::days(days as i64) => {
                (now - began).num_seconds() as f64 / 86_400.0
            }
            Some(_) => days as f64,
            None => 0.0,
        };
        (total, covered.max(0.0))
    }

    /// Whether anything has been recorded at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record what a line of a transcript cost, if it says.
    fn record(&mut self, at: DateTime<Utc>, tokens: f64, split: Split, id: Option<u64>) {
        if tokens <= 0.0 {
            return;
        }
        if let Some(id) = id {
            // Already counted: the same message, written out again.
            if !self.seen.insert(id) {
                return;
            }
        }
        self.entries.push(Entry {
            at,
            tokens,
            split,
            id,
        });
        // Day totals only count what happened while the ledger was watching;
        // see [`Ledger::start`].
        if self.began.is_none_or(|began| at >= began) {
            self.days.entry(at.date_naive()).or_default().add(&split);
        }
    }

    /// Read whatever has been appended to `path` since the last pass.
    ///
    /// `usage` pulls the token counts out of one line and `stamp` its time;
    /// both return `None` for lines that aren't about spending. `model_of`
    /// reads a model name off any line that declares one, which is how a
    /// line's tokens get a price.
    pub fn follow<U, S, M>(
        &mut self,
        path: &Path,
        weights: Weights,
        stamp: S,
        usage: U,
        model_of: M,
    ) where
        S: Fn(&str) -> Option<DateTime<Utc>>,
        U: Fn(&str) -> Option<Usage>,
        M: Fn(&str) -> Option<String>,
    {
        let key = path.to_string_lossy().to_string();
        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);

        let mark = self.marks.files.get(&key).copied();
        let from = match mark {
            // Shorter than when we last looked: rotated or rewritten, so the
            // old offset points at nothing. What's there now is short by
            // definition — read all of it.
            Some(offset) if offset > len => 0,
            Some(offset) => offset,
            // First sight: enough history to be useful, not the whole file.
            None => len.saturating_sub(FIRST_READ_BYTES),
        };
        if from >= len {
            self.marks.files.insert(key, len);
            return;
        }

        let mut text = String::new();
        if file.seek(SeekFrom::Start(from)).is_err() || file.read_to_string(&mut text).is_err() {
            return;
        }
        // Whole lines only; the agent may be mid-write.
        let end = text.rfind('\n').map_or(0, |i| i + 1);
        let mut lines = text[..end].lines();
        // A first read starts mid-file, so its opening line is a fragment.
        if mark.is_none() && from > 0 {
            lines.next();
        }

        let mut model = self.marks.models.get(&key).cloned();
        for line in lines {
            // Before the usage on the same line is priced: on Claude's
            // transcripts the model sits on the very line that reports what
            // it spent.
            if let Some(named) = model_of(line) {
                model = Some(named);
            }
            let Some(usage) = usage(line) else { continue };
            let at = stamp(line).unwrap_or_else(Utc::now);

            let mut split = Split {
                input: usage.input,
                output: usage.output,
                cache_write: usage.cache_write,
                cache_write_1h: usage.cache_write_1h,
                cache_read: usage.cache_read,
                usd: 0.0,
            };
            if let Some(price) = crate::prices::for_model(model.as_deref()) {
                split.usd = price.usd(&usage);
            }

            self.record(
                at,
                usage.input as f64 * weights.input
                    + usage.output as f64 * weights.output
                    + usage.cache_write as f64 * weights.cache_write
                    + usage.cache_write_1h as f64 * weights.cache_write_1h
                    + usage.cache_read as f64 * weights.cache_read,
                split,
                usage.id.as_deref().map(hash_id),
            );
        }

        self.marks.files.insert(key.clone(), from + end as u64);
        if let Some(model) = model {
            self.marks.models.insert(key, model);
        }
    }

    /// Forget what is too old to matter, and files that have gone.
    ///
    /// Also where coverage starts counting: this runs once per pass, so the
    /// first one marks the moment the ledger began watching.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        self.began.get_or_insert(now);
        let cutoff = now - Duration::days(KEEP_DAYS);
        self.entries.retain(|e| e.at >= cutoff);
        self.seen = self.entries.iter().filter_map(|e| e.id).collect();
        let oldest_day = (now - Duration::days(ROLLUP_DAYS)).date_naive();
        self.days.retain(|day, _| *day >= oldest_day);
        self.marks.files.retain(|path, _| Path::new(path).exists());
        self.marks.models.retain(|path, _| Path::new(path).exists());
    }
}

/// The ledgers for every provider, and where they live on disk.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StoredLedger {
    pub entries: Vec<Entry>,
    #[serde(default)]
    files: BTreeMap<String, u64>,
    /// When this ledger first started watching — see [`Ledger::covers`].
    #[serde(default)]
    began: Option<DateTime<Utc>>,
    /// Per-day totals, which outlive the entries — see [`Ledger::days`].
    #[serde(default)]
    days: BTreeMap<chrono::NaiveDate, Split>,
    /// The model each transcript was last seen using.
    #[serde(default)]
    models: BTreeMap<String, String>,
    /// What produced this file. Older counters double-counted messages, so
    /// their totals — and anything learned from them — have to be thrown
    /// away rather than blended with figures that are right.
    #[serde(default)]
    version: u32,
}

/// Bumped whenever a stored ledger's numbers stop meaning what they used to.
pub const LEDGER_VERSION: u32 = 2;

impl Ledger {
    pub fn stored(&self) -> StoredLedger {
        StoredLedger {
            entries: self.entries.clone(),
            files: self.marks.files.clone(),
            began: self.began,
            days: self.days.clone(),
            models: self.marks.models.clone(),
            version: LEDGER_VERSION,
        }
    }

    /// Whether a stored ledger was written by a counter whose figures can
    /// still be trusted.
    pub fn is_current(stored: &StoredLedger) -> bool {
        stored.version == LEDGER_VERSION
    }

    pub fn from_stored(stored: StoredLedger, now: DateTime<Utc>) -> Self {
        if !Self::is_current(&stored) {
            // Start over: counting from scratch takes hours, but blending in
            // numbers known to be inflated would be wrong for weeks.
            let mut fresh = Ledger::default();
            fresh.prune(now);
            return fresh;
        }
        let mut ledger = Ledger {
            entries: stored.entries,
            days: stored.days,
            marks: Marks {
                files: stored.files,
                models: stored.models,
            },
            began: stored.began,
            seen: Default::default(),
        };
        ledger.prune(now);
        ledger
    }
}

/// A message id, small enough to keep one per entry.
///
/// Collisions would drop a message; at a few thousand ids over eight days the
/// chance of one is around a billionth, against the certainty of counting
/// some messages fourteen times without this.
fn hash_id(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    hasher.finish()
}

/// `%APPDATA%\CodeNotch\<name>`.
pub fn state_path(name: &str) -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("CodeNotch").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    /// Lines are `<unix secs> <input> <output> <cache write> <cache read>`.
    fn stamp(line: &str) -> Option<DateTime<Utc>> {
        let secs: i64 = line.split_whitespace().next()?.parse().ok()?;
        DateTime::from_timestamp(secs, 0)
    }

    fn usage(line: &str) -> Option<Usage> {
        let n: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|p| p.parse().ok())
            .collect();
        (n.len() == 4).then(|| Usage {
            input: n[0],
            output: n[1],
            cache_write: n[2],
            cache_read: n[3],
            cache_write_1h: 0,
            id: None,
        })
    }

    /// The test lines name no model, so nothing is priced unless a test says
    /// otherwise by wrapping this.
    fn no_model(_line: &str) -> Option<String> {
        None
    }

    fn write(path: &Path, lines: &[String]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    /// Codex declares the model once, on a line of its own; every usage line
    /// after it has to be priced with it, including after a restart.
    #[test]
    fn the_model_carries_forward_from_the_line_that_named_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let named = |line: &str| line.strip_prefix("model ").map(str::to_string);

        let mut ledger = Ledger::default();
        write(&path, &["model claude-opus-5".to_string()]);
        // 1M input and 1M output of Opus: $5 + $25.
        write(
            &path,
            &[format!("{} 1000000 1000000 0 0", at(0).timestamp())],
        );
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, named);
        assert_eq!(ledger.split_since(at(-60)).usd, 30.0);

        // A later pass never sees the naming line again, and must still know.
        let stored = ledger.stored();
        let mut ledger = Ledger::from_stored(stored, at(1));
        write(&path, &[format!("{} 1000000 0 0 0", at(2).timestamp())]);
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, named);
        assert_eq!(ledger.split_since(at(-60)).usd, 35.0);
    }

    /// Claude Code writes a line per content block of an assistant message,
    /// each repeating that message's whole usage block — measured at up to
    /// fourteen lines for one message. The API billed it once.
    #[test]
    fn a_message_written_out_many_times_is_counted_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        // `<id> <secs> <input> <output> <cache write> <cache read>`
        let with_id = |line: &str| -> Option<Usage> {
            let mut parts = line.split_whitespace();
            let id = parts.next()?.to_string();
            let rest: Vec<u64> = parts.map(|p| p.parse().unwrap()).collect();
            Some(Usage {
                input: rest[1],
                output: rest[2],
                cache_write: rest[3],
                cache_read: rest[4],
                cache_write_1h: 0,
                id: Some(id),
            })
        };
        let stamp_after_id = |line: &str| -> Option<DateTime<Utc>> {
            let secs: i64 = line.split_whitespace().nth(1)?.parse().ok()?;
            DateTime::from_timestamp(secs, 0)
        };

        let mut ledger = Ledger::default();
        let t = at(0).timestamp();
        write(
            &path,
            &[
                format!("msg_A {t} 100 10 0 0"),
                format!("msg_A {t} 100 10 0 0"),
                format!("msg_A {t} 100 10 0 0"),
                format!("msg_B {t} 100 10 0 0"),
            ],
        );
        ledger.follow(&path, Weights::CLAUDE, stamp_after_id, with_id, no_model);

        // Two messages at 150 input-equivalents each, not four.
        assert_eq!(ledger.since(at(-60)), 300.0);
        assert_eq!(ledger.split_since(at(-60)).output, 20);

        // And still once after a restart that re-reads the same lines.
        let mut ledger = Ledger::from_stored(ledger.stored(), at(1));
        ledger.marks = Marks::default();
        ledger.follow(&path, Weights::CLAUDE, stamp_after_id, with_id, no_model);
        assert_eq!(ledger.since(at(-60)), 300.0);
    }

    /// A first read reaches back into history the ledger wasn't watching.
    /// Those tokens are real, but they didn't happen on this plan's watch,
    /// and counting them against a few minutes of fee would put the return
    /// out by orders of magnitude.
    #[test]
    fn spending_from_before_the_ledger_started_is_not_counted_as_return() {
        let mut ledger = Ledger::default();
        let now = at(0);
        ledger.start(now);

        let worth = |usd| Split {
            input: 1,
            usd,
            ..Default::default()
        };
        ledger.record(now - Duration::hours(3), 10.0, worth(90.0), None); // backfill
        ledger.record(now + Duration::minutes(30), 10.0, worth(4.0), None); // watched

        let (split, _) = ledger.split_last_days(30, now + Duration::hours(2));
        assert_eq!(split.usd, 4.0);
        // The entry is still there for the limit estimate, which asks a
        // different question and knows its own coverage.
        assert_eq!(ledger.since(now - Duration::days(1)), 20.0);
    }

    /// Whole-day totals outlive the entries they came from, so the plan's
    /// worth can still be judged after the eight-day detail is pruned.
    #[test]
    fn day_totals_survive_the_pruning_of_entries() {
        let mut ledger = Ledger::default();
        let now = at(0);
        ledger.record(
            now - Duration::days(20),
            100.0,
            Split {
                input: 7,
                usd: 4.0,
                ..Default::default()
            },
            None,
        );
        ledger.began = Some(now - Duration::days(45));
        ledger.prune(now);

        assert!(ledger.is_empty(), "the entry itself is long gone");
        let (split, covered) = ledger.split_last_days(30, now);
        assert_eq!(split.usd, 4.0);
        assert_eq!(split.input, 7);
        // Watching for longer than asked about: the answer is the 30 days.
        assert_eq!(covered, 30.0);
    }

    #[test]
    fn only_new_lines_are_counted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut ledger = Ledger::default();

        write(&path, &[format!("{} 100 10 0 0", at(0).timestamp())]);
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, no_model);
        // 100 input + 10 output x5
        assert_eq!(ledger.since(at(-60)), 150.0);

        // Reading again without new lines adds nothing.
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, no_model);
        assert_eq!(ledger.since(at(-60)), 150.0);

        write(&path, &[format!("{} 0 0 0 1000", at(10).timestamp())]);
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, no_model);
        // ...plus 1000 cache reads at a tenth each.
        assert_eq!(ledger.since(at(-60)), 250.0);
    }

    #[test]
    fn a_window_only_counts_what_falls_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut ledger = Ledger::default();

        write(
            &path,
            &[
                format!("{} 100 0 0 0", at(0).timestamp()),
                format!("{} 300 0 0 0", at(600).timestamp()),
            ],
        );
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, no_model);

        assert_eq!(ledger.since(at(-1)), 400.0);
        assert_eq!(ledger.since(at(300)), 300.0, "only the later one");
        assert_eq!(ledger.since(at(900)), 0.0);
    }

    #[test]
    fn a_truncated_transcript_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.jsonl");
        let mut ledger = Ledger::default();

        write(&path, &[format!("{} 100 0 0 0", at(0).timestamp())]);
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, no_model);

        // Replaced by something shorter: pick up from its end, not from a
        // stale offset that would read garbage.
        std::fs::write(&path, "").unwrap();
        write(&path, &[format!("{} 50 0 0 0", at(10).timestamp())]);
        ledger.follow(&path, Weights::CLAUDE, stamp, usage, no_model);
        assert_eq!(ledger.since(at(-60)), 150.0);
    }

    #[test]
    fn old_entries_are_pruned() {
        let mut ledger = Ledger::default();
        ledger.record(at(0), 10.0, Split::default(), None);
        ledger.record(at(0) - Duration::days(30), 999.0, Split::default(), None);
        ledger.prune(at(0));
        assert_eq!(ledger.since(at(0) - Duration::days(365)), 10.0);
    }
}
