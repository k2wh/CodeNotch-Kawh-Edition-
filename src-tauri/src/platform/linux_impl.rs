//! X11 window management for the notch on Linux.
//!
//! The same jobs as on Windows (see `windows_impl`), done the X11 way. The
//! window manager is asked through EWMH client messages; clicks fall through
//! the resting notch because its input region, from the SHAPE extension, is
//! empty; and the rest of the desktop is read off the root window: which
//! window is in front, where the panels leave room, what is open.
//!
//! Tauri keeps what it already does on Linux: the window's size and position,
//! and not taking the focus (see `hud.rs`). Everything here goes through a
//! connection of our own, which, unlike GTK's, can be used from any thread.
//!
//! X11 only. Under Wayland the notch runs through XWayland (see `lib.rs`): it
//! still docks, but other apps' windows are out of its sight.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::{anyhow, Result};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ClipOrdering, ConnectionExt as _, EventMask, Rectangle, Window,
};
use x11rb::rust_connection::RustConnection;

use codenotch_core::config::Edge;
use codenotch_core::layout::{place, Placement, WorkArea};

use super::{Backdrop, WindowHandle};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_CLIENT_LIST_STACKING,
        _NET_CURRENT_DESKTOP,
        _NET_RESTACK_WINDOW,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WORKAREA,
        UTF8_STRING,
    }
}

/// `_NET_WM_STATE` actions.
const REMOVE: u32 = 0;
const ADD: u32 = 1;
/// Who is asking, in an EWMH request. A pager is obeyed without the focus
/// stealing prevention an ordinary app's request would meet.
const FROM_APP: u32 = 1;
const FROM_PAGER: u32 = 2;
/// `_NET_RESTACK_WINDOW`'s "below the sibling".
const BELOW: u32 = 1;

/// Our connection to the X server, and what it keeps asking about.
struct Display {
    conn: RustConnection,
    root: Window,
    screen_size: (i32, i32),
    atoms: Atoms,
}

static DISPLAY: OnceLock<Option<Display>> = OnceLock::new();

/// The X server, connected to on first use. `None` without one (a Wayland
/// session with no XWayland, a headless machine), for good.
fn display() -> Option<&'static Display> {
    DISPLAY
        .get_or_init(|| {
            let (conn, screen) = x11rb::connect(None).ok()?;
            let screen = conn.setup().roots.get(screen)?;
            let (root, width, height) =
                (screen.root, screen.width_in_pixels, screen.height_in_pixels);
            let atoms = Atoms::new(&conn).ok()?.reply().ok()?;
            Some(Display {
                conn,
                root,
                screen_size: (width.into(), height.into()),
                atoms,
            })
        })
        .as_ref()
}

/// Our display and the notch's window, when there are both to work with.
/// Without them (a Wayland window, no X server) there is nothing to do here,
/// and that is no error: Tauri still shows and docks the notch.
fn own(handle: WindowHandle) -> Option<(&'static Display, Window)> {
    let display = display()?;
    (handle != 0).then_some((display, handle as Window))
}

impl Display {
    /// A property of 32-bit values: windows, atoms, cardinals.
    fn u32s(&self, window: Window, property: impl Into<u32>, kind: impl Into<u32>) -> Vec<u32> {
        self.conn
            .get_property(false, window, property.into(), kind.into(), 0, 4096)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .and_then(|reply| reply.value32().map(|values| values.collect()))
            .unwrap_or_default()
    }

    /// A text property, as UTF-8 however it was stored.
    fn text(
        &self,
        window: Window,
        property: impl Into<u32>,
        kind: impl Into<u32>,
    ) -> Option<String> {
        let reply = self
            .conn
            .get_property(false, window, property.into(), kind.into(), 0, 1024)
            .ok()?
            .reply()
            .ok()?;
        (!reply.value.is_empty()).then(|| String::from_utf8_lossy(&reply.value).into_owned())
    }

    /// An atom that exists already, by name.
    fn existing_atom(&self, name: &str) -> Option<u32> {
        let atom = self
            .conn
            .intern_atom(true, name.as_bytes())
            .ok()?
            .reply()
            .ok()?
            .atom;
        (atom != 0).then_some(atom)
    }

