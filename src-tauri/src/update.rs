//! Updates: a new release found on GitHub, fetched in the background, and
//! installed when the user clicks the update ring.
//!
//! The updater plugin does the parts that have to be exactly right: reading
//! the release's `latest.json`, downloading, and checking the download against
//! the public key in `tauri.conf.json` before anything runs, so a file that
//! wasn't signed with this project's key is refused (the private half never
//! leaves the maintainer's machine — see `packaging/sign-release.sh`). This
//! module decides when to ask, keeps what it found, and tells the webview,
//! which draws it as one more ring on the strip.
//!
//! Nothing installs on its own. `auto` downloads in the background and
//! `notify` only says a release is out; either way the app restarts only when
//! the user clicks.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::commands::AppState;

/// Event carrying the update's state to the webview.
pub const UPDATE_EVENT: &str = "codenotch://update";

/// The first look waits for the notch to settle, and for a laptop that has
/// just woken to find its network.
const FIRST_CHECK: Duration = Duration::from_secs(45);
/// Then every six hours. Releases are days apart, and each look is one
/// request to GitHub.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How often the ring hears about download progress: smooth enough to watch,
/// without an event per network chunk.
const PROGRESS_EVERY: Duration = Duration::from_millis(150);
/// Release notes longer than this are cut: the card shows their first lines.
const NOTES_MAX: usize = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Nothing to show: up to date, or not looked yet.
    #[default]
    Idle,
    /// A look the user asked for is under way.
    Checking,
    /// There is a release, not downloaded yet (`notify`, or before `auto`
    /// starts on it).
    Available,
    Downloading,
    /// Downloaded and verified: a click installs it.
    Ready,
    Installing,
    /// The last step failed; `error` says how. A click tries again.
    Failed,
}

/// What the webview draws: the ring, its card, and the line in settings.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub phase: Phase,
    /// The version running now.
    pub current: String,
    /// The release on offer, once one has been found.
    pub version: Option<String>,
    /// Its release notes, as written on GitHub (Markdown).
    pub notes: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub error: Option<String>,
    /// When the last look finished, RFC 3339.
    pub checked_at: Option<String>,
    /// "Later": the ring stays away until the next look.
    pub dismissed: bool,
}

/// The release found, and its verified bytes once downloaded.
struct Found {
    update: Update,
    bytes: Option<Vec<u8>>,
}

/// Held in Tauri's state.
pub struct Updates {
    status: Mutex<UpdateStatus>,
    found: Mutex<Option<Found>>,
    /// One look or download at a time.
    busy: tokio::sync::Mutex<()>,
}

impl Updates {
    fn new(current: String) -> Self {
        Self {
            status: Mutex::new(UpdateStatus {
                current,
                ..Default::default()
            }),
            found: Mutex::new(None),
            busy: tokio::sync::Mutex::new(()),
        }
    }
}

