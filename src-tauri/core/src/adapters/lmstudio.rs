//! LM Studio adapter.
//!
//! A local runtime rather than a plan: there is no quota to burn and no reset
//! to count down to, so the rings it produces are context, never alarm. What
//! it reports is which models are loaded, how big they are and how much
//! context each was given — the same shape as Ollama's.
//!
//! LM Studio ships with authentication off, so no token is sent unless one
//! was set. Its address is read from its own settings file, because the port
//! is configurable and guessing 1234 would quietly show nothing to anyone who
//! changed it.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::home_dir;
use crate::model::{
    Activity, Health, ProviderId, ProviderSnapshot, Session, UsageUnit, UsageWindow,
};

/// LM Studio's own default, used when its settings say nothing.
const DEFAULT_ADDRESS: &str = "http://127.0.0.1:1234";
/// A local server either answers at once or isn't there.
const TIMEOUT: Duration = Duration::from_secs(3);

/// One model instance the server has loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModel {
    /// The instance's own id — a model loaded twice is two of these.
    pub id: String,
    pub size: Option<u64>,
    pub context_length: Option<u64>,
    pub quantization: Option<String>,
}

impl LoadedModel {
    fn detail(&self) -> String {
        let mut parts = Vec::new();
        if let Some(size) = self.size {
            parts.push(format!("{:.1} GB", size as f64 / 1_073_741_824.0));
        }
        if let Some(context) = self.context_length {
            parts.push(format!("{}k context", context / 1000));
        }
        if let Some(quantization) = &self.quantization {
            parts.push(quantization.clone());
        }
        parts.join(" · ")
    }
}

/// Where LM Studio says it is listening.
///
/// Loopback only, and the port comes from its settings rather than from a
/// guess: the default is configurable, and a notch that silently shows
/// nothing is worse than one that says the server isn't there.
pub fn address() -> String {
    configured_address().unwrap_or_else(|| DEFAULT_ADDRESS.to_string())
}

fn configured_address() -> Option<String> {
    let path: PathBuf = home_dir()?
        .join(".lmstudio")
        .join(".internal")
        .join("http-server-config.json");
    let root: Json = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let port = root.get("port")?.as_u64()?;
    (1..=65535)
        .contains(&port)
        .then(|| format!("http://127.0.0.1:{port}"))
}

/// Whether LM Studio is installed at all.
pub fn installed() -> bool {
    home_dir().is_some_and(|home| home.join(".lmstudio").is_dir())
}