    /// Ask the window manager for something about `window`, the EWMH way: a
    /// client message to the root window.
    fn ask(&self, window: Window, kind: u32, data: [u32; 5]) -> bool {
        let event = ClientMessageEvent::new(32, window, kind, data);
        let mask = EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY;
        // Nothing here ever reads the event queue, where an error would
        // otherwise wait forever: a request that fails just didn't happen.
        self.conn
            .send_event(false, self.root, mask, event)
            .map(|cookie| cookie.ignore_error())
            .is_ok()
            && self.conn.flush().is_ok()
    }

    /// Top-level app windows, the one in front first.
    fn clients(&self) -> Vec<Window> {
        // The window manager keeps the list bottom to top.
        let mut windows = self.u32s(
            self.root,
            self.atoms._NET_CLIENT_LIST_STACKING,
            AtomEnum::WINDOW,
        );
        windows.reverse();
        windows
    }

    fn active(&self) -> Option<Window> {
        self.u32s(self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW)
            .first()
            .copied()
            .filter(|&window| window != 0)
    }

    fn pid(&self, window: Window) -> Option<u32> {
        self.u32s(window, self.atoms._NET_WM_PID, AtomEnum::CARDINAL)
            .first()
            .copied()
    }

    fn title(&self, window: Window) -> String {
        self.text(window, self.atoms._NET_WM_NAME, self.atoms.UTF8_STRING)
            .or_else(|| self.text(window, AtomEnum::WM_NAME, AtomEnum::ANY))
            .unwrap_or_default()
    }

    fn states(&self, window: Window) -> Vec<u32> {
        self.u32s(window, self.atoms._NET_WM_STATE, AtomEnum::ATOM)
    }

    /// The app window `window` belongs to: a dialog counts as the window it
    /// was opened for, all the way up.
    fn owner(&self, window: Window) -> Window {
        let mut current = window;
        for _ in 0..8 {
            match self
                .u32s(current, AtomEnum::WM_TRANSIENT_FOR, AtomEnum::WINDOW)
                .first()
            {
                Some(&parent) if parent != 0 && parent != self.root && parent != current => {
                    current = parent
                }
                _ => break,
            }
        }
        current
    }

    /// An app window of the kind a click on a ring may raise: a normal window
    /// (or a dialog), on the taskbar, not ours. Minimised ones count; raising
    /// restores them.
    fn is_app_window(&self, window: Window) -> bool {
        let kind = self.u32s(window, self.atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM);
        let normal = kind.is_empty()
            || kind.contains(&self.atoms._NET_WM_WINDOW_TYPE_NORMAL)
            || kind.contains(&self.atoms._NET_WM_WINDOW_TYPE_DIALOG);
        normal
            && !self
                .states(window)
                .contains(&self.atoms._NET_WM_STATE_SKIP_TASKBAR)
            && self.pid(window) != Some(std::process::id())
    }

    fn minimized(&self, window: Window) -> bool {
        self.states(window)
            .contains(&self.atoms._NET_WM_STATE_HIDDEN)
    }
}

/// The executable a process runs, as a window search names it: `code`,
/// `gnome-terminal-server`. A script's interpreter is looked past, so
/// Terminator (Python) is `terminator` rather than `python3.10`.
fn image(pid: u32) -> Option<String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let name = exe.as_deref().and_then(Path::file_name).map(|name| {
        let name = name.to_string_lossy();
        // A binary replaced by an update while it runs.
        name.trim_end_matches(" (deleted)").to_string()
    });
    match name {
        Some(name) if is_interpreter(&name) => script_name(pid).or(Some(name)),
        Some(name) => Some(name),
        None => std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|comm| comm.trim().to_string())
            .filter(|comm| !comm.is_empty()),
    }
}

fn is_interpreter(name: &str) -> bool {
    name.starts_with("python") || name.starts_with("perl") || name == "node" || name == "gjs"
}

