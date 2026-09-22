//! How many tokens a limit is worth.
//!
//! Providers report how much of a window is gone as a percentage, never what
//! the window holds. The ledger knows what has been spent in the same period,
//! so the two together give the size of the limit — and with it an answer to
//! "how much have I got left, in tokens?"
//!
//! One division would be a poor way to get there. A percentage arrives
//! rounded, so early in a window a point or two of rounding is most of the
//! reading; and the notch only sees what this machine spent, so a session on
//! the phone quietly inflates the answer. Two things make it hold up:
//!
//! 1. **Within a window**, every (spent, percent) pair is kept and a line is
//!    fitted through the origin. Rounding noise averages out as the window
//!    fills, and the fit leans on the later, larger readings.
//! 2. **Across windows**, each finished window contributes its own fit to a
//!    running average, weighted by how far that window actually got — a
//!    window that reached 60% says far more than one that reached 4%.
//!
//! Windows themselves move (they roll, and plans change), which is why the
//! identity of a window here is when it resets: a new reset time starts a new
//! fit and retires the old one into the average.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Below this, a window's fit is too rounding-bound to mean anything — and
/// that cuts both ways: it is no basis for learning from, and no basis for
/// showing either. A reported 3% is anywhere from 2.5 to 3.5, so dividing by
/// it puts the answer out by a fifth in either direction.
const MIN_USEFUL_PCT: f64 = 8.0;
/// How much of the running average one finished window may displace.
const MAX_BLEND: f64 = 0.4;

/// The fit for the window in progress.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Live {
    /// The window's reset time: its identity.
    resets_at: Option<DateTime<Utc>>,
    /// Σ(tokens × fraction) and Σ(fraction²) for a least-squares line through
    /// the origin, where fraction is percent/100.
    sum_tp: f64,
    sum_pp: f64,
    /// The furthest this window got, which is how much its fit is trusted.
    peak_pct: f64,
}

impl Live {
    fn capacity(&self) -> Option<f64> {
        (self.sum_pp > 0.0).then(|| self.sum_tp / self.sum_pp)
    }
}

/// What past windows have taught us.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Learned {
    /// Input-equivalent tokens at 100%.
    capacity: f64,
    /// How many windows have gone into it, for a gentler early average.
    windows: u32,
}

/// One provider's window, e.g. `claudeCode/five_hour`.
type Key = String;

/// Learns what each window is worth, and keeps learning as they roll.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Estimator {
    live: BTreeMap<Key, Live>,
    learned: BTreeMap<Key, Learned>,
}

/// What the notch shows for a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Input-equivalent tokens spent in this window.
    pub used: f64,
    /// Input-equivalent tokens the window holds, at 100%.
    pub capacity: f64,
}

impl Estimator {
    /// Record where a window stands, and return what it is worth.
    ///
    /// `used` is what the ledger says has been spent inside this window.
    pub fn observe(
        &mut self,
        key: &str,
        resets_at: Option<DateTime<Utc>>,
        pct: f64,
        used: f64,
    ) -> Option<Estimate> {
        let live = self.live.entry(key.to_string()).or_default();

        // A different reset time means a different window: retire the old fit
        // into the average before starting a fresh one.
        if live.resets_at != resets_at {
            let finished = std::mem::take(live);
            live.resets_at = resets_at;
            if let (Some(capacity), true) =
                (finished.capacity(), finished.peak_pct >= MIN_USEFUL_PCT)
            {
                self.blend(key, capacity, finished.peak_pct);
            }
        }

        let live = self.live.entry(key.to_string()).or_default();
        if pct > 0.0 && used > 0.0 {
            let fraction = pct / 100.0;
            live.sum_tp += used * fraction;
            live.sum_pp += fraction * fraction;
            live.peak_pct = live.peak_pct.max(pct);
        }

        self.capacity_for(key)
            .map(|capacity| Estimate { used, capacity })
    }