/// Read the loaded instances out of `/api/v1/models`.
///
/// Embedding models are dropped: they are loaded, but they are not what
/// anyone means by "what have I got running".
pub fn parse_models(body: &str) -> Vec<LoadedModel> {
    let Ok(root) = serde_json::from_str::<Json>(body) else {
        return Vec::new();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();

    for model in root
        .get("models")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        if model.get("type").and_then(Json::as_str) != Some("llm") {
            continue;
        }
        let size = model.get("size_bytes").and_then(Json::as_u64);
        let fallback_context = model.get("max_context_length").and_then(Json::as_u64);
        let quantization = model
            .get("quantization")
            .and_then(|q| q.get("name"))
            .and_then(Json::as_str)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty());

        for instance in model
            .get("loaded_instances")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = instance.get("id").and_then(Json::as_str) else {
                continue;
            };
            let id = id.trim().to_string();
            if id.is_empty() || seen.contains(&id) {
                continue;
            }
            seen.push(id.clone());
            out.push(LoadedModel {
                id,
                size,
                context_length: instance
                    .get("config")
                    .and_then(|c| c.get("context_length"))
                    .and_then(Json::as_u64)
                    .or(fallback_context),
                quantization: quantization.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Build the snapshot from what is loaded.
pub fn to_snapshot(models: &[LoadedModel]) -> ProviderSnapshot {
    if models.is_empty() {
        let mut snapshot = ProviderSnapshot::new(ProviderId::LmStudio).with_source("api/v1/models");
        snapshot.detail = Some("Running, no models loaded".into());
        return snapshot;
    }

    let total: u64 = models.iter().filter_map(|m| m.size).sum();
    // A count with no denominator: nothing here is a quota, so there is no
    // percentage to draw and no threshold to cross.
    let windows = vec![UsageWindow::new("resident", "Loaded")
        .with_counts(total as f64, None)
        .with_unit(UsageUnit::Bytes)
        .informational()];

    let sessions = models
        .iter()
        .map(|model| Session {
            id: model.id.clone(),
            title: model.id.clone(),
            cwd: None,
            model: Some(model.id.clone()),
            // Loaded is not the same as working, and a spinning arc on an idle
            // machine would say it was.
            activity: Activity::Idle,
            last_activity: None,
            tokens: model.context_length,
            detail: Some(model.detail()),
            host: None,
        })
        .collect::<Vec<_>>();

    ProviderSnapshot::new(ProviderId::LmStudio)
        .with_source("api/v1/models")
        .with_windows(windows)
        .with_sessions(sessions)
}

/// Reads what LM Studio has loaded.
#[derive(Debug)]
pub struct LmStudioAdapter {
    http: reqwest::Client,
}

impl LmStudioAdapter {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    pub async fn collect(&mut self, now: DateTime<Utc>) -> ProviderSnapshot {
        if !installed() {
            return ProviderSnapshot::degraded(
                ProviderId::LmStudio,
                Health::Unavailable,
                "LM Studio not found",
            );
        }

        // Only set when the user turned authentication on; LM Studio ships
        // with it off and refuses a malformed header.
        let token = std::env::var("LM_API_TOKEN")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let mut request = self
            .http
            .get(format!("{}/api/v1/models", address()))
            .header("Accept", "application/json")
            .timeout(TIMEOUT);
        if let Some(token) = &token {
            request = request.bearer_auth(token);
        }

        let response = match request.send().await {
            Ok(response) => response,
            // Not an error: the app simply isn't running, which is the normal
            // state of a local runtime most of the day.
            Err(_) => {
                return ProviderSnapshot::degraded(
                    ProviderId::LmStudio,
                    Health::Unavailable,
                    "LM Studio's server isn't running",
                )
            }
        };

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return ProviderSnapshot::degraded(
                ProviderId::LmStudio,
                Health::NeedsAuth,
                "LM Studio wants an API token — set LM_API_TOKEN",
            );
        }
        if !status.is_success() {
            return ProviderSnapshot::degraded(
                ProviderId::LmStudio,
                Health::Error,
                format!("LM Studio's server returned {status}"),
            );
        }

        let Ok(body) = response.text().await else {
            return ProviderSnapshot::degraded(
                ProviderId::LmStudio,
                Health::Error,
                "LM Studio's reply could not be read",
            );
        };

        let mut snapshot = to_snapshot(&parse_models(&body));
        snapshot.updated_at = now;
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"models": [
      {"type": "llm", "key": "qwen/qwen3-coder-30b", "size_bytes": 18253611008,
       "max_context_length": 262144, "quantization": {"name": "Q4_K_M"},
       "loaded_instances": [{"id": "qwen3-coder-30b", "config": {"context_length": 32768}}]},
      {"type": "embedding", "key": "nomic-embed", "size_bytes": 274000000,
       "loaded_instances": [{"id": "nomic-embed"}]},
      {"type": "llm", "key": "unloaded-model", "size_bytes": 900, "loaded_instances": []}
    ]}"#;

    #[test]
    fn only_loaded_language_models_count() {
        let models = parse_models(SAMPLE);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "qwen3-coder-30b");
    }

    /// The instance's own context wins over the model's ceiling: what matters
    /// is what this copy was actually given.
    #[test]
    fn the_instances_context_beats_the_models_maximum() {
        assert_eq!(parse_models(SAMPLE)[0].context_length, Some(32768));
    }

    #[test]
    fn the_same_model_loaded_twice_is_two_entries() {
        let models = parse_models(
            r#"{"models": [{"type": "llm", "key": "m", "loaded_instances": [
              {"id": "one"}, {"id": "two"}, {"id": "one"}
            ]}]}"#,
        );
        assert_eq!(models.len(), 2, "and the repeat of an id is not a third");
    }

    /// Nothing here is a quota, so nothing here may colour a ring as one.
    #[test]
    fn what_is_loaded_is_context_not_a_limit() {
        let snapshot = to_snapshot(&parse_models(SAMPLE));
        assert!(snapshot.windows.iter().all(|w| w.informational));
        assert!(snapshot.windows.iter().all(|w| w.used_pct.is_none()));
        assert_eq!(snapshot.sessions.len(), 1);
    }

    #[test]
    fn a_server_with_nothing_loaded_says_so() {
        let snapshot = to_snapshot(&[]);
        assert_eq!(snapshot.health, Health::Ok);
        assert!(snapshot.windows.is_empty());
        assert!(snapshot.detail.is_some());
    }

    #[test]
    fn a_reply_of_the_wrong_shape_loads_nothing() {
        assert!(parse_models("nonsense").is_empty());
        assert!(parse_models(r#"{"models": "not a list"}"#).is_empty());
    }
}
