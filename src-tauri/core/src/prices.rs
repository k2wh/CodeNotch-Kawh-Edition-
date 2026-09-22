//! What a model's tokens cost at list price.
//!
//! A subscription is a flat fee for work that the API would bill by the token,
//! so the only way to say what a plan is returning is to price the tokens as
//! if they had been bought. These are the published per-million rates for the
//! first-party APIs, in USD.
//!
//! Model names arrive from transcripts, where they carry date suffixes
//! (`claude-opus-5-20260115`), bare aliases (`sonnet`) and occasional
//! placeholders (`<synthetic>`). Matching is therefore by substring against a
//! table ordered specific-first, ending in family-wide catch-alls so a model
//! released after this table was written is still priced like its siblings
//! rather than silently counted as free.

/// Per-million-token rates for one model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    /// Writing to the prompt cache for five minutes. OpenAI doesn't charge
    /// for this separately, so for those models it is the input rate.
    pub cache_write: f64,
    /// Writing to it for an hour, which Anthropic charges at twice base
    /// input against 1.25x for the five-minute write. Claude Code uses the
    /// one-hour cache heavily, and the transcripts report the two separately,
    /// so pricing them the same would understate a session by half of its
    /// largest line. Unused for models whose transcripts don't distinguish.
    pub cache_write_1h: f64,
    /// Reading from it — a tenth of input on most models.
    pub cache_read: f64,
}

impl Price {
    const fn new(input: f64, output: f64, cache_write: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_write,
            cache_write_1h: input * 2.0,
            cache_read,
        }
    }

    /// What this many tokens would have cost, in USD.
    pub fn usd(&self, usage: &crate::ledger::Usage) -> f64 {
        (usage.input as f64 * self.input
            + usage.output as f64 * self.output
            + usage.cache_write as f64 * self.cache_write
            + usage.cache_write_1h as f64 * self.cache_write_1h
            + usage.cache_read as f64 * self.cache_read)
            / 1_000_000.0
    }
}