    /// What a window is worth: the live fit once it means something, otherwise
    /// what earlier windows taught us — and nothing at all until one of the
    /// two does. An early window has no answer worth printing, and a made-up
    /// one is worse than a blank.
    pub fn capacity_for(&self, key: &str) -> Option<f64> {
        let live = self
            .live
            .get(key)
            .filter(|l| l.peak_pct >= MIN_USEFUL_PCT)
            .and_then(Live::capacity);
        let learned = self.learned.get(key).map(|l| l.capacity);

        let capacity = match (live, learned) {
            // Both: lean on the live fit, which is about *this* window.
            (Some(now), Some(before)) => now * 0.7 + before * 0.3,
            (Some(now), None) => now,
            (None, Some(before)) => before,
            (None, None) => return None,
        };
        (capacity > 0.0).then_some(capacity)
    }

    /// Fold a finished window's fit into the running average.
    fn blend(&mut self, key: &str, capacity: f64, peak_pct: f64) {
        let learned = self.learned.entry(key.to_string()).or_default();
        if learned.windows == 0 {
            learned.capacity = capacity;
            learned.windows = 1;
            return;
        }
        // Weight by how far the window got, capped so one window can't take
        // over, and gentler as the average gathers more of them.
        let share = (peak_pct / 100.0).min(MAX_BLEND) / (1.0 + learned.windows as f64 * 0.25);
        learned.capacity = learned.capacity * (1.0 - share) + capacity * share;
        learned.windows = learned.windows.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(mins: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(1_800_000_000 + mins * 60, 0)
    }

    #[test]
    fn a_window_that_has_barely_started_says_nothing() {
        let mut est = Estimator::default();
        // 1% in: a rounded point could mean anything.
        assert!(est
            .observe("claude/five_hour", at(300), 1.0, 2_000_000.0)
            .is_none());
    }

    #[test]
    fn a_rounding_bound_window_says_nothing_either() {
        let mut est = Estimator::default();
        // 3% could be 2.5 or 3.49: the same spend would give a capacity from
        // 540M to 756M. Better to say nothing than to pick one.
        est.observe("claude/five_hour", at(300), 2.0, 12_000_000.0);
        assert!(est
            .observe("claude/five_hour", at(300), 3.0, 18_900_000.0)
            .is_none());
    }

    #[test]
    fn a_window_in_progress_estimates_its_own_size() {
        let mut est = Estimator::default();
        // A 200M-token window, seen at 10% and again at 20%.
        est.observe("claude/five_hour", at(300), 10.0, 20_000_000.0);
        let estimate = est
            .observe("claude/five_hour", at(300), 20.0, 40_000_000.0)
            .expect("enough to go on");
        assert!((estimate.capacity - 200_000_000.0).abs() < 1_000_000.0);
        assert_eq!(estimate.used, 40_000_000.0);
    }

    #[test]
    fn later_readings_outweigh_early_rounding() {
        let mut est = Estimator::default();
        // 4% reported for what is really 5% of a 100M window: a big relative
        // error early on...
        est.observe("claude/five_hour", at(300), 4.0, 5_000_000.0);
        // ...that a reading at 50% should wash out.
        let estimate = est
            .observe("claude/five_hour", at(300), 50.0, 50_000_000.0)
            .unwrap();
        assert!(
            (estimate.capacity - 100_000_000.0).abs() < 8_000_000.0,
            "got {}",
            estimate.capacity
        );
    }

    #[test]
    fn a_new_window_keeps_what_the_last_one_taught() {
        let mut est = Estimator::default();
        est.observe("claude/five_hour", at(300), 50.0, 100_000_000.0);

        // The window rolls: a fresh one, barely started, still has an answer.
        let estimate = est
            .observe("claude/five_hour", at(600), 1.0, 1_000_000.0)
            .expect("the previous window is still worth something");
        assert!((estimate.capacity - 200_000_000.0).abs() < 20_000_000.0);
    }

    #[test]
    fn one_odd_window_cannot_take_over_the_average() {
        let mut est = Estimator::default();
        // Three consistent windows at 200M.
        for w in 0..3 {
            est.observe("claude/five_hour", at(300 * w), 60.0, 120_000_000.0);
        }
        // Then one that looks half the size, because half the work happened
        // on another machine.
        est.observe("claude/five_hour", at(1200), 60.0, 60_000_000.0);
        let estimate = est
            .observe("claude/five_hour", at(1500), 1.0, 500_000.0)
            .unwrap();
        assert!(
            estimate.capacity > 150_000_000.0,
            "one outlier moved it to {}",
            estimate.capacity
        );
    }
}
