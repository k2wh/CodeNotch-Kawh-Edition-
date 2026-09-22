//! Tauri commands the webview calls, plus the shared application state.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{Emitter, Manager, State};
use tokio::sync::Mutex as AsyncMutex;

use codenotch_core::config::{Config, Edge, HudMetrics};
use codenotch_core::model::{host_app_id, ProviderId, Telemetry};
use codenotch_core::Collector;

use crate::focus;
use crate::hud::{Hud, HudState, Regions};
use crate::platform;

/// Event carrying a fresh telemetry payload to the webview.
pub const TELEMETRY_EVENT: &str = "codenotch://telemetry";
/// Event carrying updated settings to the webview.
pub const CONFIG_EVENT: &str = "codenotch://config";

/// Everything the commands need. Held in Tauri's state.
pub struct AppState {
    pub hud: Arc<Hud>,
    pub collector: Arc<AsyncMutex<Collector>>,
    /// Latest telemetry, so a reloading webview gets data without waiting for
    /// the next poll.
    pub latest: Arc<Mutex<Telemetry>>,
}

/// The payload a freshly loaded webview needs to render immediately.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub config: Config,
    pub telemetry: Telemetry,
    pub hud: HudState,
    /// Strip and popover dimensions, so the webview draws to the same numbers
    /// the window is sized with rather than its own copy of them.
    pub metrics: HudMetrics,
    pub edge: Edge,
    pub version: String,
    /// False on non-Windows dev builds, where the Win32 layer is a no-op.
    pub native_window: bool,
}

/// Called once the webview has mounted.
#[tauri::command]
pub fn hud_ready(state: State<'_, AppState>) -> Result<Bootstrap, String> {
    state.hud.apply().map_err(|e| e.to_string())?;

    Ok(Bootstrap {
        config: state.hud.config(),
        telemetry: state.latest.lock().map_err(|e| e.to_string())?.clone(),
        hud: state.hud.state(),
        metrics: state.hud.metrics(),
        edge: state.hud.edge(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        native_window: cfg!(windows),
    })
}

/// Pointer entered or left the notch.
#[tauri::command]
pub fn hud_hover(hovering: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.hud.set_hover(hovering).map_err(|e| e.to_string())
}

/// The webview laid out the strip (and maybe a card): where they are drives
/// hover and the window's click region.
#[tauri::command]
pub fn hud_set_regions(regions: Regions, state: State<'_, AppState>) -> Result<(), String> {
    state.hud.set_regions(regions).map_err(|e| e.to_string())
}

/// A field or native picker gained or lost focus.
#[tauri::command]
pub fn hud_set_interactive(on: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.hud.set_interactive(on).map_err(|e| e.to_string())
}

/// Toggle "stay expanded".
#[tauri::command]
pub fn hud_toggle_pin(state: State<'_, AppState>) -> Result<bool, String> {
    state.hud.toggle_pin().map_err(|e| e.to_string())
}

/// Current settings.
#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    state.hud.config()
}

/// Current strip dimensions. The webview refetches these after a size change,
/// since the bootstrap copy only describes the size it started with.
#[tauri::command]
pub fn get_metrics(state: State<'_, AppState>) -> HudMetrics {
    state.hud.metrics()
}

/// Persist new settings and apply them everywhere.
#[tauri::command]
pub async fn set_config(config: Config, app: tauri::AppHandle) -> Result<Config, String> {
    apply_config(&app, config).await
}

/// Serialises config changes, so overlapping saves (a dragged slider, a colour
/// picker) are written and applied in the order they were made.
static CONFIG_WRITE: AsyncMutex<()> = AsyncMutex::const_new(());
/// The newest config the collector hasn't picked up yet. See [`apply_config`].
static COLLECTOR_PENDING: Mutex<Option<Config>> = Mutex::new(None);

