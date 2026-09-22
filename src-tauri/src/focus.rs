//! Getting to an agent's window, and back again.
//!
//! A click on a ring raises the app its session runs in. A second click, with
//! that app still in front, goes back to wherever the user was before — so
//! the ring works as a switch rather than a one-way door.
//!
//! The same question, "is that window the one in front?", decides whether an
//! agent's news needs announcing at all. If the user is already looking at
//! it, a chime only repeats what the screen is saying.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use codenotch_core::model::{host_processes, ProviderId, ProviderSnapshot, Session};

use crate::platform::{self, WindowHandle};

/// How many windows back the trail goes. The way back needs the first one
/// that still exists; the rest are for when it doesn't.
const TRAIL_LEN: usize = 8;

/// A second click on the same ring this soon is the other half of a double
/// click, not a request to turn straight back.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// App windows the user has worked in, most recent first.
static TRAIL: Mutex<Vec<WindowHandle>> = Mutex::new(Vec::new());

/// The window a ring last switched to or away from, and when.
static LAST_CLICK: Mutex<Option<(WindowHandle, Instant)>> = Mutex::new(None);

/// Note the window in front. Called a few times a second, so where the user
/// was is known before anyone asks.
pub fn note_foreground() {
    let Some(window) = platform::foreground_app() else {
        return;
    };
    let mut trail = TRAIL.lock().expect("trail lock");
    if trail.first() == Some(&window) {
        return;
    }
    trail.retain(|seen| *seen != window);
    trail.insert(0, window);
    trail.truncate(TRAIL_LEN);
}

/// The app window the user is in. While the notch itself holds the focus (a
/// field on it being edited), that is still the one they were in before.
fn current() -> Option<WindowHandle> {
    if platform::foreground_is_ours() {
        return TRAIL.lock().expect("trail lock").first().copied();
    }
    platform::foreground_app()
}

/// Whether `window`, or a dialog of it, is the one the user is in.
pub fn is_current(window: WindowHandle) -> bool {
    current().is_some_and(|current| platform::same_window(current, window))
}

/// The most recent window before `current` that is still there to go back to.
fn before(current: WindowHandle) -> Option<WindowHandle> {
    TRAIL
        .lock()
        .expect("trail lock")
        .iter()
        .copied()
        .find(|&seen| !platform::same_window(seen, current) && platform::can_return_to(seen))
}

/// The window a click on a ring raises for a session: its own app's when the
/// session says where it runs, else the provider's usual suspects. `None` when
/// that app has no window open.
pub fn target(
    provider: ProviderId,
    hint: Option<&str>,
    host: Option<&str>,
) -> Option<WindowHandle> {
    match host.and_then(host_processes) {
        Some(processes) => platform::provider_window(processes, hint),
        None => platform::provider_window(provider.focus_processes(), hint),
    }
}

/// Whether this click on the ring for `window` is the second half of a double
/// click, whose first half has already done the work.
pub fn repeated(window: WindowHandle) -> bool {
    let mut last = LAST_CLICK.lock().expect("click lock");
    if last.is_some_and(|(previous, at)| {
        at.elapsed() < DOUBLE_CLICK && platform::same_window(previous, window)
    }) {
        return true;
    }
    *last = Some((window, Instant::now()));
    false
}

/// Leave `window` for the window the user was in before it. With nowhere left
/// to go back to, staying put is the answer.
pub fn go_back(window: WindowHandle) -> bool {
    // In case the user only just got to where they are.
    note_foreground();
    before(window).is_none_or(platform::bring_to_front)
}

/// Whether a ring's agent is in the window in front — the one a click on the
/// ring would raise — so whatever it just did is already on screen.
///
/// Errs towards "no": a chime too many is a small annoyance, one missed
/// because the notch guessed wrong is the thing it exists to prevent. So a
/// session that says where it runs counts only that app, and one that doesn't
/// counts only the window a click would pick, not any app it might be in.
pub fn in_front(provider: &ProviderSnapshot) -> bool {
    let session = provider.sessions.first();
    let hint = session.and_then(title_hint);
    let host = session.and_then(|s| s.host.as_deref());
    target(provider.id, hint.as_deref(), host).is_some_and(is_current)
}

/// What a session's window would have in its title: the project folder, as
/// editors put it there, or else the session's own title. The same hint the
/// webview sends with a click.
fn title_hint(session: &Session) -> Option<String> {
    session
        .cwd
        .as_deref()
        .and_then(|cwd| cwd.split(['/', '\\']).rfind(|part| !part.is_empty()))
        .or(Some(session.title.as_str()).filter(|title| !title.is_empty()))
        .map(str::to_string)
}
