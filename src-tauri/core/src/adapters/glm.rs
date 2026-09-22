//! GLM (Z.ai coding plan) adapter.
//!
//! GLM has no client of its own: it is used *through* other tools, which keep
//! its key in their own config. So this borrows the key from whichever of them
//! has one — Claude Code pointed at z.ai, ZCode, or OpenCode — and asks z.ai's
//! monitor endpoint what is left.
//!
//! Borrowing a key means being careful whose key it is. Claude Code's config
//! holds a token and a base URL, and the token is only z.ai's if the base URL
//! is: pointed at Anthropic, the same field holds an Anthropic key, and
//! sending that to z.ai would be handing one vendor another's credential.

use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value as Json;

use crate::adapters::{home_dir, Backoff};
use crate::model::{Health, ProviderId, ProviderSnapshot, UsageWindow};

const TIMEOUT: Duration = Duration::from_secs(15);
/// Tokens whose value is still encrypted at rest; ZCode alone can read them.
const ENCRYPTED: &str = "enc:v1:";

/// A key, and the console it belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    pub token: String,
    /// `https://api.z.ai` or `https://open.bigmodel.cn`.
    pub base: String,
    /// Which tool's config it came from, for the card.
    pub source: &'static str,
}

/// Whether a host belongs to z.ai or its mainland sibling.
fn is_zai_host(host: &str) -> bool {
    host == "api.z.ai"
        || host.ends_with(".z.ai")
        || host == "open.bigmodel.cn"
        || host.ends_with(".bigmodel.cn")
}

/// The console for a host: the two regions have separate ones.
fn console_for(host: &str) -> String {
    if host.ends_with("bigmodel.cn") {
        "https://open.bigmodel.cn".to_string()
    } else {
        "https://api.z.ai".to_string()
    }
}