/// The one path every settings change goes through: the settings panel, the
/// tray's show/hide, and a second launch un-hiding the notch.
///
/// Anything that changes the config without coming through here leaves the
/// webview holding a stale copy, and its next save quietly reverts the change.
pub async fn apply_config(app: &tauri::AppHandle, config: Config) -> Result<Config, String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Err("app is shutting down".into());
    };
    let _serial = CONFIG_WRITE.lock().await;

    // Save first: if this fails the user should hear about it rather than see
    // a setting silently revert on the next launch.
    config.save().map_err(|e| e.to_string())?;

    // Only touch the Run key when the setting actually changed, not on every
    // unrelated save.
    let previous = state.hud.config();
    if cfg!(windows) && previous.launch_at_login != config.launch_at_login {
        platform::set_launch_at_login(config.launch_at_login).map_err(|e| e.to_string())?;
    }

    state
        .hud
        .set_config(config.clone())
        .map_err(|e| e.to_string())?;

    // Tell the webview straight away. The collector below can be busy with a
    // slow network poll for seconds, and the UI shouldn't wait on it.
    let _ = app.emit(CONFIG_EVENT, &config);
    crate::tray::sync(app, &config);

    // Hand the collector the newest config without holding up this call. Only
    // the latest pending one is applied, so a burst of changes can't land out
    // of order even though each hand-off waits on the collector separately.
    if let Ok(mut pending) = COLLECTOR_PENDING.lock() {
        *pending = Some(config.clone());
    }
    let collector = state.collector.clone();
    tauri::async_runtime::spawn(async move {
        let mut collector = collector.lock().await;
        let next = COLLECTOR_PENDING.lock().ok().and_then(|mut p| p.take());
        if let Some(next) = next {
            collector.set_config(next);
        }
    });

    Ok(config)
}

/// Show or hide the notch through [`apply_config`], so the webview and the
/// tray hear about it too.
pub async fn set_hidden_everywhere(app: &tauri::AppHandle, hidden: bool) -> Result<(), String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    let mut config = state.hud.config();
    if config.hidden == hidden {
        return Ok(());
    }
    config.hidden = hidden;
    apply_config(app, config).await.map(|_| ())
}