/// Substring, then what it costs. Order is significant: the first match wins,
/// so `claude-fable-5-1` has to come before `claude-fable-5`, and the bare
/// family names come last.
const TABLE: &[(&str, Price)] = &[
    // Anthropic — platform.claude.com/docs/en/about-claude/pricing
    ("claude-fable-5-1", Price::new(10.0, 50.0, 12.50, 0.25)),
    ("claude-mythos-5-1", Price::new(10.0, 50.0, 12.50, 0.25)),
    ("claude-fable-5", Price::new(10.0, 50.0, 12.50, 1.0)),
    ("claude-mythos-5", Price::new(10.0, 50.0, 12.50, 1.0)),
    // Opus 5.5 costs less than the 5 before it, and its cache reads are a
    // twentieth of input where every other model here charges a tenth.
    ("claude-opus-5-5", Price::new(4.0, 20.0, 5.0, 0.20)),
    ("claude-opus-5", Price::new(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-8", Price::new(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-7", Price::new(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-6", Price::new(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-5", Price::new(5.0, 25.0, 6.25, 0.50)),
    ("claude-opus-4-1", Price::new(15.0, 75.0, 18.75, 1.50)),
    ("claude-opus-4", Price::new(15.0, 75.0, 18.75, 1.50)),
    ("claude-sonnet-5", Price::new(2.0, 10.0, 2.50, 0.20)),
    ("claude-sonnet-4", Price::new(3.0, 15.0, 3.75, 0.30)),
    ("claude-haiku-4", Price::new(1.0, 5.0, 1.25, 0.10)),
    ("claude-haiku-3", Price::new(0.80, 4.0, 1.0, 0.08)),
    // OpenAI — developers.openai.com/api/docs/pricing
    ("gpt-6-astra", Price::new(10.0, 50.0, 10.0, 1.0)),
    ("gpt-5.6-sol", Price::new(4.0, 20.0, 4.0, 0.40)),
    ("gpt-5.6-terra", Price::new(2.0, 12.0, 2.0, 0.20)),
    ("gpt-5.6-luna", Price::new(0.20, 1.20, 0.20, 0.02)),
    ("gpt-5.5-pro", Price::new(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5.5", Price::new(5.0, 30.0, 5.0, 0.50)),
    ("gpt-5.4-mini", Price::new(0.75, 4.50, 0.75, 0.075)),
    ("gpt-5.4-nano", Price::new(0.20, 1.25, 0.20, 0.02)),
    ("gpt-5.4-pro", Price::new(30.0, 180.0, 30.0, 30.0)),
    ("gpt-5.4", Price::new(2.50, 15.0, 2.50, 0.25)),
    ("gpt-5.3-codex", Price::new(1.75, 14.0, 1.75, 0.175)),
    ("gpt-5.2-pro", Price::new(21.0, 168.0, 21.0, 21.0)),
    ("gpt-5.2", Price::new(1.75, 14.0, 1.75, 0.175)),
    ("gpt-5.1", Price::new(1.25, 10.0, 1.25, 0.125)),
    ("gpt-5-pro", Price::new(15.0, 120.0, 15.0, 15.0)),
    ("gpt-5-mini", Price::new(0.25, 2.0, 0.25, 0.025)),
    ("gpt-5-nano", Price::new(0.05, 0.40, 0.05, 0.005)),
    ("gpt-5", Price::new(1.25, 10.0, 1.25, 0.125)),
    ("o4-mini", Price::new(1.10, 4.40, 1.10, 0.275)),
    ("o3-mini", Price::new(1.10, 4.40, 1.10, 0.55)),
    ("o3-pro", Price::new(20.0, 80.0, 20.0, 20.0)),
    ("o3", Price::new(2.0, 8.0, 2.0, 0.50)),
    ("o1-pro", Price::new(150.0, 600.0, 150.0, 150.0)),
    ("o1", Price::new(15.0, 60.0, 15.0, 7.50)),
    // Families, for names this table hasn't met yet: the newest of each, which
    // is what a bare name most likely resolves to.
    ("opus", Price::new(4.0, 20.0, 5.0, 0.20)),
    ("sonnet", Price::new(2.0, 10.0, 2.50, 0.20)),
    ("haiku", Price::new(1.0, 5.0, 1.25, 0.10)),
    ("codex", Price::new(1.75, 14.0, 1.75, 0.175)),
    ("gpt", Price::new(1.25, 10.0, 1.25, 0.125)),
];

/// What one model's tokens are worth, if this is a model we can price.
///
/// `None` covers the placeholders transcripts carry for messages no model
/// produced — `<synthetic>` and the like — which cost nothing and should not
/// be counted as though they did.
pub fn for_model(model: Option<&str>) -> Option<Price> {
    let name = model?.trim().to_ascii_lowercase();
    if name.is_empty() || name.starts_with('<') {
        return None;
    }
    TABLE
        .iter()
        .find(|(pattern, _)| name.contains(pattern))
        .map(|(_, price)| *price)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dated_model_name_still_matches() {
        let price = for_model(Some("claude-opus-5-20260115")).unwrap();
        assert_eq!(price.input, 5.0);
        assert_eq!(price.output, 25.0);
    }

    #[test]
    fn the_more_specific_name_wins() {
        // `claude-fable-5-1` contains `claude-fable-5`, and they differ on
        // cache reads: 0.25 against 1.00.
        assert_eq!(
            for_model(Some("claude-fable-5-1")).unwrap().cache_read,
            0.25
        );
        assert_eq!(for_model(Some("claude-fable-5")).unwrap().cache_read, 1.0);
        // Same trap on the OpenAI side.
        assert_eq!(for_model(Some("gpt-5.5-pro")).unwrap().output, 180.0);
        assert_eq!(for_model(Some("gpt-5.5")).unwrap().output, 30.0);
    }

    #[test]
    fn a_bare_alias_is_priced_as_the_current_model() {
        assert_eq!(for_model(Some("sonnet")).unwrap().input, 2.0);
        assert_eq!(for_model(Some("opus")).unwrap().input, 4.0);
    }

    #[test]
    fn opus_5_5_is_read_before_the_5_whose_name_it_starts_with() {
        let new = for_model(Some("claude-opus-5-5-20260301")).unwrap();
        assert_eq!(new.input, 4.0);
        assert_eq!(new.output, 20.0);
        assert_eq!(new.cache_write, 5.0);
        assert_eq!(new.cache_write_1h, 8.0);
        assert_eq!(new.cache_read, 0.20);

        // The one before it keeps its own, dearer rates.
        let before = for_model(Some("claude-opus-5-20260115")).unwrap();
        assert_eq!(before.input, 5.0);
        assert_eq!(before.cache_read, 0.50);
    }

    #[test]
    fn nothing_is_charged_for_what_no_model_produced() {
        assert!(for_model(Some("<synthetic>")).is_none());
        assert!(for_model(Some("")).is_none());
        assert!(for_model(None).is_none());
    }

    #[test]
    fn a_model_released_after_this_table_is_priced_like_its_family() {
        let unheard_of = for_model(Some("claude-opus-9-20281231")).unwrap();
        assert_eq!(unheard_of.input, 4.0);
    }

    #[test]
    fn an_hour_of_opus_costs_what_the_page_says() {
        let opus = for_model(Some("claude-opus-5")).unwrap();
        // The worked example from the pricing page: 50k in, 15k out.
        let spent = crate::ledger::Usage {
            input: 50_000,
            output: 15_000,
            ..Default::default()
        };
        assert!((opus.usd(&spent) - 0.625).abs() < 1e-9);
        // And with 40k of the input served from cache.
        let cached = crate::ledger::Usage {
            input: 10_000,
            output: 15_000,
            cache_read: 40_000,
            ..Default::default()
        };
        assert!((opus.usd(&cached) - 0.445).abs() < 1e-9);
    }

    #[test]
    fn an_hour_long_cache_write_costs_twice_base_input() {
        let opus = for_model(Some("claude-opus-5")).unwrap();
        // $5 base input: $6.25 for five minutes, $10 for an hour.
        assert_eq!(opus.cache_write, 6.25);
        assert_eq!(opus.cache_write_1h, 10.0);

        let hour = crate::ledger::Usage {
            cache_write_1h: 1_000_000,
            ..Default::default()
        };
        let five = crate::ledger::Usage {
            cache_write: 1_000_000,
            ..Default::default()
        };
        assert_eq!(opus.usd(&hour), 10.0);
        assert_eq!(opus.usd(&five), 6.25);
    }
}
