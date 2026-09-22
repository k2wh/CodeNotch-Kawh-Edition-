//! The background polling loop.
//!
//! Runs on Tauri's async runtime, refreshes whichever providers are due, pushes
//! the result to the webview, and decides when something deserves the user's
//! attention (a peek, a notification, or both).

use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use codenotch_core::model::Activity;
use codenotch_core::Alert;

use crate::commands::{AppState, TELEMETRY_EVENT};

/// Never sleep longer than this, so a config change is picked up promptly.
const MAX_SLEEP: Duration = Duration::from_secs(15);

/// How often the cursor is checked against the notch. Fast enough that hover
/// feels immediate, slow enough to be free.
const TRACK_EVERY: Duration = Duration::from_millis(80);
/// Display changes are checked every this many cursor checks (~1 s).
const DISPLAY_CHECK_EVERY: u32 = 12;
/// The foreground app is checked every this many cursor checks (~0.25 s), for
/// the "stay behind" settings and the way back from a ring's app.
const FOREGROUND_CHECK_EVERY: u32 = 3;
/// What the local agents are doing is re-read every this many cursor checks
/// (~2 s). Usage limits stay on their own, much slower, schedule: they come
/// from an API, while activity is files on this machine.
const ACTIVITY_CHECK_EVERY: u32 = 25;
/// Hook reports are picked up every this many cursor checks (~0.5 s). They
/// are the timely signal — an agent saying it has stopped — so they are read
/// far more often than files are.
const HOOK_CHECK_EVERY: u32 = 6;

/// Start the loops. Returns immediately; the work happens on spawned tasks.
pub fn spawn(app: AppHandle) {
    // Hover and peek expiry run on their own timer: tying them to provider
    // polling made the notch take up to 15 s to open under the cursor.
    let tracker = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut ticks: u32 = 0;
        let mut hook_offset: u64 = 0;
        loop {
            tokio::time::sleep(TRACK_EVERY).await;
            let Some(state) = tracker.try_state::<AppState>() else {
                return;
            };
            let _ = state.hud.track();
            ticks = ticks.wrapping_add(1);
            if ticks % FOREGROUND_CHECK_EVERY == 0 {
                let _ = state.hud.check_foreground();
                // Where the user is, so a second click on a ring can take
                // them back there.
                crate::focus::note_foreground();
            }
            if ticks % DISPLAY_CHECK_EVERY == 0 {
                let _ = state.hud.check_display();
            }
            if ticks % HOOK_CHECK_EVERY == 0 {
                deliver_hook_events(&tracker, &mut hook_offset).await;
            } else if ticks % ACTIVITY_CHECK_EVERY == 0 {
                publish_activity(&tracker).await;
            }
        }
    });

    tauri::async_runtime::spawn(async move {
        // A first pass straight away so the HUD isn't empty on launch.
        run_once(&app).await;

        loop {
            let interval = match app.try_state::<AppState>() {
                Some(state) => {
                    let collector = state.collector.lock().await;
                    Duration::from_secs(collector.tick_interval_secs())
                }
                // The app is shutting down.
                None => return,
            };

            tokio::time::sleep(interval.min(MAX_SLEEP)).await;
            run_once(&app).await;
        }
    });
}