/// Keep the state and start looking on a schedule.
pub fn spawn(app: AppHandle) {
    let current = app.package_info().version.to_string();
    app.manage(Updates::new(current));

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if mode(&app) != "off" {
                check(&app, false).await;
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// `auto`, `notify` or `off`, from the settings.
fn mode(app: &AppHandle) -> String {
    app.try_state::<AppState>()
        .map(|state| state.hud.config().updates)
        .unwrap_or_else(|| "auto".to_string())
}

fn updates(app: &AppHandle) -> Option<tauri::State<'_, Updates>> {
    app.try_state::<Updates>()
}

/// The current state, as the webview draws it.
pub fn status(app: &AppHandle) -> UpdateStatus {
    updates(app)
        .and_then(|u| u.status.lock().ok().map(|s| s.clone()))
        .unwrap_or_default()
}

/// Change the state and tell the webview.
fn change(app: &AppHandle, f: impl FnOnce(&mut UpdateStatus)) {
    let Some(updates) = updates(app) else {
        return;
    };
    let snapshot = {
        let Ok(mut status) = updates.status.lock() else {
            return;
        };
        f(&mut status);
        status.clone()
    };
    let _ = app.emit(UPDATE_EVENT, &snapshot);
}

/// Look for a release, and in `auto` download it.
///
/// A look the user asked for reports its failure; a scheduled one only logs
/// it, since being offline for a while is no reason for a ring to appear.
pub async fn check(app: &AppHandle, asked: bool) {
    let Some(state) = updates(app) else {
        return;
    };
    // A scheduled look never queues behind a download; one the user asked for
    // waits its turn.
    let _busy = if asked {
        state.busy.lock().await
    } else {
        match state.busy.try_lock() {
            Ok(guard) => guard,
            Err(_) => return,
        }
    };

    // Already holding a verified download: nothing to look for until it's
    // used. A "later" lasts until this look, though, so the ring comes back.
    let phase = status(app).phase;
    if matches!(phase, Phase::Ready | Phase::Installing) {
        if phase == Phase::Ready && !asked {
            change(app, |s| s.dismissed = false);
        }
        return;
    }
    if asked {
        change(app, |s| {
            s.phase = Phase::Checking;
            s.error = None;
        });
    }

    let result = match app.updater() {
        Ok(updater) => updater.check().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let checked_at = chrono::Utc::now().to_rfc3339();

    match result {
        Ok(Some(update)) => {
            tracing::info!(version = %update.version, "update available");
            let version = update.version.clone();
            let notes = update.body.as_deref().map(trim_notes);
            if let Ok(mut found) = state.found.lock() {
                *found = Some(Found {
                    update,
                    bytes: None,
                });
            }
            change(app, |s| {
                s.phase = Phase::Available;
                s.version = Some(version);
                s.notes = notes;
                s.downloaded = 0;
                s.total = None;
                s.error = None;
                s.checked_at = Some(checked_at);
                s.dismissed = false;
            });
            if mode(app) == "auto" {
                download_locked(app, &state).await;
            }
        }
        Ok(None) => {
            if let Ok(mut found) = state.found.lock() {
                *found = None;
            }
            change(app, |s| {
                *s = UpdateStatus {
                    current: s.current.clone(),
                    checked_at: Some(checked_at),
                    ..Default::default()
                };
            });
        }
        Err(error) => {
            tracing::warn!(%error, "could not check for updates");
            change(app, |s| {
                s.checked_at = Some(checked_at);
                if asked {
                    s.phase = Phase::Failed;
                    s.error = Some(error);
                } else if s.phase == Phase::Checking {
                    s.phase = Phase::Idle;
                }
            });
        }
    }
}

/// Download the release found, for `notify` or after a failed download.
pub async fn download(app: &AppHandle) {
    let Some(state) = updates(app) else {
        return;
    };
    let _busy = state.busy.lock().await;
    download_locked(app, &state).await;
}

/// The download itself; the caller holds `busy`.
async fn download_locked(app: &AppHandle, state: &Updates) {
    let update = match state.found.lock() {
        Ok(found) => match found.as_ref() {
            Some(found) if found.bytes.is_none() => found.update.clone(),
            _ => return,
        },
        Err(_) => return,
    };

    change(app, |s| {
        s.phase = Phase::Downloading;
        s.downloaded = 0;
        s.total = None;
        s.error = None;
    });

    let mut downloaded: u64 = 0;
    let mut last_told = Instant::now() - PROGRESS_EVERY;
    let result = update
        .download(
            |chunk, total| {
                downloaded += chunk as u64;
                if last_told.elapsed() >= PROGRESS_EVERY {
                    last_told = Instant::now();
                    let so_far = downloaded;
                    change(app, |s| {
                        s.downloaded = so_far;
                        s.total = total;
                    });
                }
            },
            || {},
        )
        .await;

    match result {
        Ok(bytes) => {
            tracing::info!(version = %update.version, size = bytes.len(), "update downloaded and verified");
            let size = bytes.len() as u64;
            if let Ok(mut found) = state.found.lock() {
                if let Some(found) = found.as_mut() {
                    found.bytes = Some(bytes);
                }
            }
            change(app, |s| {
                s.phase = Phase::Ready;
                s.downloaded = size;
                s.total = Some(size);
            });
        }
        Err(error) => {
            tracing::warn!(%error, "could not download the update");
            change(app, |s| {
                s.phase = Phase::Failed;
                s.error = Some(error.to_string());
            });
        }
    }
}

/// Install the downloaded release and start it.
///
/// On Windows the installer takes over: the plugin starts it and exits this
/// process, and the installer starts the new version when it's done. On
/// macOS and Linux the files are replaced here (Linux asks for the
/// administrator's password first) and this app restarts into them.
pub async fn install(app: &AppHandle) {
    let Some(state) = updates(app) else {
        return;
    };
    let _busy = state.busy.lock().await;

    let (update, bytes) = match state.found.lock() {
        Ok(mut found) => match found.as_mut() {
            Some(Found { update, bytes }) => match bytes.take() {
                Some(bytes) => (update.clone(), bytes),
                None => return,
            },
            None => return,
        },
        Err(_) => return,
    };

    change(app, |s| {
        s.phase = Phase::Installing;
        s.error = None;
    });
    // Let the ring show it before a password prompt or the installer covers it.
    tokio::time::sleep(Duration::from_millis(250)).await;

    let result = tauri::async_runtime::spawn_blocking(move || {
        let result = update.install(&bytes);
        (result, bytes)
    })
    .await;

    match result {
        Ok((Ok(()), _)) => {
            tracing::info!("update installed; restarting");
            app.restart();
        }
        Ok((Err(error), bytes)) => {
            tracing::warn!(%error, "could not install the update");
            // Keep the bytes, so trying again doesn't download it again.
            if let Ok(mut found) = state.found.lock() {
                if let Some(found) = found.as_mut() {
                    found.bytes = Some(bytes);
                }
            }
            change(app, |s| {
                s.phase = Phase::Ready;
                s.error = Some(error.to_string());
            });
        }
        Err(error) => {
            change(app, |s| {
                s.phase = Phase::Failed;
                s.error = Some(error.to_string());
            });
        }
    }
}

/// Look, download and install in one go: `codenotch --update`.
pub async fn update_now(app: &AppHandle) {
    check(app, true).await;
    if status(app).phase == Phase::Available {
        download(app).await;
    }
    if status(app).phase == Phase::Ready {
        install(app).await;
    }
}

/// Whether this is the first launch of a version that replaced another, for
/// "what's new". Writes the version down, so it's true once.
///
/// A fresh install has nothing that changed, so it says no. One that predates
/// this record has settings but no record, and counts as replaced.
pub fn first_launch_of_new_version(current: &str) -> bool {
    let Some(dir) = dirs::config_dir().map(|d| d.join("CodeNotch")) else {
        return false;
    };
    let record = dir.join("seen-version");
    let seen = std::fs::read_to_string(&record)
        .ok()
        .map(|s| s.trim().to_string());
    let had_settings = dir.join("config.json").exists();
    let new = match &seen {
        Some(seen) => seen != current,
        None => had_settings,
    };
    if seen.as_deref() != Some(current) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&record, current);
    }
    new
}

