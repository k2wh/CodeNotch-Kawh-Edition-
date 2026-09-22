//! MiniMax coding-plan adapter.
//!
//! MiniMax has three ways in, and only two of them are worth having here: an
//! API key, or a session cookie the user pasted. The third is an in-app
//! browser sign-in, which is a session store and a web view rather than an
//! adapter — and browser cookie databases are never opened, for the same
//! reason as everywhere else in this project: a credential that wasn't handed
//! over wasn't offered.
//!
//! Two regions with separate hosts, chosen by setting rather than guessed,
//! because asking the wrong one returns a perfectly valid "not signed in".

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value as Json;

use crate::adapters::Backoff;
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageWindow};

const TIMEOUT: Duration = Duration::from_secs(15);

/// Which MiniMax to ask. They are separate services with separate accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    International,
    China,
}

impl Region {
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "china" | "cn" | "minimaxi" => Region::China,
            _ => Region::International,
        }
    }

    /// Where a key is accepted, in the order the endpoints moved over time.
    fn key_urls(self) -> [&'static str; 3] {
        match self {
            Region::International => [
                "https://api.minimax.io/v1/token_plan/remains",
                "https://www.minimax.io/v1/token_plan/remains",
                "https://api.minimax.io/v1/api/openplatform/coding_plan/remains",
            ],
            Region::China => [
                "https://api.minimaxi.com/v1/token_plan/remains",
                "https://www.minimaxi.com/v1/token_plan/remains",
                "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains",
            ],
        }
    }

    /// Cookies are scoped to the `www` host, so a session goes there.
    fn cookie_url(self) -> &'static str {
        match self {
            Region::International => {
                "https://www.minimax.io/v1/api/openplatform/coding_plan/remains"
            }
            Region::China => "https://www.minimaxi.com/v1/api/openplatform/coding_plan/remains",
        }
    }
}

/// How to identify ourselves: a key, or a session the user pasted.
#[derive(Debug, Clone, PartialEq)]
pub enum Credential {
    Key(String),
    Cookie(String),
}

/// Read the key or cookie from the environment.
///
/// Nothing is read out of a browser's own cookie store — those weren't handed
/// to us. A cookie header is only used when the user pasted one themselves.
pub fn credentials() -> Option<Credential> {
    for name in [
        "MiniMax_CODING_API_KEY",
        "MINIMAX_CODING_API_KEY",
        "MINIMAX_API_KEY",
    ] {
        if let Some(key) = std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(Credential::Key(key));
        }
    }
    for name in ["MINIMAX_COOKIE", "MINIMAX_COOKIE_HEADER"] {
        if let Some(cookie) = std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(Credential::Cookie(cookie));
        }
    }
    None
}