/// The script an interpreter was started on: its first argument that isn't
/// an option.
fn script_name(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let script = cmdline
        .split(|&byte| byte == 0)
        .skip(1)
        .find(|arg| !arg.is_empty() && !arg.starts_with(b"-"))?;
    let script = String::from_utf8_lossy(script);
    Path::new(script.as_ref())
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

// --- The notch's own window ------------------------------------------------

/// Which part of the notch takes clicks: the part drawn, or none of it while
/// it rests. Kept here so either half can change without forgetting the other.
struct InputShape {
    region: Option<(i32, i32, i32, i32)>,
    click_through: bool,
}

static INPUT: Mutex<InputShape> = Mutex::new(InputShape {
    region: None,
    click_through: false,
});

/// Apply [`INPUT`] as the window's input region.
fn reshape(handle: WindowHandle) -> Result<()> {
    let Some((display, window)) = own(handle) else {
        return Ok(());
    };
    let (region, click_through) = {
        let input = INPUT.lock().expect("input lock");
        (input.region, input.click_through)
    };
    let conn = &display.conn;
    // As in `Display::ask`, a failed request's error is let go of.
    match (click_through, region) {
        // No rectangles: nothing takes a click, and the pointer passes through.
        (true, _) => conn
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[],
            )?
            .ignore_error(),
        (false, Some((left, top, right, bottom))) => {
            let rect = Rectangle {
                x: clamp_i16(left),
                y: clamp_i16(top),
                width: clamp_u16(right - left),
                height: clamp_u16(bottom - top),
            };
            conn.shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[rect],
            )?
            .ignore_error()
        }
        // No region yet: the whole window, as X11 has it by default.
        (false, None) => conn
            .shape_mask(SO::SET, SK::INPUT, window, 0, 0, x11rb::NONE)?
            .ignore_error(),
    }
    conn.flush()?;
    Ok(())
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i16::MIN.into(), i16::MAX.into()) as i16
}

fn clamp_u16(value: i32) -> u16 {
    value.clamp(0, u16::MAX.into()) as u16
}

/// Stay on top and let clicks through or not. The rest of what makes the
/// notch a HUD — off the taskbar, on every workspace, never taking the
/// focus — Tauri asks for (see `tauri.conf.json` and `hud.rs`).
pub fn apply_hud_chrome(handle: WindowHandle, click_through: bool) -> Result<()> {
    INPUT.lock().expect("input lock").click_through = click_through;
    reshape(handle)?;
    set_topmost(handle)
}

/// Toggle click-through without disturbing the region.
pub fn set_click_through(handle: WindowHandle, enabled: bool) -> Result<()> {
    INPUT.lock().expect("input lock").click_through = enabled;
    reshape(handle)
}

/// Taking the focus for a field being edited is Tauri's to grant on Linux
/// (see `Hud::set_interactive`).
pub fn set_activatable(_handle: WindowHandle, _enabled: bool) -> Result<()> {
    Ok(())
}

/// Nothing to guard: nothing underneath rewrites what this layer sets.
pub fn guard_hud_style(_handle: WindowHandle) {}

/// The input region already decides which clicks the notch takes.
pub fn pointer_at(_x: i32, _y: i32) {}

/// Into the window manager's "above" layer.
pub fn set_topmost(handle: WindowHandle) -> Result<()> {
    if let Some((display, window)) = own(handle) {
        let above = display.atoms._NET_WM_STATE_ABOVE;
        display.ask(
            window,
            display.atoms._NET_WM_STATE,
            [ADD, above, 0, FROM_APP, 0],
        );
    }
    Ok(())
}

/// Out of the "above" layer and directly beneath `other`, for when the user
/// has asked the notch to stay behind the app in front.
pub fn place_below(handle: WindowHandle, other: WindowHandle) -> Result<()> {
    let Some((display, window)) = own(handle) else {
        return Ok(());
    };
    let above = display.atoms._NET_WM_STATE_ABOVE;
    display.ask(
        window,
        display.atoms._NET_WM_STATE,
        [REMOVE, above, 0, FROM_APP, 0],
    );
    display.ask(
        window,
        display.atoms._NET_RESTACK_WINDOW,
        [FROM_PAGER, other as u32, BELOW, 0, 0],
    );
    Ok(())
}

