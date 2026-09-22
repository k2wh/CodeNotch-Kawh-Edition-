//! Ollama adapter.
//!
//! Unlike the hosted providers there is no quota here — the interesting
//! resource is the machine's own memory. `GET /api/ps` lists the models
//! currently resident, how much of each sits in VRAM versus system RAM, and
//! when Ollama will evict them.
//!
//! A refused connection just means Ollama isn't running, which is
//! [`Health::Unavailable`] rather than an error worth colouring the HUD for.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use crate::adapters::parse_timestamp;
use crate::model::{
    Activity, Health, ProviderId, ProviderSnapshot, Session, UsageUnit, UsageWindow,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// One resident model.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModel {
    pub name: String,
    /// Total size of the loaded model, in bytes.
    pub size: Option<u64>,
    /// How much of that is on the GPU.
    pub size_vram: Option<u64>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
    pub context_length: Option<u64>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl LoadedModel {
    /// Fraction of the model sitting in VRAM, 0..=100.
    ///
    /// This is the number that tells you whether a model is running on the GPU
    /// or has partly spilled to system RAM, which is the difference between
    /// fast and unusable.
    pub fn gpu_fraction(&self) -> Option<f32> {
        let size = self.size.filter(|s| *s > 0)?;
        let vram = self.size_vram?;
        Some(((vram as f64 / size as f64) * 100.0).clamp(0.0, 100.0) as f32)
    }

    /// "8.2 GB · 92% GPU · Q4_K_M"
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if let Some(size) = self.size {
            parts.push(human_bytes(size));
        }
        match self.gpu_fraction() {
            Some(pct) if pct >= 99.5 => parts.push("GPU".into()),
            Some(pct) if pct <= 0.5 => parts.push("CPU".into()),
            Some(pct) => parts.push(format!("{pct:.0}% GPU")),
            None => {}
        }
        if let Some(q) = &self.quantization {
            parts.push(q.clone());
        }
        parts.join(" · ")
    }
}

/// Format a byte count the way a developer reads it.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Parse the `/api/ps` response.
pub fn parse_ps(raw: &str) -> Vec<LoadedModel> {
    let Ok(json) = serde_json::from_str::<Json>(raw) else {
        return Vec::new();
    };
    let Some(models) = json.get("models").and_then(Json::as_array) else {
        return Vec::new();
    };

    models
        .iter()
        .filter_map(|m| {
            let name = m
                .get("name")
                .or_else(|| m.get("model"))
                .and_then(Json::as_str)?
                .to_string();

            let details = m.get("details");
            Some(LoadedModel {
                name,
                size: m.get("size").and_then(Json::as_u64),
                size_vram: m.get("size_vram").and_then(Json::as_u64),
                parameter_size: details
                    .and_then(|d| d.get("parameter_size"))
                    .and_then(Json::as_str)
                    .map(str::to_string),
                quantization: details
                    .and_then(|d| d.get("quantization_level"))
                    .and_then(Json::as_str)
                    .map(str::to_string),
                context_length: m
                    .get("context_length")
                    .or_else(|| m.get("num_ctx"))
                    .and_then(Json::as_u64),
                expires_at: m.get("expires_at").and_then(parse_timestamp),
            })
        })
        .collect()
}