fn read_json(path: std::path::PathBuf) -> Option<Json> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn non_empty(value: Option<&Json>) -> Option<String> {
    let text = value?.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Claude Code's settings, but only when it is pointed at z.ai.
fn from_claude_code(root: &Json) -> Option<Credential> {
    let env = root.get("env")?;
    let token = non_empty(env.get("ANTHROPIC_AUTH_TOKEN"))
        .or_else(|| non_empty(env.get("ANTHROPIC_API_KEY")))?;
    let base = non_empty(env.get("ANTHROPIC_BASE_URL"))?;
    // Only the host matters, and it decides whether this key is even z.ai's.
    let host = base
        .split("://")
        .nth(1)?
        .split('/')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    is_zai_host(&host).then(|| Credential {
        token,
        base: console_for(&host),
        source: "Claude Code",
    })
}

/// ZCode's provider list: the entry whose id names a coding plan.
fn from_zcode_config(root: &Json) -> Option<Credential> {
    let providers = root.get("provider")?.as_object()?;
    // Sorted, so which entry wins doesn't depend on map order.
    let mut ids: Vec<&String> = providers.keys().collect();
    ids.sort();
    for id in ids {
        if !id.contains("coding-plan") {
            continue;
        }
        let entry = &providers[id];
        if entry.get("enabled") == Some(&Json::Bool(false)) {
            continue;
        }
        let options = entry.get("options")?;
        let Some(token) = non_empty(options.get("apiKey")) else {
            continue;
        };
        let base = non_empty(options.get("baseURL"))
            .and_then(|url| {
                let host = url.split("://").nth(1)?.split('/').next()?.to_string();
                Some(console_for(&host))
            })
            .unwrap_or_else(|| {
                if id.starts_with("zhipu") {
                    "https://open.bigmodel.cn".to_string()
                } else {
                    "https://api.z.ai".to_string()
                }
            });
        return Some(Credential {
            token,
            base,
            source: "ZCode",
        });
    }
    None
}

/// ZCode's OAuth store. An encrypted value is skipped rather than guessed at.
fn from_zcode_credentials(root: &Json) -> Option<Credential> {
    let token = non_empty(root.get("oauth:zai:access_token"))?;
    (!token.starts_with(ENCRYPTED)).then(|| Credential {
        token,
        base: "https://api.z.ai".to_string(),
        source: "ZCode",
    })
}

/// OpenCode's auth file, under any of the names z.ai goes by there.
fn from_opencode(root: &Json) -> Option<Credential> {
    const NAMES: &[&str] = &[
        "zai-coding-plan",
        "zai",
        "z-ai",
        "z.ai",
        "glm",
        "zhipu",
        "zhipuai",
    ];
    for name in NAMES {
        let Some(entry) = root.get(*name) else {
            continue;
        };
        let token = match entry {
            Json::String(text) if !text.trim().is_empty() => text.trim().to_string(),
            Json::Object(_) => {
                let keys = [
                    "apiKey",
                    "api_key",
                    "token",
                    "key",
                    "accessToken",
                    "auth_token",
                ];
                match keys.iter().find_map(|k| non_empty(entry.get(*k))) {
                    Some(token) => token,
                    None => continue,
                }
            }
            _ => continue,
        };
        let base = if name.starts_with("zhipu") {
            "https://open.bigmodel.cn".to_string()
        } else {
            "https://api.z.ai".to_string()
        };
        return Some(Credential {
            token,
            base,
            source: "OpenCode",
        });
    }
    None
}

/// The first key any installed tool is holding for z.ai.
pub fn credentials() -> Option<Credential> {
    let home = home_dir()?;
    let claude = home.join(".claude").join("settings.json");
    let zcode_config = home.join(".zcode").join("v2").join("config.json");
    let zcode_creds = home.join(".zcode").join("v2").join("credentials.json");
    let opencode = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json");

    read_json(claude)
        .as_ref()
        .and_then(from_claude_code)
        .or_else(|| read_json(zcode_config).as_ref().and_then(from_zcode_config))
        .or_else(|| {
            read_json(zcode_creds)
                .as_ref()
                .and_then(from_zcode_credentials)
        })
        .or_else(|| read_json(opencode).as_ref().and_then(from_opencode))
}

/// z.ai answers failures with HTTP 200 and a code in the body.
fn body_failure(root: &Json) -> Option<Health> {
    let ok = root.get("success").and_then(Json::as_bool).unwrap_or(false)
        || root.get("code").is_none()
        || root.get("code").and_then(Json::as_i64) == Some(200);
    if ok {
        return None;
    }
    Some(match root.get("code").and_then(Json::as_i64) {
        Some(401) | Some(403) => Health::NeedsAuth,
        Some(429) => Health::RateLimited,
        _ => Health::Error,
    })
}

/// `unit` is a code, not a duration: 3 counts hours, 6 counts weeks.
fn window_minutes(unit: Option<i64>, number: Option<i64>) -> Option<u32> {
    let number = number?;
    match unit? {
        3 => Some((number * 60) as u32),
        6 => Some((number * 7 * 24 * 60) as u32),
        _ => None,
    }
}

/// Identity by shape rather than by `type`: a token plan calls its rows
/// `TOKENS_LIMIT` and a credit plan `CREDIT_LIMIT`, and both mean the same
/// five hours.
fn window_key(limit: &Json) -> String {
    if limit.get("type").and_then(Json::as_str) == Some("TIME_LIMIT") {
        return "mcp".to_string();
    }
    let unit = limit.get("unit").and_then(Json::as_i64);
    let number = limit.get("number").and_then(Json::as_i64);
    match (unit, number) {
        (Some(3), Some(5)) => "session".to_string(),
        (Some(6), Some(1)) => "weekly".to_string(),
        (Some(unit), Some(number)) => format!("window-{unit}x{number}"),
        _ => limit
            .get("type")
            .and_then(Json::as_str)
            .unwrap_or("unknown")
            .to_ascii_lowercase(),
    }
}

fn window_label(key: &str, unit: Option<i64>, number: Option<i64>) -> String {
    match key {
        "session" => "Current session".to_string(),
        "weekly" => "Weekly".to_string(),
        "mcp" => "MCP (1 month)".to_string(),
        _ => match (unit, number) {
            (Some(3), Some(n)) => format!("Usage ({n} h)"),
            (Some(6), Some(n)) => format!("Usage ({n} wk)"),
            _ => "Usage".to_string(),
        },
    }
}

/// Turn the monitor reply into windows, ordered session → weekly → the rest.
pub fn parse_quota(body: &str) -> Result<Vec<UsageWindow>, Health> {
    let Ok(root) = serde_json::from_str::<Json>(body) else {
        return Err(Health::Error);
    };
    if let Some(health) = body_failure(&root) {
        return Err(health);
    }

    let mut windows: Vec<UsageWindow> = Vec::new();
    let limits = root
        .get("data")
        .and_then(|data| data.get("limits"))
        .and_then(Json::as_array);
    for limit in limits.into_iter().flatten() {
        // A row without a percentage has nothing to draw; the counts beside it
        // are in units the plan never states.
        let Some(percentage) = limit.get("percentage").and_then(Json::as_f64) else {
            continue;
        };
        let unit = limit.get("unit").and_then(Json::as_i64);
        let number = limit.get("number").and_then(Json::as_i64);
        let key = window_key(limit);
        let label = window_label(&key, unit, number);

        let mut window = UsageWindow::new(key.clone(), label)
            .with_pct(percentage.clamp(0.0, 100.0) as f32)
            .weekly(key == "weekly");
        // Milliseconds here, where most of the others use seconds.
        window.resets_at = limit
            .get("nextResetTime")
            .and_then(Json::as_f64)
            .filter(|ms| *ms > 0.0)
            .and_then(|ms| Utc.timestamp_millis_opt(ms as i64).single());
        if let Some(minutes) = window_minutes(unit, number) {
            window = window.lasting(minutes);
        }
        windows.push(window);
    }

    let rank = |key: &str| match key {
        "session" => 0,
        "weekly" => 1,
        "mcp" => 2,
        _ => 3,
    };
    windows.sort_by_key(|w| rank(&w.key));
    Ok(windows)
}

/// Reads a z.ai coding plan through whichever tool holds its key.
#[derive(Debug)]
pub struct GlmAdapter {
    http: reqwest::Client,
    backoff: Backoff,
}

impl GlmAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            backoff: Backoff::default(),
        }
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        let Some(creds) = credentials() else {
            return ProviderSnapshot::degraded(
                ProviderId::Glm,
                Health::Unavailable,
                "No z.ai key found in Claude Code, ZCode or OpenCode",
            );
        };
        if !self.backoff.ready_at(now) {
            return ProviderSnapshot::degraded(
                ProviderId::Glm,
                Health::RateLimited,
                "z.ai is rate limiting; waiting before asking again",
            );
        }

        let url = format!("{}/api/monitor/usage/quota/limit", creds.base);
        // The token goes in bare: prefixing it with `Bearer` is rejected.
        let response = self
            .http
            .get(&url)
            .header("Authorization", &creds.token)
            .header("Content-Type", "application/json")
            .timeout(TIMEOUT)
            .send()
            .await;

        let response = match response {
            Ok(response) => response,
            Err(err) => {
                self.backoff
                    .record_failure(now, None, rand::random::<f64>());
                return ProviderSnapshot::degraded(
                    ProviderId::Glm,
                    Health::Error,
                    format!("Could not reach z.ai: {err}"),
                );
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return ProviderSnapshot::degraded(
                ProviderId::Glm,
                Health::NeedsAuth,
                format!("z.ai rejected the key from {}", creds.source),
            );
        }
        if !status.is_success() {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::Glm,
                if status.as_u16() == 429 {
                    Health::RateLimited
                } else {
                    Health::Error
                },
                format!("z.ai's monitor endpoint returned {status}"),
            );
        }

        let Ok(body) = response.text().await else {
            self.backoff
                .record_failure(now, None, rand::random::<f64>());
            return ProviderSnapshot::degraded(
                ProviderId::Glm,
                Health::Error,
                "z.ai's reply could not be read",
            );
        };

        match parse_quota(&body) {
            Err(health) => {
                if health != Health::NeedsAuth {
                    self.backoff
                        .record_failure(now, None, rand::random::<f64>());
                }
                ProviderSnapshot::degraded(
                    ProviderId::Glm,
                    health,
                    match health {
                        Health::NeedsAuth => format!("z.ai rejected the key from {}", creds.source),
                        _ => "z.ai's reply could not be understood".to_string(),
                    },
                )
            }
            Ok(windows) if windows.is_empty() => ProviderSnapshot::degraded(
                ProviderId::Glm,
                Health::Ok,
                "No coding plan metered on this z.ai account",
            ),
            Ok(windows) => {
                self.backoff.record_success();
                let mut snapshot = ProviderSnapshot::new(ProviderId::Glm);
                snapshot.source = Some(format!("key from {}", creds.source));
                snapshot.account = serde_json::from_str::<Json>(&body).ok().and_then(|root| {
                    root.get("data")
                        .and_then(|d| d.get("level"))
                        .and_then(Json::as_str)
                        .map(str::to_string)
                });
                snapshot.windows = windows;
                snapshot.updated_at = now;
                snapshot
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same field holds an Anthropic key when Claude Code is pointed at
    /// Anthropic, and sending that to z.ai would hand over the wrong vendor's
    /// credential.
    #[test]
    fn claude_codes_key_is_only_taken_when_it_points_at_zai() {
        let anthropic: Json = serde_json::from_str(
            r#"{"env": {"ANTHROPIC_AUTH_TOKEN": "sk-ant", "ANTHROPIC_BASE_URL": "https://api.anthropic.com"}}"#,
        )
        .unwrap();
        assert!(from_claude_code(&anthropic).is_none());

        let zai: Json = serde_json::from_str(
            r#"{"env": {"ANTHROPIC_AUTH_TOKEN": "zai-key", "ANTHROPIC_BASE_URL": "https://api.z.ai/api/anthropic"}}"#,
        )
        .unwrap();
        let creds = from_claude_code(&zai).unwrap();
        assert_eq!(creds.token, "zai-key");
        assert_eq!(creds.base, "https://api.z.ai");
    }

    #[test]
    fn the_mainland_console_is_kept_apart() {
        let cn: Json = serde_json::from_str(
            r#"{"env": {"ANTHROPIC_API_KEY": "k", "ANTHROPIC_BASE_URL": "https://open.bigmodel.cn/api/anthropic"}}"#,
        )
        .unwrap();
        assert_eq!(
            from_claude_code(&cn).unwrap().base,
            "https://open.bigmodel.cn"
        );
    }

    #[test]
    fn a_disabled_or_unrelated_zcode_provider_is_skipped() {
        let root: Json = serde_json::from_str(
            r#"{"provider": {
              "openai": {"options": {"apiKey": "not-ours"}},
              "zai-coding-plan": {"enabled": false, "options": {"apiKey": "off"}},
              "zzz-coding-plan": {"options": {"apiKey": "ours"}}
            }}"#,
        )
        .unwrap();
        assert_eq!(from_zcode_config(&root).unwrap().token, "ours");
    }

    #[test]
    fn an_encrypted_token_is_left_alone() {
        let root: Json =
            serde_json::from_str(r#"{"oauth:zai:access_token": "enc:v1:abc"}"#).unwrap();
        assert!(from_zcode_credentials(&root).is_none());
    }

    #[test]
    fn opencode_entries_may_be_a_string_or_an_object() {
        let bare: Json = serde_json::from_str(r#"{"zai-coding-plan": "plain"}"#).unwrap();
        assert_eq!(from_opencode(&bare).unwrap().token, "plain");
        let wrapped: Json = serde_json::from_str(r#"{"glm": {"api_key": "wrapped"}}"#).unwrap();
        assert_eq!(from_opencode(&wrapped).unwrap().token, "wrapped");
    }

    /// z.ai answers a dead token with HTTP 200 and the code in the body.
    #[test]
    fn an_error_inside_a_200_is_still_an_error() {
        assert_eq!(
            parse_quota(r#"{"code": 401, "success": false}"#),
            Err(Health::NeedsAuth)
        );
        assert_eq!(
            parse_quota(r#"{"code": 500, "success": false}"#),
            Err(Health::Error)
        );
    }

    #[test]
    fn the_two_lanes_are_recognised_by_shape() {
        let windows = parse_quota(
            r#"{"success": true, "data": {"level": "pro", "limits": [
              {"type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 42.0, "nextResetTime": 1758326400000},
              {"type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 17.5, "nextResetTime": 1758326400000},
              {"type": "TIME_LIMIT", "percentage": 3.0}
            ]}}"#,
        )
        .unwrap();

        // Sorted session first, whatever order they arrived in.
        assert_eq!(windows[0].key, "session");
        assert_eq!(windows[0].window_minutes, Some(300));
        assert_eq!(windows[0].used_pct, Some(17.5));
        assert_eq!(windows[1].key, "weekly");
        assert!(windows[1].weekly);
        assert_eq!(windows[1].window_minutes, Some(7 * 24 * 60));
        // The MCP row has no reset of its own, and is kept anyway.
        assert_eq!(windows[2].key, "mcp");
        assert_eq!(windows[2].resets_at, None);
    }

    #[test]
    fn a_row_without_a_percentage_is_dropped() {
        let windows = parse_quota(
            r#"{"success": true, "data": {"limits": [{"unit": 3, "number": 5, "total": 100}]}}"#,
        )
        .unwrap();
        assert!(windows.is_empty());
    }
}