/// The first `NOTES_MAX` characters of the notes, cut at a line.
fn trim_notes(notes: &str) -> String {
    let notes = notes.trim();
    if notes.chars().count() <= NOTES_MAX {
        return notes.to_string();
    }
    let cut: String = notes.chars().take(NOTES_MAX).collect();
    match cut.rfind('\n') {
        Some(line) => cut[..line].trim_end().to_string(),
        None => cut,
    }
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateStatus {
    status(&app)
}

/// "Check now" in settings.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> UpdateStatus {
    check(&app, true).await;
    status(&app)
}

/// A click on the ring: download what `notify` found, try a failed step
/// again, or install what is ready.
#[tauri::command]
pub async fn update_act(app: AppHandle) {
    let current = status(&app);
    match current.phase {
        Phase::Available => download(&app).await,
        Phase::Ready => install(&app).await,
        Phase::Failed if current.version.is_some() => {
            let downloaded = updates(&app)
                .and_then(|u| {
                    u.found
                        .lock()
                        .ok()
                        .map(|f| f.as_ref().is_some_and(|f| f.bytes.is_some()))
                })
                .unwrap_or(false);
            if downloaded {
                install(&app).await;
            } else {
                download(&app).await;
            }
        }
        Phase::Failed => check(&app, true).await,
        _ => {}
    }
}

/// "Later": hide the ring until the next look.
#[tauri::command]
pub fn update_dismiss(app: AppHandle) {
    change(&app, |s| s.dismissed = true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_notes_are_kept_whole() {
        assert_eq!(trim_notes("  Fixes.\n"), "Fixes.");
    }

    #[test]
    fn long_notes_are_cut_at_a_line() {
        let line = "a".repeat(100);
        let notes = std::iter::repeat_n(line.as_str(), 20)
            .collect::<Vec<_>>()
            .join("\n");
        let cut = trim_notes(&notes);
        assert!(cut.chars().count() <= NOTES_MAX);
        assert!(cut.ends_with('a'));
        assert_eq!(cut.lines().count(), NOTES_MAX / 101);
    }

    #[test]
    fn cutting_never_splits_a_character() {
        let notes = "ç".repeat(NOTES_MAX + 50);
        assert_eq!(trim_notes(&notes).chars().count(), NOTES_MAX);
    }
}