/// Build the snapshot's windows and sessions from the resident models.
pub fn to_snapshot(models: &[LoadedModel], version: Option<&str>) -> ProviderSnapshot {
    if models.is_empty() {
        let mut snap = ProviderSnapshot::new(ProviderId::Ollama)
            .with_source("api/ps")
            .with_account(version.map(|v| format!("v{v}")));
        snap.detail = Some("Running, no models loaded".into());
        return snap;
    }

    let total: u64 = models.iter().filter_map(|m| m.size).sum();
    let vram: u64 = models.iter().filter_map(|m| m.size_vram).sum();

    let mut windows = vec![
        // No denominator exists for "how much memory could I use", so this is a
        // count with no percentage rather than a made-up ring.
        UsageWindow::new("resident", "Resident")
            .with_counts(total as f64, None)
            .with_unit(UsageUnit::Bytes)
            .informational(),
    ];
    if total > 0 {
        windows.push(
            UsageWindow::new("vram", "On GPU")
                .with_pct(((vram as f64 / total as f64) * 100.0) as f32)
                // More on the GPU is better, so this must not turn the ring red.
                .informational(),
        );
    }

    let sessions = models
        .iter()
        .map(|m| Session {
            id: m.name.clone(),
            title: m.name.clone(),
            cwd: None,
            model: Some(m.name.clone()),
            // A resident model is loaded, not necessarily generating; claiming
            // otherwise would put a spinning arc on an idle machine.
            activity: Activity::Idle,
            last_activity: None,
            tokens: m.context_length,
            detail: Some(m.detail()),
            host: None,
        })
        .collect::<Vec<_>>();

    ProviderSnapshot::new(ProviderId::Ollama)
        .with_source("api/ps")
        .with_account(version.map(|v| format!("v{v}")))
        .with_windows(windows)
        .with_sessions(sessions)
}

/// Collects Ollama state over its local HTTP API.
pub struct OllamaAdapter {
    http: reqwest::Client,
    base_url: String,
}

impl OllamaAdapter {
    pub fn new(http: reqwest::Client, base_url: impl Into<String>) -> Self {
        Self {
            http,
            base_url: base_url.into(),
        }
    }

    pub fn set_base_url(&mut self, url: impl Into<String>) {
        self.base_url = url.into();
    }