fn number(value: Option<&Json>) -> Option<f64> {
    match value? {
        Json::Number(n) => n.as_f64(),
        Json::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn int(value: Option<&Json>) -> Option<i64> {
    number(value).map(|n| n as i64)
}

/// Epoch seconds or milliseconds, whichever this field happens to be in.
fn moment(value: Option<&Json>) -> Option<DateTime<Utc>> {
    let raw = number(value).filter(|v| *v > 1_000_000_000.0)?;
    let secs = if raw > 1_000_000_000_000.0 {
        raw / 1000.0
    } else {
        raw
    };
    Utc.timestamp_opt(secs as i64, 0).single()
}

/// One lane's numbers, under whichever spelling this lane uses.
struct Meter {
    total: Option<i64>,
    /// **Remaining**, despite being called `usage_count` on the wire.
    remaining: Option<i64>,
    remaining_percent: Option<f64>,
    status: Option<i64>,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    /// A duration in milliseconds, not a moment.
    remains_ms: Option<f64>,
}

fn meter(lane: &Json, weekly: bool) -> Meter {
    let field = |interval: &str, week: &str| -> Option<&Json> {
        lane.get(if weekly { week } else { interval })
    };
    Meter {
        total: int(field(
            "current_interval_total_count",
            "current_weekly_total_count",
        )),
        remaining: int(field(
            "current_interval_usage_count",
            "current_weekly_usage_count",
        )),
        remaining_percent: number(field(
            "current_interval_remaining_percent",
            "current_weekly_remaining_percent",
        )),
        status: int(field("current_interval_status", "current_weekly_status")),
        start: moment(field("start_time", "weekly_start_time")),
        end: moment(field("end_time", "weekly_end_time")),
        remains_ms: number(field("remains_time", "weekly_remains_time")),
    }
}

/// Only the text lanes; video, speech and image quotas aren't coding plans.
fn is_text_lane(name: Option<&str>) -> bool {
    let Some(name) = name else { return true };
    let name = name.to_ascii_lowercase();
    name == "general"
        || name.contains("text")
        || name.contains("minimax-m")
        || name.starts_with("m2.")
}

/// Turn one lane into a window.
fn window(lane: &Json, weekly: bool, now: DateTime<Utc>) -> Option<UsageWindow> {
    let meter = meter(lane, weekly);
    let (key, label, default_minutes) = if weekly {
        ("weekly", "Weekly limit", 7 * 24 * 60)
    } else {
        ("session", "5h limit", 5 * 60)
    };

    // Status 3 with everything still there is a placeholder row, except on
    // the weekly lane where it means the plan has no weekly ceiling at all.
    let untouched = meter.remaining_percent.is_some_and(|pct| pct >= 100.0);
    if meter.status == Some(3) && untouched {
        if !weekly {
            return None;
        }
        return Some(
            UsageWindow::new(key, "Weekly · unlimited")
                .with_pct(0.0)
                .weekly(true),
        );
    }

    let used_pct = match meter.remaining_percent {
        // The wire reports what is *left*.
        Some(remaining) => ((100.0 - remaining) / 100.0 * 100.0).clamp(0.0, 100.0),
        None => {
            let total = meter.total.filter(|t| *t > 0)?;
            let remaining = meter.remaining?;
            ((total - remaining).max(0) as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
        }
    };

    let resets_at = meter.end.filter(|end| *end > now).or_else(|| {
        let remains = meter.remains_ms.filter(|ms| *ms > 0.0)?;
        Some(now + chrono::Duration::milliseconds(remains as i64))
    });

    let mut window = UsageWindow::new(key, label)
        .with_pct(used_pct as f32)
        .weekly(weekly);
    window.resets_at = resets_at;

    // The plan's own cycle beats the assumed one when both ends are known.
    let minutes = match (meter.start, meter.end) {
        (Some(start), Some(end)) if end > start => (end - start).num_minutes() as u32,
        _ => default_minutes,
    };
    Some(window.lasting(minutes))
}

/// An error MiniMax reports inside an HTTP 200.
fn body_failure(root: &Json) -> Option<Health> {
    let status = root
        .get("base_resp")
        .and_then(|resp| resp.get("status_code").or_else(|| resp.get("code")))
        .and_then(Json::as_i64);
    let message = root
        .get("base_resp")
        .and_then(|resp| resp.get("status_msg").or_else(|| resp.get("msg")))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();

    match status {
        None | Some(0) => None,
        // 1004 is MiniMax's own "not signed in".
        Some(1004) | Some(401) | Some(403) => Some(Health::NeedsAuth),
        Some(_)
            if ["cookie", "log in", "login", "unauthorized"]
                .iter()
                .any(|hint| message.contains(hint)) =>
        {
            Some(Health::NeedsAuth)
        }
        Some(_) => Some(Health::Error),
    }
}

/// Parse a `remains` reply into windows and the plan's name.
pub fn parse_remains(
    body: &str,
    now: DateTime<Utc>,
) -> Result<(Vec<UsageWindow>, Option<String>), Health> {
    let Ok(root) = serde_json::from_str::<Json>(body) else {
        return Err(Health::Error);
    };
    // Some replies wrap everything in `data`, with the envelope's status
    // outside it.
    let payload = root.get("data").filter(|data| data.is_object());
    let merged = payload.unwrap_or(&root);
    if let Some(health) = body_failure(&root).or_else(|| body_failure(merged)) {
        return Err(health);
    }

    let plan = ["current_subscribe_title", "plan_name", "combo_title"]
        .iter()
        .find_map(|name| merged.get(*name).and_then(Json::as_str))
        .map(str::to_string);

    let lanes = merged.get("model_remains").and_then(Json::as_array);
    let lane = lanes
        .into_iter()
        .flatten()
        .find(|lane| is_text_lane(lane.get("model_name").and_then(Json::as_str)));
    let Some(lane) = lane else {
        return Ok((Vec::new(), plan));
    };

    let windows = [false, true]
        .into_iter()
        .filter_map(|weekly| window(lane, weekly, now))
        .collect();
    Ok((windows, plan))
}

/// Reads a MiniMax coding plan from a key or a pasted session.
#[derive(Debug)]
pub struct MiniMaxAdapter {
    http: reqwest::Client,
    backoff: Backoff,
    region: Region,
}

impl MiniMaxAdapter {
    pub fn new(http: reqwest::Client, region: Region) -> Self {
        Self {
            http,
            backoff: Backoff::default(),
            region,
        }
    }

    pub fn set_region(&mut self, region: Region) {
        self.region = region;
    }

    async fn ask(&self, url: &str, creds: &Credential) -> Result<String, Health> {
        let mut request = self
            .http
            .get(url)
            .header("Accept", "application/json")
            .timeout(TIMEOUT);
        request = match creds {
            Credential::Key(key) => request.bearer_auth(key),
            Credential::Cookie(cookie) => request.header("Cookie", cookie),
        };

        let response = request.send().await.map_err(|_| Health::Error)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(Health::NeedsAuth);
        }
        if status.as_u16() == 429 {
            return Err(Health::RateLimited);
        }
        if !status.is_success() {
            return Err(Health::Error);
        }
        response.text().await.map_err(|_| Health::Error)
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(creds) = credentials() else {
            return ProviderSnapshot::degraded(
                ProviderId::MiniMax,
                Health::Unavailable,
                "No MiniMax key — set MINIMAX_API_KEY",
            );
        };
        if !self.backoff.ready_at(now) {
            return ProviderSnapshot::degraded(
                ProviderId::MiniMax,
                Health::RateLimited,
                "MiniMax is rate limiting; waiting before asking again",
            );
        }

        // A key is accepted at more than one address, and which one depends on
        // when the account was created; a cookie only works on its own host.
        let urls: Vec<&str> = match creds {
            Credential::Key(_) => self.region.key_urls().to_vec(),
            Credential::Cookie(_) => vec![self.region.cookie_url()],
        };

        let mut last = Health::Error;
        for url in urls {
            match self.ask(url, &creds).await {
                Ok(body) => match parse_remains(&body, now) {
                    Ok((windows, plan)) if !windows.is_empty() => {
                        self.backoff.record_success();
                        let mut snapshot = ProviderSnapshot::new(ProviderId::MiniMax);
                        snapshot.source = Some("coding plan".to_string());
                        snapshot.account = plan;
                        snapshot.windows = windows;
                        snapshot.updated_at = now;
                        return snapshot;
                    }
                    Ok(_) => last = Health::Ok,
                    Err(health) => last = health,
                },
                Err(health) => last = health,
            }
        }

        if matches!(last, Health::Error | Health::RateLimited) {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
        }
        ProviderSnapshot::degraded(
            ProviderId::MiniMax,
            last,
            match last {
                Health::NeedsAuth => "MiniMax rejected the key or session",
                Health::RateLimited => "MiniMax is rate limiting",
                Health::Ok => "No coding plan metered on this MiniMax account",
                _ => "MiniMax could not be read",
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        Utc.timestamp_opt(1_800_000_000, 0).unwrap()
    }

    /// The field is called `usage_count`, and it is what is *left*. Reading it
    /// as spending shows a plan at 10% as a plan at 90%.
    #[test]
    fn the_counts_are_what_remains_not_what_was_spent() {
        let body = format!(
            r#"{{"base_resp": {{"status_code": 0}}, "current_subscribe_title": "Max",
               "model_remains": [{{"model_name": "general",
                 "current_interval_total_count": 1000,
                 "current_interval_usage_count": 900,
                 "start_time": {}, "end_time": {}}}]}}"#,
            now().timestamp() - 3600,
            now().timestamp() + 3600
        );
        let (windows, plan) = parse_remains(&body, now()).unwrap();
        assert_eq!(plan.as_deref(), Some("Max"));
        // 900 of 1000 left is 10% used.
        assert_eq!(windows[0].used_pct, Some(10.0));
    }

    #[test]
    fn a_remaining_percentage_is_flipped_too() {
        let body = r#"{"model_remains": [{"model_name": "general",
            "current_interval_remaining_percent": 75.0}]}"#;
        let (windows, _) = parse_remains(body, now()).unwrap();
        assert_eq!(windows[0].used_pct, Some(25.0));
    }

    /// MiniMax answers a dead session with HTTP 200 and 1004 in the body.
    #[test]
    fn an_error_inside_a_200_is_still_an_error() {
        assert_eq!(
            parse_remains(r#"{"base_resp": {"status_code": 1004}}"#, now()),
            Err(Health::NeedsAuth)
        );
        assert_eq!(
            parse_remains(
                r#"{"base_resp": {"status_code": 2013, "status_msg": "bad request"}}"#,
                now()
            ),
            Err(Health::Error)
        );
    }

    /// A message about signing in is an auth failure whatever its code.
    #[test]
    fn a_login_message_is_read_as_signed_out() {
        assert_eq!(
            parse_remains(
                r#"{"base_resp": {"status_code": 99, "status_msg": "please log in"}}"#,
                now()
            ),
            Err(Health::NeedsAuth)
        );
    }

    #[test]
    fn video_and_speech_lanes_are_not_coding_plans() {
        let body = r#"{"model_remains": [
            {"model_name": "video-01", "current_interval_remaining_percent": 10.0},
            {"model_name": "general", "current_interval_remaining_percent": 90.0}
        ]}"#;
        let (windows, _) = parse_remains(body, now()).unwrap();
        assert_eq!(
            windows[0].used_pct,
            Some(10.0),
            "the text lane, not the video one"
        );
    }

    /// `remains_time` is how long is left, not when it ends.
    #[test]
    fn a_duration_is_not_a_moment() {
        let body = r#"{"model_remains": [{"model_name": "general",
            "current_interval_remaining_percent": 50.0, "remains_time": 3600000}]}"#;
        let (windows, _) = parse_remains(body, now()).unwrap();
        assert_eq!(
            windows[0].resets_at,
            Some(now() + chrono::Duration::hours(1))
        );
    }

    #[test]
    fn an_unlimited_week_is_drawn_empty_rather_than_full() {
        let body = r#"{"model_remains": [{"model_name": "general",
            "current_weekly_status": 3, "current_weekly_remaining_percent": 100.0,
            "current_interval_remaining_percent": 40.0}]}"#;
        let (windows, _) = parse_remains(body, now()).unwrap();
        let weekly = windows.iter().find(|w| w.key == "weekly").unwrap();
        assert_eq!(weekly.used_pct, Some(0.0));
        assert!(weekly.label.contains("unlimited"));
    }

    #[test]
    fn the_region_is_chosen_not_guessed() {
        assert_eq!(Region::from_setting("china"), Region::China);
        assert_eq!(Region::from_setting(""), Region::International);
        assert!(Region::China.key_urls()[0].contains("minimaxi.com"));
        assert!(Region::International.key_urls()[0].contains("minimax.io"));
    }
}