/// Tauri moves the window on Linux (see `Hud::apply`); GTK has to know where
/// its window is.
pub fn move_no_activate(_handle: WindowHandle, _placement: Placement) -> Result<()> {
    Ok(())
}

/// Where the notch goes on its monitor, for `Hud::apply` to put it there.
pub fn dock(
    _handle: WindowHandle,
    monitor: Option<usize>,
    edge: Edge,
    offset: f32,
    margin: f64,
    logical_width: f64,
    logical_height: f64,
) -> Result<Placement> {
    let area = work_area_for(monitor).ok_or_else(|| anyhow!("no monitors reported a work area"))?;
    Ok(place(
        area,
        edge,
        offset,
        margin,
        logical_width,
        logical_height,
    ))
}

/// Clip the part of the window that takes clicks to what the webview drew.
/// Unlike Windows' window region this leaves the drawing alone, but the rest
/// is transparent anyway.
pub fn set_region(handle: WindowHandle, rect: Option<(i32, i32, i32, i32)>) -> Result<()> {
    INPUT.lock().expect("input lock").region = rect;
    reshape(handle)
}

/// Windows' backdrop and frame colour have no X11 counterpart.
pub fn apply_appearance(_handle: WindowHandle, _backdrop: Backdrop) {}

pub fn refresh_frame(_handle: WindowHandle) {}

// --- Scale, monitors and the cursor ----------------------------------------

/// The desktop's scale factor, as the window last reported it (see
/// `note_scale`). GNOME on X11 scales by whole numbers; the X server itself
/// only ever deals in device pixels.
static SCALE: AtomicU64 = AtomicU64::new(0);

/// Record the scale factor the window is drawn at, for the work areas.
pub fn note_scale(scale: f64) {
    if scale.is_finite() && scale > 0.0 {
        SCALE.store(scale.to_bits(), Ordering::Relaxed);
    }
}

fn scale() -> f64 {
    match SCALE.load(Ordering::Relaxed) {
        0 => 1.0,
        bits => f64::from_bits(bits),
    }
}

pub fn window_scale(_handle: WindowHandle) -> f64 {
    scale()
}

type Rect = (i32, i32, i32, i32);

/// Where a monitor leaves room for windows: the work area the window manager
/// published for it (Mutter's per-monitor `_GTK_WORKAREAS`, as GTK reads
/// them), else the desktop-wide `_NET_WORKAREA` cut to the monitor, else the
/// whole monitor.
fn work_area_in(monitor: Rect, per_monitor: &[Rect], desktop: Option<Rect>) -> Rect {
    let (x, y, width, height) = monitor;
    let inside = |&(ax, ay, aw, ah): &Rect| {
        ax >= x && ay >= y && ax + aw <= x + width && ay + ah <= y + height && aw > 0 && ah > 0
    };
    per_monitor
        .iter()
        .find(|area| inside(area))
        .copied()
        .or_else(|| desktop.and_then(|area| intersect(monitor, area)))
        .unwrap_or(monitor)
}

fn intersect(a: Rect, b: Rect) -> Option<Rect> {
    let left = a.0.max(b.0);
    let top = a.1.max(b.1);
    let right = (a.0 + a.2).min(b.0 + b.2);
    let bottom = (a.1 + a.3).min(b.1 + b.3);
    (right > left && bottom > top).then_some((left, top, right - left, bottom - top))
}

fn rects(values: &[u32]) -> Vec<Rect> {
    values
        .chunks_exact(4)
        .map(|v| (v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32))
        .collect()
}