/// Collect every provider right now, rather than waiting for the schedule.
#[tauri::command]
pub async fn refresh_now(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<Telemetry, String> {
    let mut collector = state.collector.lock().await;
    collector.invalidate();
    let (telemetry, _alerts) = collector.poll(chrono::Utc::now()).await;
    drop(collector);

    if let Ok(mut latest) = state.latest.lock() {
        *latest = telemetry.clone();
    }
    let _ = app.emit(TELEMETRY_EVENT, &telemetry);
    Ok(telemetry)
}

/// Bring a provider's window to the front — or, when it is already there, go
/// back to the window the user was in before it, so a ring works as a switch.
///
/// `host` is where the session runs, as its transcript recorded it (see
/// `Session::host`): a Claude Code session in the Claude app raises the Claude
/// app, not whichever editor happens to be open. When that app isn't running,
/// or the origin is unknown, the provider's general list is tried instead.
///
/// Returns false when nothing matching is running, so the UI can say so instead
/// of appearing to do nothing.
#[tauri::command]
pub fn focus_provider(
    provider: ProviderId,
    title_hint: Option<String>,
    host: Option<String>,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    // Collapse first: the card is about to be behind whatever we raise.
    let _ = state.hud.set_hover(false);

    let hint = title_hint.as_deref();
    let host = host.as_deref();

    // 1. The session's own app when it says where it runs, the usual suspects
    //    when it doesn't — if a window of it is open.
    if let Some(window) = focus::target(provider, hint, host) {
        // The second half of a double click: the first half did the work.
        if focus::repeated(window) {
            return Ok(true);
        }
        // Already there: this click is the way back.
        if focus::is_current(window) {
            return Ok(focus::go_back(window));
        }
        if platform::bring_to_front(window) {
            return Ok(true);
        }
    }
    // 2. A desktop app that's closed: open it, rather than raising some other
    //    app the session has nothing to do with.
    if host
        .and_then(host_app_id)
        .is_some_and(platform::activate_app)
    {
        return Ok(true);
    }
    // 3. Its app isn't installed: the usual suspects.
    Ok(platform::provider_window(provider.focus_processes(), hint)
        .is_some_and(platform::bring_to_front))
}

/// Hide the HUD (the tray icon stays).
#[tauri::command]
pub async fn set_hidden(hidden: bool, app: tauri::AppHandle) -> Result<(), String> {
    set_hidden_everywhere(&app, hidden).await
}

/// Briefly expand the HUD.
#[tauri::command]
pub fn peek(seconds: Option<u64>, state: State<'_, AppState>) -> Result<(), String> {
    let secs = seconds.unwrap_or(5).clamp(1, 60);
    state
        .hud
        .peek(Duration::from_secs(secs))
        .map_err(|e| e.to_string())
}

/// Monitors available to pin to, for the settings UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub index: usize,
    pub label: String,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

#[tauri::command]
pub fn list_monitors() -> Vec<MonitorInfo> {
    let primary = platform::primary_work_area();
    platform::monitors()
        .into_iter()
        .enumerate()
        .map(|(index, area)| MonitorInfo {
            index,
            label: format!("Display {} · {}×{}", index + 1, area.width, area.height),
            width: area.width,
            height: area.height,
            primary: Some(area) == primary,
        })
        .collect()
}

/// Install or remove the agent hooks, and say whether they are in place.
///
/// This writes to files the user owns (`~/.claude/settings.json`,
/// `~/.codex/hooks.json`), so it only ever happens on their say-so, merges
/// with whatever else is in there, and removes cleanly.
#[tauri::command]
pub fn set_agent_hooks(enabled: bool) -> Result<bool, String> {
    crate::hooks::set_installed(enabled).map_err(|e| e.to_string())?;
    Ok(crate::hooks::installed())
}

/// Whether the agents are currently reporting to us.
#[tauri::command]
pub fn agent_hooks_installed() -> bool {
    crate::hooks::installed()
}

/// Apps with an open window, for the "stay behind these apps" picker.
#[tauri::command]
pub fn list_open_apps() -> Vec<String> {
    platform::open_apps()
}

/// An open window for the picker, with its thumbnail as a data URL.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenWindow {
    pub id: String,
    pub title: String,
    /// Executable name, e.g. `chrome.exe`; the picker selects by app.
    pub exe: String,
    pub thumbnail: Option<String>,
    pub minimized: bool,
}

/// Thumbnail width in physical pixels: about twice the card's CSS width, so
/// it stays sharp at 150–200% scaling.
const THUMB_WIDTH: i32 = 300;

/// Every open app window with a thumbnail, for the "stay behind" picker.
///
/// Capturing takes a moment per window, so it runs off the async runtime.
#[tauri::command]
pub async fn list_open_windows() -> Vec<OpenWindow> {
    use base64::Engine;

    let shots = tauri::async_runtime::spawn_blocking(|| platform::open_windows(THUMB_WIDTH))
        .await
        .unwrap_or_default();

    shots
        .into_iter()
        .map(|shot| OpenWindow {
            id: shot.id.to_string(),
            title: shot.title,
            exe: shot.image,
            thumbnail: shot.thumbnail.map(|png| {
                format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(png)
                )
            }),
            minimized: shot.minimized,
        })
        .collect()
}

/// Quit the application.
#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

/// Open the config file's folder so the user can hand-edit it.
#[tauri::command]
pub fn open_config_dir(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = Config::path().map_err(|e| e.to_string())?;
    let dir = path
        .parent()
        .ok_or_else(|| "config path has no parent".to_string())?;
    // Make sure it exists, or the shell will just beep at the user.
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// Convenience for the tray and the poll loop.
pub fn app_state(app: &tauri::AppHandle) -> Option<State<'_, AppState>> {
    app.try_state::<AppState>()
}