    async fn get(&self, path: &str) -> anyhow::Result<String> {
        let url = format!("{}{path}", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .get(&url)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await?
            .error_for_status()?;
        Ok(response.text().await?)
    }

    pub async fn collect(&mut self, _now: DateTime<Utc>) -> ProviderSnapshot {
        let body = match self.get("/api/ps").await {
            Ok(body) => body,
            Err(err) => {
                // Not running is the common case and isn't a problem.
                return ProviderSnapshot::degraded(
                    ProviderId::Ollama,
                    Health::Unavailable,
                    if err.is::<reqwest::Error>() {
                        "Ollama not running".to_string()
                    } else {
                        err.to_string()
                    },
                );
            }
        };

        let version = self
            .get("/api/version")
            .await
            .ok()
            .and_then(|raw| serde_json::from_str::<Json>(&raw).ok())
            .and_then(|j| j.get("version").and_then(Json::as_str).map(str::to_string));

        to_snapshot(&parse_ps(&body), version.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS_RESPONSE: &str = r#"{
      "models": [
        {
          "name": "llama3.1:8b",
          "model": "llama3.1:8b",
          "size": 6000000000,
          "size_vram": 6000000000,
          "context_length": 8192,
          "expires_at": "2026-01-01T12:05:00Z",
          "details": {"parameter_size": "8.0B", "quantization_level": "Q4_K_M"}
        },
        {
          "name": "qwen2.5-coder:32b",
          "size": 20000000000,
          "size_vram": 10000000000,
          "details": {"parameter_size": "32B", "quantization_level": "Q4_0"}
        }
      ]
    }"#;

    #[test]
    fn parses_loaded_models() {
        let models = parse_ps(PS_RESPONSE);
        assert_eq!(models.len(), 2);

        assert_eq!(models[0].name, "llama3.1:8b");
        assert_eq!(models[0].size, Some(6_000_000_000));
        assert_eq!(models[0].quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(models[0].context_length, Some(8192));
        assert_eq!(
            models[0].expires_at,
            Some(
                DateTime::parse_from_rfc3339("2026-01-01T12:05:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[test]
    fn gpu_fraction_reflects_partial_offload() {
        let models = parse_ps(PS_RESPONSE);
        assert_eq!(models[0].gpu_fraction(), Some(100.0));
        assert_eq!(models[1].gpu_fraction(), Some(50.0));

        // Missing sizes must not produce a bogus 0%.
        let m = LoadedModel {
            name: "x".into(),
            size: None,
            size_vram: Some(1),
            parameter_size: None,
            quantization: None,
            context_length: None,
            expires_at: None,
        };
        assert_eq!(m.gpu_fraction(), None);
    }

    #[test]
    fn detail_lines_read_well() {
        let models = parse_ps(PS_RESPONSE);
        assert_eq!(models[0].detail(), "5.6 GB · GPU · Q4_K_M");
        assert_eq!(models[1].detail(), "18.6 GB · 50% GPU · Q4_0");

        let cpu_only = LoadedModel {
            name: "x".into(),
            size: Some(1024 * 1024),
            size_vram: Some(0),
            parameter_size: None,
            quantization: None,
            context_length: None,
            expires_at: None,
        };
        assert_eq!(cpu_only.detail(), "1.0 MB · CPU");
    }

    #[test]
    fn byte_formatting_is_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(6_000_000_000), "5.6 GB");
        assert_eq!(human_bytes(0), "0 B");
        // Three digits drop the decimal so the label stays narrow.
        assert_eq!(human_bytes(500 * 1024 * 1024), "500 MB");
    }

    #[test]
    fn a_malformed_or_empty_response_yields_no_models() {
        assert!(parse_ps("not json").is_empty());
        assert!(parse_ps("{}").is_empty());
        assert!(parse_ps(r#"{"models":[]}"#).is_empty());
        // Entries with no name can't be shown.
        assert!(parse_ps(r#"{"models":[{"size":1}]}"#).is_empty());
    }

    #[test]
    fn snapshot_summarises_memory_across_models() {
        let snap = to_snapshot(&parse_ps(PS_RESPONSE), Some("0.5.7"));
        assert_eq!(snap.health, Health::Ok);
        assert_eq!(snap.account.as_deref(), Some("v0.5.7"));

        assert_eq!(snap.windows[0].label, "Resident");
        assert_eq!(snap.windows[0].used, Some(26_000_000_000.0));
        assert_eq!(
            snap.windows[0].used_pct, None,
            "there is no total-memory denominator to divide by"
        );

        assert_eq!(snap.windows[1].label, "On GPU");
        assert!(
            snap.windows.iter().all(|w| w.informational),
            "memory readings are context, not a quota"
        );
        assert_eq!(snap.peak_pct(), None, "Ollama has no limit to be near");
        let on_gpu = snap.windows[1].used_pct.expect("GPU share");
        assert!(
            (on_gpu - 61.538_46).abs() < 0.001,
            "16 GB of 26 GB resident is ~61.5%, got {on_gpu}"
        );

        assert_eq!(snap.sessions.len(), 2);
        assert_eq!(
            snap.sessions[0].detail.as_deref(),
            Some("5.6 GB · GPU · Q4_K_M")
        );
    }

    #[test]
    fn a_resident_model_is_not_reported_as_generating() {
        let snap = to_snapshot(&parse_ps(PS_RESPONSE), None);
        assert_eq!(
            snap.activity,
            Activity::Idle,
            "loaded is not the same as busy"
        );
    }

    #[test]
    fn running_with_nothing_loaded_says_so() {
        let snap = to_snapshot(&[], Some("0.5.7"));
        assert_eq!(snap.health, Health::Ok);
        assert!(snap.windows.is_empty());
        assert_eq!(snap.detail.as_deref(), Some("Running, no models loaded"));
    }

    #[tokio::test]
    async fn an_unreachable_daemon_is_unavailable_not_an_error() {
        let mut adapter = OllamaAdapter::new(
            reqwest::Client::new(),
            // Reserved-for-testing port that nothing should be listening on.
            "http://127.0.0.1:1",
        );
        let snap = adapter.collect(Utc::now()).await;
        assert_eq!(snap.health, Health::Unavailable);
        assert!(snap.windows.is_empty());
    }
}