/// Every monitor's work area, and whether it is the primary one.
fn enumerate_monitors() -> Vec<(WorkArea, bool)> {
    let Some(display) = display() else {
        return Vec::new();
    };
    let mut monitors: Vec<(Rect, bool)> = display
        .conn
        .randr_get_monitors(display.root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| {
            reply
                .monitors
                .iter()
                .map(|m| {
                    (
                        (m.x.into(), m.y.into(), m.width.into(), m.height.into()),
                        m.primary,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if monitors.is_empty() {
        let (width, height) = display.screen_size;
        monitors.push(((0, 0, width, height), true));
    }

    let desktop = display
        .u32s(
            display.root,
            display.atoms._NET_CURRENT_DESKTOP,
            AtomEnum::CARDINAL,
        )
        .first()
        .copied()
        .unwrap_or(0);
    let per_monitor = display
        .existing_atom(&format!("_GTK_WORKAREAS_D{desktop}"))
        .map(|atom| rects(&display.u32s(display.root, atom, AtomEnum::CARDINAL)))
        .unwrap_or_default();
    let desktop_area = rects(&display.u32s(
        display.root,
        display.atoms._NET_WORKAREA,
        AtomEnum::CARDINAL,
    ))
    .get(desktop as usize)
    .copied();

    let scale = scale();
    monitors
        .into_iter()
        .map(|(monitor, primary)| {
            let (x, y, width, height) = work_area_in(monitor, &per_monitor, desktop_area);
            (WorkArea::new(x, y, width, height, scale), primary)
        })
        .collect()
}

pub fn monitors() -> Vec<WorkArea> {
    enumerate_monitors()
        .into_iter()
        .map(|(area, _)| area)
        .collect()
}

pub fn primary_work_area() -> Option<WorkArea> {
    let all = enumerate_monitors();
    all.iter()
        .find(|(_, primary)| *primary)
        .or_else(|| all.first())
        .map(|(area, _)| *area)
}

pub fn work_area_for(index: Option<usize>) -> Option<WorkArea> {
    match index {
        Some(i) => monitors().get(i).copied().or_else(primary_work_area),
        None => primary_work_area(),
    }
}

/// Where the pointer is, in the same device pixels as the work areas.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let display = display()?;
    let reply = display
        .conn
        .query_pointer(display.root)
        .ok()?
        .reply()
        .ok()?;
    Some((reply.root_x.into(), reply.root_y.into()))
}

// --- The rest of the desktop -----------------------------------------------

/// The window the user is working in right now.
#[derive(Debug, Clone)]
pub struct Foreground {
    pub window: WindowHandle,
    pub pid: u32,
    /// Executable name, e.g. `firefox`.
    pub image: String,
    /// Full screen (a game, a video, a presentation).
    pub fullscreen: bool,
}

/// Describe the window in front, or `None` when there isn't one (the desktop).
pub fn foreground() -> Option<Foreground> {
    let display = display()?;
    let window = display.active()?;
    let pid = display.pid(window)?;
    Some(Foreground {
        window: window as WindowHandle,
        pid,
        image: image(pid)?,
        fullscreen: display
            .states(window)
            .contains(&display.atoms._NET_WM_STATE_FULLSCREEN),
    })
}

/// The window a provider's card raises, or `None` when none of
/// `process_names` has a window open.
///
/// `title_hint` (typically the project folder) picks between several windows
/// of the same application, as on Windows.
pub fn provider_window(process_names: &[&str], title_hint: Option<&str>) -> Option<WindowHandle> {
    let display = display()?;
    let candidates: Vec<(Window, String, String)> = display
        .clients()
        .into_iter()
        .filter(|&window| display.is_app_window(window))
        .filter_map(|window| {
            let image = image(display.pid(window)?)?;
            process_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&image))
                .then(|| (window, image, display.title(window)))
        })
        .collect();

    let chosen = title_hint
        .and_then(|hint| {
            let hint = hint.to_lowercase();
            candidates
                .iter()
                .find(|(_, _, title)| title.to_lowercase().contains(&hint))
        })
        // Otherwise the earliest-listed executable, in the order the caller
        // ranked them, and of its windows the one nearest the front.
        .or_else(|| {
            process_names.iter().find_map(|name| {
                candidates
                    .iter()
                    .find(|(_, image, _)| image.eq_ignore_ascii_case(name))
            })
        })
        .or_else(|| candidates.first());

    chosen.map(|(window, _, _)| *window as WindowHandle)
}

/// Bring a window to the front, the way a taskbar does: a pager's request
/// to activate it, which also restores it and switches to its workspace.
pub fn bring_to_front(window: WindowHandle) -> bool {
    let Some(display) = display() else {
        return false;
    };
    let current = display.active().unwrap_or(0);
    display.ask(
        window as Window,
        display.atoms._NET_ACTIVE_WINDOW,
        [FROM_PAGER, 0, current, 0, 0],
    )
}

/// Whether two handles are the same app window, a dialog counting as its owner.
pub fn same_window(a: WindowHandle, b: WindowHandle) -> bool {
    match display() {
        Some(display) => display.owner(a as Window) == display.owner(b as Window),
        None => a == b,
    }
}

/// Whether `window`, or a dialog of it, is the one the user is in.
pub fn is_foreground(window: WindowHandle) -> bool {
    display().is_some_and(|display| {
        display
            .active()
            .is_some_and(|active| display.owner(active) == display.owner(window as Window))
    })
}

/// Whether the window in front is ours: the notch, holding the focus while a
/// field on it is being edited.
pub fn foreground_is_ours() -> bool {
    display().is_some_and(|display| {
        display
            .active()
            .and_then(|window| display.pid(window))
            .is_some_and(|pid| pid == std::process::id())
    })
}

/// Whether the user could be sent back to `window`: still open, on screen
/// rather than minimised, and an app window rather than a panel or the notch.
pub fn can_return_to(window: WindowHandle) -> bool {
    let Some(display) = display() else {
        return false;
    };
    let window = window as Window;
    display.clients().contains(&window)
        && display.is_app_window(window)
        && !display.minimized(window)
        && !display.title(window).is_empty()
}

/// The app window the user is working in, when it is one they could be sent
/// back to later (see [`can_return_to`]).
pub fn foreground_app() -> Option<WindowHandle> {
    let display = display()?;
    let window = display.owner(display.active()?) as WindowHandle;
    can_return_to(window).then_some(window)
}

/// A desktop app opened by its Store id: Windows only.
pub fn activate_app(_app_id: &str) -> bool {
    false
}

/// One open window, as the "stay behind" picker shows it.
#[derive(Debug, Clone)]
pub struct WindowShot {
    pub id: WindowHandle,
    pub title: String,
    /// Executable name, e.g. `firefox`.
    pub image: String,
    /// Always `None` here: X11 would only show what isn't covered.
    pub thumbnail: Option<Vec<u8>>,
    pub minimized: bool,
}

/// Every app window, the one in front first. Without thumbnails: capturing a
/// window on X11 only yields the parts of it nothing else is covering.
pub fn open_windows(_thumb_width: i32) -> Vec<WindowShot> {
    let Some(display) = display() else {
        return Vec::new();
    };
    display
        .clients()
        .into_iter()
        .filter(|&window| display.is_app_window(window))
        .filter_map(|window| {
            let title = display.title(window);
            if title.is_empty() {
                return None;
            }
            Some(WindowShot {
                id: window as WindowHandle,
                image: image(display.pid(window)?)?,
                minimized: display.minimized(window),
                title,
                thumbnail: None,
            })
        })
        .collect()
}

/// The executables with a window open, for the "stay behind" list.
pub fn open_apps() -> Vec<String> {
    let mut found: Vec<String> = open_windows(0).into_iter().map(|shot| shot.image).collect();
    found.sort_by_key(|name| name.to_lowercase());
    found.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    found
}

// --- The session -----------------------------------------------------------

/// The desktop's language as one of the interface languages, for the "auto"
/// setting, the way gettext picks it: `LANGUAGE` first, then the locale.
pub fn system_language() -> &'static str {
    let locale = ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        .unwrap_or_default();
    language_of(&locale)
}

fn language_of(locale: &str) -> &'static str {
    match locale.split([':', '_', '.', '@', '-']).next().unwrap_or("") {
        "pt" => "pt",
        "es" => "es",
        _ => "en",
    }
}