/// Take whatever the agents' hooks have reported and act on it.
async fn deliver_hook_events(app: &AppHandle, offset: &mut u64) {
    let events = crate::hooks::drain(offset);
    if events.is_empty() {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    let telemetry = {
        let Ok(mut collector) = state.collector.try_lock() else {
            return; // the events stay in the queue for the next pass
        };
        let now = chrono::Utc::now();
        let mut changed = false;
        for event in events {
            if let (Some(id), Some(activity)) = (event.provider_id(), event.activity()) {
                changed |= collector.report_activity(id, activity, now);
            }
        }
        if !changed {
            return;
        }
        collector.telemetry(now)
    };

    publish(app, &state, telemetry);
}

/// Re-read local agent activity and push it out if anything moved.
///
/// Skips a pass rather than waiting when the collector is busy with a slow
/// network poll — this runs every couple of seconds and there will be another
/// one along shortly.
async fn publish_activity(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    let telemetry = {
        let Ok(mut collector) = state.collector.try_lock() else {
            return;
        };
        let now = chrono::Utc::now();
        if !collector.refresh_activity(now) {
            return;
        }
        collector.telemetry(now)
    };

    publish(app, &state, telemetry);
}

/// One pass: poll what's due, publish it, react to it.
async fn run_once(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };

    // Keep the window on top, even on ticks where no provider was due.
    let _ = state.hud.tick();

    let (telemetry, alerts) = {
        let mut collector = state.collector.lock().await;
        collector.poll(chrono::Utc::now()).await
    };

    publish(app, &state, telemetry);

    for alert in alerts {
        notify(app, &alert);
    }
}

/// Hand a fresh picture to the rest of the app: react to what changed, keep
/// the strip the right length, and tell the webview.
fn publish(app: &AppHandle, state: &AppState, mut telemetry: codenotch_core::model::Telemetry) {
    // What each ring was doing last time, so attention is raised on the
    // transition rather than every pass while something sits blocked. By
    // ring, not by tool: two accounts of one tool are two agents.
    let previous: Vec<(String, Activity)> = state
        .latest
        .lock()
        .ok()
        .map(|t| {
            t.providers
                .iter()
                .map(|p| (p.key.clone(), p.activity))
                .collect()
        })
        .unwrap_or_default();

    // Rings whose agent just answered, or is now waiting on the user...
    let arrived: Vec<_> = telemetry
        .providers
        .iter()
        .filter(|p| {
            matches!(p.activity, Activity::AwaitingInput | Activity::Done)
                && previous
                    .iter()
                    .find(|(key, _)| *key == p.key)
                    .is_some_and(|(_, was)| *was != p.activity)
        })
        .collect();
    // ...and of those, the ones the user is already looking at. Their news is
    // on screen; opening the notch over it would only get in the way.
    let in_front: Vec<String> = arrived
        .iter()
        .filter(|p| crate::focus::in_front(p))
        .map(|p| p.key.clone())
        .collect();

    // The chime itself is played by the webview, which can shape its own
    // sounds and holds it for the same rings; this side surfaces the notch.
    if arrived.iter().any(|p| !in_front.contains(&p.key)) {
        let secs = state.hud.config().peek_secs;
        let _ = state.hud.peek(Duration::from_secs(secs));
    }
    telemetry.in_front = in_front;

    // The strip is one ring per provider that has something to say, so its
    // length follows the collection rather than a fixed guess.
    let rings = telemetry
        .providers
        .iter()
        .filter(|p| p.health != codenotch_core::Health::Unavailable)
        .count();
    let _ = state.hud.set_provider_count(rings);

    if let Ok(mut latest) = state.latest.lock() {
        *latest = telemetry.clone();
    }
    let _ = app.emit(TELEMETRY_EVENT, &telemetry);
}

/// Force a full refresh now (used by the tray's "Refresh now").
pub async fn refresh(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.collector.lock().await.invalidate();
    }
    run_once(app).await;
}

/// Show a desktop notification for a threshold crossing.
///
/// Best-effort: a machine with notifications switched off still gets the ring
/// colour, which is the important part.
fn notify(app: &AppHandle, alert: &Alert) {
    tracing::info!(
        provider = %alert.provider_name,
        window = %alert.window_label,
        pct = alert.used_pct,
        "usage threshold crossed"
    );

    // Also nudge the HUD open, since the number just got interesting.
    if let Some(state) = app.try_state::<AppState>() {
        let secs = state.hud.config().peek_secs;
        let _ = state.hud.peek(Duration::from_secs(secs));
    }
}