/// The autostart entry that starts the notch at sign-in, per the XDG spec.
fn autostart_file() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("autostart")
            .join("codenotch.desktop"),
    )
}

/// Whether CodeNotch is set to start at sign-in.
pub fn launch_at_login() -> bool {
    autostart_file().is_some_and(|path| path.is_file())
}

/// Add or remove the autostart entry.
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    let path = autostart_file().ok_or_else(|| anyhow!("no config directory"))?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Quoted, so a path with a space in it stays one argument.
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=CodeNotch\n\
         Comment=AI coding-assistant usage limits at the edge of the screen\n\
         Exec=\"{}\"\n\
         Icon=codenotch\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        exe.display()
    );
    std::fs::write(&path, entry)?;
    Ok(())
}

pub fn swap_rgb(rgb: u32) -> u32 {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    (b << 16) | (g << 8) | r
}

pub fn parse_hex_colour(hex: &str) -> Option<u32> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_comes_from_the_locale_name() {
        assert_eq!(language_of("pt_BR.UTF-8"), "pt");
        assert_eq!(language_of("pt_BR:en"), "pt");
        assert_eq!(language_of("es_ES@euro"), "es");
        assert_eq!(language_of("en_US.UTF-8"), "en");
        assert_eq!(language_of("de_DE"), "en");
        assert_eq!(language_of(""), "en");
    }

    #[test]
    fn a_monitor_takes_its_own_work_area_over_the_desktop_wide_one() {
        let left = (0, 0, 1920, 1080);
        let right = (1920, 0, 2560, 1440);
        // GNOME's top bar on each monitor.
        let per_monitor = [(0, 32, 1920, 1048), (1920, 32, 2560, 1408)];
        let desktop = Some((0, 32, 4480, 1048));
        assert_eq!(
            work_area_in(left, &per_monitor, desktop),
            (0, 32, 1920, 1048)
        );
        assert_eq!(
            work_area_in(right, &per_monitor, desktop),
            (1920, 32, 2560, 1408)
        );
    }

    /// Against a live X server with a window manager and two xterms open,
    /// titled `one` and `two` (see `packaging/linux/probe.sh`):
    /// `cargo test --lib -- --ignored x11_desktop --nocapture`.
    #[test]
    #[ignore]
    fn x11_desktop() {
        use std::time::Duration;
        let settle = || std::thread::sleep(Duration::from_millis(400));

        println!("monitors: {:?}", monitors());
        println!("cursor:   {:?}", cursor_pos());
        for shot in open_windows(0) {
            println!(
                "window {:#x}: {} {:?} minimized={}",
                shot.id, shot.image, shot.title, shot.minimized
            );
        }
        assert_eq!(open_apps(), vec!["xterm".to_string()]);

        // The title picks between two windows of the same program.
        let one = provider_window(&["xterm"], Some("one")).expect("xterm `one` is open");
        let two = provider_window(&["xterm"], Some("two")).expect("xterm `two` is open");
        assert_ne!(one, two);
        assert!(same_window(one, one) && !same_window(one, two));

        // Raising one puts it in front, and the other is somewhere to go back
        // to. Starting with whichever isn't in front already.
        let (one, two) = if is_foreground(one) {
            (two, one)
        } else {
            (one, two)
        };
        println!("in front before: {:?}", foreground().map(|f| f.window));
        assert!(bring_to_front(one));
        settle();
        println!(
            "in front after raising {one:#x}: {:?}",
            foreground().map(|f| f.window)
        );
        assert!(is_foreground(one), "{:?}", foreground());
        assert_eq!(foreground_app(), Some(one));
        assert!(can_return_to(two));
        assert!(!foreground_is_ours());
        let front = foreground().expect("something is in front");
        assert_eq!(front.image, "xterm");
        assert!(!front.fullscreen);

        assert!(bring_to_front(two));
        settle();
        assert!(is_foreground(two) && !is_foreground(one));
    }

    #[test]
    fn without_one_it_falls_back_to_the_desktop_cut_to_the_monitor_then_the_monitor() {
        let monitor = (1920, 0, 1920, 1080);
        let desktop = Some((0, 27, 3840, 1053));
        assert_eq!(work_area_in(monitor, &[], desktop), (1920, 27, 1920, 1053));
        assert_eq!(work_area_in(monitor, &[], None), monitor);
        // A desktop area that misses the monitor entirely is no help.
        assert_eq!(work_area_in(monitor, &[], Some((0, 0, 100, 100))), monitor);
    }
}
