//! The HUD window controller: what size the notch is and where it sits.
//!
//! All sizing and positioning goes through here so there is exactly one place
//! that decides how big the notch is and one call that moves it — which is what
//! keeps the "never steal focus" guarantee honest.
//!
//! The window never resizes on hover. It spans the whole edge at the size the
//! biggest card needs, stays transparent, and the webview draws the strip and
//! the card inside it. Resizing a WebView2 window makes Windows stretch the old
//! frame until the new one is painted, which read as the notch smearing out
//! and snapping back on every hover. Instead, a window region clipped to what
//! is actually drawn keeps the empty part from ever blocking clicks.
//!
//! Hover is driven from the cursor position rather than from the webview's own
//! mouse events. It has to be: while the notch is resting it carries
//! `WS_EX_TRANSPARENT` so clicks fall through to whatever is underneath, and a
//! click-through window receives no mouse messages at all — not even
//! `mouseenter`. Polling `GetCursorPos` is what lets the notch be both
//! click-through *and* hoverable.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, WebviewWindow};

use codenotch_core::config::{Config, Edge, HudMetrics, MonitorChoice};
use codenotch_core::layout::{Placement, WorkArea, MAX_POPOVER_LENGTH};

use crate::platform::{self, Backdrop, WindowHandle};

/// Event name the webview listens on for open/close changes.
pub const HUD_STATE_EVENT: &str = "codenotch://hud-state";

/// Extra physical pixels around the drawn areas, so a pointer on the very edge
/// of a ring or card still counts as on it.
const HIT_SLOP: i32 = 2;
/// How long the pointer has to stay off the notch before it closes. Opening
/// is immediate; closing waits, so a pointer that overshoots a card's edge on
/// its way to a control doesn't slam the card shut under it.
const LEAVE_GRACE: Duration = Duration::from_millis(250);

/// What the frontend needs to know about the window's own state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HudState {
    /// The notch is expanded: hovered, pinned, peeking or being edited.
    pub open: bool,
    pub pinned: bool,
    pub hidden: bool,
    /// True while a temporary attention peek is showing.
    pub peeking: bool,
    /// Tucked into the screen edge because a game, a video or a listed app
    /// has the foreground. The webview slides the notch away on this, before
    /// the window drops behind whatever is in front, and back out after it
    /// has come back up — so going behind something is a motion rather than
    /// a disappearance.
    #[serde(default)]
    pub tucked: bool,
    /// Logical size of the window, so the webview can place the strip within it.
    pub width: f64,
    pub height: f64,
}

/// A rectangle in logical pixels, relative to the window's top-left corner.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn union(self, other: Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect {
            x,
            y,
            width: (self.x + self.width).max(other.x + other.width) - x,
            height: (self.y + self.height).max(other.y + other.height) - y,
        }
    }

    fn usable(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.width > 0.0
            && self.height > 0.0
    }
}

/// What the webview has drawn, and where: the strip, plus the card or
/// settings panel when one is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Regions {
    pub strip: Rect,
    pub popover: Option<Rect>,
}

impl Regions {
    /// Everything drawn, as one box. The gap between the strip and the card is
    /// inside it, so crossing from a ring to its card never leaves the notch.
    fn drawn(&self) -> Rect {
        match self.popover {
            Some(popover) => self.strip.union(popover),
            None => self.strip,
        }
    }
}

struct Inner {
    config: Config,
    /// Cursor is over the notch (or over the popover it opened).
    hovering: bool,
    /// When the cursor left the notch, while [`LEAVE_GRACE`] runs out.
    outside_since: Option<Instant>,
    /// A text field or native picker (colour, dropdown) is in use: stay open
    /// even though the pointer has left for the picker's own popup, and accept
    /// keyboard focus while it lasts.
    interactive: bool,
    pinned: bool,
    peek_until: Option<Instant>,
    /// Number of rings, for sizing the strip before the webview reports it.
    providers: usize,
    /// Where the webview drew things, once it has told us.
    regions: Option<Regions>,
    last_placement: Option<Placement>,
    last_state: Option<HudState>,
    /// Window-relative physical rectangle last applied as the window region.
    last_region: Option<(i32, i32, i32, i32)>,
    /// Work area the notch was last docked into, to notice resolution, DPI,
    /// taskbar and monitor changes that happen behind our back.
    last_area: Option<WorkArea>,
    /// The foreground window the notch is staying behind, per the user's
    /// "stay behind" settings. `None` means the usual always-on-top.
    below: Option<WindowHandle>,
    /// A window the notch is about to go behind, once its slide away has had
    /// time to play. Dropping the window at once would hide the animation
    /// behind the very thing the notch is getting out of the way of.
    tucking: Option<WindowHandle>,
}

impl Inner {
    /// Open when anything is asking for it.
    fn open(&self) -> bool {
        if self.config.hidden {
            return false;
        }
        self.config.always_expanded
            || self.pinned
            || self.hovering
            || self.interactive
            || self.peek_until.is_some_and(|t| Instant::now() < t)
    }

    fn metrics(&self) -> HudMetrics {
        self.config.metrics()
    }

    fn monitor(&self) -> Option<usize> {
        match self.config.monitor {
            MonitorChoice::Primary => None,
            MonitorChoice::Index(i) => Some(i),
        }
    }

    /// The fixed logical window size: the full length of the edge, and deep
    /// enough for the strip plus the biggest card beside it.
    fn extent(&self, area: WorkArea) -> (f64, f64) {
        let metrics = self.metrics();
        let scale = if area.scale > 0.0 { area.scale } else { 1.0 };
        if self.config.edge.is_horizontal() {
            let depth = metrics.strip_thickness + metrics.popover_gap + MAX_POPOVER_LENGTH;
            (area.width as f64 / scale, depth)
        } else {
            let depth = metrics.strip_thickness + metrics.popover_gap + metrics.popover_size;
            (depth, area.height as f64 / scale)
        }
    }

    /// Window-relative physical rectangle for a logical one.
    fn to_physical(&self, rect: Rect) -> Option<(i32, i32, i32, i32)> {
        let scale = self.last_area?.scale;
        let x = (rect.x * scale).floor() as i32 - HIT_SLOP;
        let y = (rect.y * scale).floor() as i32 - HIT_SLOP;
        let right = ((rect.x + rect.width) * scale).ceil() as i32 + HIT_SLOP;
        let bottom = ((rect.y + rect.height) * scale).ceil() as i32 + HIT_SLOP;
        Some((x.max(0), y.max(0), right, bottom))
    }
}

/// Owns the HUD window.
pub struct Hud {
    window: WebviewWindow,
    inner: Mutex<Inner>,
    /// The window's X11 id or `NSWindow`, found once as it is set up (see
    /// `prepare_x11`, `prepare_mac`).
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    native: std::sync::OnceLock<WindowHandle>,
}

impl Hud {
    pub fn new(window: WebviewWindow, config: Config) -> Self {
        Self {
            window,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            native: std::sync::OnceLock::new(),
            inner: Mutex::new(Inner {
                config,
                hovering: false,
                outside_since: None,
                interactive: false,
                pinned: false,
                peek_until: None,
                providers: 0,
                regions: None,
                last_placement: None,
                last_state: None,
                last_region: None,
                last_area: None,
                below: None,
                tucking: None,
            }),
        }
    }

    pub fn window(&self) -> &WebviewWindow {
        &self.window
    }

    pub fn config(&self) -> Config {
        self.inner.lock().expect("hud lock").config.clone()
    }

    pub fn metrics(&self) -> HudMetrics {
        self.inner.lock().expect("hud lock").metrics()
    }

    /// Native handle, or `None` if the window has already gone away.
    fn handle(&self) -> Option<WindowHandle> {
        #[cfg(windows)]
        {
            self.window.hwnd().ok().map(|h| h.0 as WindowHandle)
        }
        // Without an X11 id (a Wayland window) the platform layer is handed
        // 0 and leaves the window be; Tauri still docks it.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            Some(self.native.get().copied().unwrap_or(0))
        }
        #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
        {
            Some(0)
        }
    }

    /// macOS: hand the platform layer the window, which turns it into the
    /// non-activating panel the original uses, and keep its `NSWindow`.
    #[cfg(target_os = "macos")]
    fn prepare_mac(&self) {
        platform::attach(&self.window);
        if let Ok(ns_window) = self.window.ns_window() {
            let _ = self.native.set(ns_window as WindowHandle);
        }
    }

    /// Linux: find the window's X11 id, which the platform layer works with.
    /// The window is realised here, still hidden, so it has one before it is
    /// first shown. (That it never takes the focus from the app the user is
    /// in is `focusable: false` in `tauri.conf.json`; a field being edited
    /// borrows it, see `set_interactive`.)
    #[cfg(target_os = "linux")]
    fn prepare_x11(&self) {
        use gtk::prelude::WidgetExt;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        if let Ok(gtk_window) = self.window.gtk_window() {
            gtk_window.realize();
        }
        if let Ok(scale) = self.window.scale_factor() {
            platform::note_scale(scale);
        }
        let xid = self
            .window
            .window_handle()
            .ok()
            .and_then(|handle| match handle.as_raw() {
                RawWindowHandle::Xlib(xlib) => Some(xlib.window as WindowHandle),
                RawWindowHandle::Xcb(xcb) => Some(xcb.window.get() as WindowHandle),
                _ => None,
            });
        if let Some(xid) = xid {
            let _ = self.native.set(xid);
        }
    }

    /// One-time setup: HUD window styles and the Win11 appearance attributes.
    pub fn initialise(&self) -> Result<()> {
        #[cfg(target_os = "linux")]
        self.prepare_x11();
        #[cfg(target_os = "macos")]
        self.prepare_mac();

        let config = self.config();
        let Some(handle) = self.handle() else {
            return Ok(());
        };

        // Before anything shows the window: showing it is one of the things
        // that rewrites its style.
        platform::guard_hud_style(handle);
        platform::apply_hud_chrome(handle, config.click_through_when_collapsed)?;
        #[cfg(target_os = "macos")]
        platform::set_over_fullscreen(handle, !config.stay_below_fullscreen);
        // No system backdrop: acrylic tints the whole (mostly empty) window
        // rectangle grey around the notch.
        platform::apply_appearance(handle, Backdrop::None);
        self.apply()?;

        // Start out trimmed: the notch launches closed and may stay that way
        // for hours, and waiting for the first open-then-close to ask for it
        // left all of WebView2's start-up memory resident until then.
        crate::webview::set_low_memory(&self.window, true);

        if !config.hidden {
            let _ = self.window.show();
        }
        Ok(())
    }

    /// Dock the window, update click-through and the region, and push the
    /// state to the webview.
    pub fn apply(&self) -> Result<()> {
        let (config, open, monitor) = {
            let inner = self.inner.lock().expect("hud lock");
            (inner.config.clone(), inner.open(), inner.monitor())
        };

        if config.hidden {
            let _ = self.window.hide();
            let state = self.state();
            self.publish(state);
            return Ok(());
        }

        let Some(handle) = self.handle() else {
            return Ok(());
        };

        // Clicks only pass through while the notch is resting; an open popover
        // has things on it to click.
        let click_through = !open && config.click_through_when_collapsed;
        platform::set_click_through(handle, click_through)?;

        let area = platform::work_area_for(monitor)
            .ok_or_else(|| anyhow!("no monitors reported a work area"))?;
        let (width, height) = self.inner.lock().expect("hud lock").extent(area);

        // The window spans the whole edge, so the offset along it doesn't move
        // the window; the webview places the strip inside it instead.
        let placement = platform::dock(
            handle,
            monitor,
            config.edge,
            0.5,
            config.margin,
            width,
            height,
        )?;

        // Elsewhere the platform layer leaves moving to Tauri, so the toolkit
        // underneath (GTK, on Linux) knows where its window is. Only when the
        // spot changed: a resize, even to the same size, makes GTK lay the
        // webview out again, and this runs on every hover.
        #[cfg(not(windows))]
        {
            use tauri::{PhysicalPosition, PhysicalSize};
            let last = self.inner.lock().expect("hud lock").last_placement;
            if last.map(|p| (p.width, p.height)) != Some((placement.width, placement.height)) {
                let _ = self.window.set_size(PhysicalSize::new(
                    placement.width.max(1) as u32,
                    placement.height.max(1) as u32,
                ));
            }
            if last.map(|p| (p.x, p.y)) != Some((placement.x, placement.y)) {
                let _ = self
                    .window
                    .set_position(PhysicalPosition::new(placement.x, placement.y));
            }
        }

        let below = {
            let mut inner = self.inner.lock().expect("hud lock");
            if inner.last_area != Some(area) {
                // A new display or scale: the old region no longer lines up.
                inner.last_region = None;
            }
            inner.last_placement = Some(placement);
            inner.last_area = Some(area);
            inner.below
        };
        // Moving never touches the z-order, so restate it here.
        self.restate_layer(handle, below);
        self.update_region(handle);

        let _ = self.window.show();
        let state = self.state();
        self.publish(state);
        Ok(())
    }

    /// Clip the window to what the webview has drawn, so its transparent
    /// remainder never takes clicks meant for the apps underneath.
    fn update_region(&self, handle: WindowHandle) {
        let rect = {
            let mut inner = self.inner.lock().expect("hud lock");
            let rect = inner.regions.and_then(|r| inner.to_physical(r.drawn()));
            if rect == inner.last_region {
                return;
            }
            inner.last_region = rect;
            rect
        };
        let _ = platform::set_region(handle, rect);
    }

    /// Send the state to the webview if it differs from what it last got.
    fn publish(&self, state: HudState) {
        let (changed, opened) = {
            let mut inner = self.inner.lock().expect("hud lock");
            let previous = inner.last_state.replace(state.clone());
            (
                previous.as_ref() != Some(&state),
                previous.map(|p| p.open) != Some(state.open),
            )
        };
        if opened {
            // Resting, the notch draws nothing: let WebView2 release what it
            // is holding, and take it back when something is on screen.
            crate::webview::set_low_memory(&self.window, !state.open);
        }
        if changed {
            let _ = self.window.emit(HUD_STATE_EVENT, &state);
        }
    }

    /// Pointer entered or left the notch.
    pub fn set_hover(&self, hovering: bool) -> Result<()> {
        {
            let mut inner = self.inner.lock().expect("hud lock");
            if inner.hovering == hovering {
                return Ok(());
            }
            inner.hovering = hovering;
            inner.outside_since = None;
            // Leaving cancels a peek: the user has seen it.
            if !hovering {
                inner.peek_until = None;
            }
        }
        self.apply()
    }

    /// Enter or leave interactive mode (see [`Inner::interactive`]).
    ///
    /// The notch is normally `WS_EX_NOACTIVATE` so it never steals focus, which
    /// also means keystrokes can never reach it. While a field is being edited
    /// it has to become a normal, activatable window; clicking anywhere else
    /// then deactivates it, the field blurs, and the webview switches it back.
    pub fn set_interactive(&self, on: bool) -> Result<()> {
        {
            let mut inner = self.inner.lock().expect("hud lock");
            if inner.interactive == on {
                return Ok(());
            }
            inner.interactive = on;
        }
        if let Some(handle) = self.handle() {
            platform::set_activatable(handle, on)?;
        }
        // On Linux whether a window takes the focus is GTK's to say, so the
        // lending is done through it. (On macOS the panel does it itself.)
        #[cfg(target_os = "linux")]
        {
            let _ = self.window.set_focusable(on);
            if on {
                let _ = self.window.set_focus();
            }
        }
        self.apply()
    }

    /// Record where the webview drew the strip and the card.
    pub fn set_regions(&self, regions: Regions) -> Result<()> {
        if !regions.strip.usable() {
            return Ok(());
        }
        let regions = Regions {
            popover: regions.popover.filter(|p| p.usable()),
            ..regions
        };
        {
            let mut inner = self.inner.lock().expect("hud lock");
            if inner.regions == Some(regions) {
                return Ok(());
            }
            inner.regions = Some(regions);
        }
        if let Some(handle) = self.handle() {
            self.update_region(handle);
        }
        Ok(())
    }

    /// How many rings the strip has to hold.
    pub fn set_provider_count(&self, providers: usize) -> Result<()> {
        self.inner.lock().expect("hud lock").providers = providers;
        Ok(())
    }

    /// Toggle "stay open", returning the new pinned state.
    pub fn toggle_pin(&self) -> Result<bool> {
        let pinned = {
            let mut inner = self.inner.lock().expect("hud lock");
            inner.pinned = !inner.pinned;
            inner.pinned
        };
        self.apply()?;
        Ok(pinned)
    }

    /// Briefly open the notch, for when an agent needs attention.
    ///
    /// The macOS original's rationale applies: a peek is useless behind a
    /// full-screen window, so it is time-boxed and always cancellable.
    pub fn peek(&self, duration: Duration) -> Result<()> {
        {
            let mut inner = self.inner.lock().expect("hud lock");
            if inner.config.hidden || !inner.config.peek_on_attention {
                return Ok(());
            }
            let until = Instant::now() + duration;
            // Never shorten an in-flight peek.
            if inner.peek_until.is_some_and(|t| t >= until) {
                return Ok(());
            }
            inner.peek_until = Some(until);
        }
        self.apply()
    }

    /// The screen rectangle the cursor has to be inside for the notch to be
    /// hovered: the strip while resting, and the strip plus its card (and the
    /// gap between them) while open.
    fn hover_rect(&self) -> Option<Placement> {
        let inner = self.inner.lock().expect("hud lock");
        let placement = inner.last_placement?;

        let Some(regions) = inner.regions else {
            // Nothing measured yet: the strip's resting slot along the edge.
            return Some(self.fallback_strip(&inner, placement));
        };
        let rect = if inner.open() {
            regions.drawn()
        } else {
            regions.strip
        };
        let (x, y, right, bottom) = inner.to_physical(rect)?;
        Some(Placement {
            x: placement.x + x,
            y: placement.y + y,
            width: right - x,
            height: bottom - y,
        })
    }

    /// Where the strip should be before the webview has reported it, so the
    /// notch can still be hovered open during start-up.
    fn fallback_strip(&self, inner: &Inner, placement: Placement) -> Placement {
        let scale = inner.last_area.map_or(1.0, |a| a.scale);
        let metrics = inner.metrics();
        let (along, thickness) = metrics.strip_extent(inner.providers);
        let along = (along * scale).round() as i32;
        let thickness = (thickness * scale).round() as i32;
        let offset = inner.config.edge_offset.clamp(0.0, 1.0) as f64;
        match inner.config.edge {
            Edge::Right | Edge::Left => {
                let y = placement.y + ((placement.height - along).max(0) as f64 * offset) as i32;
                let x = if inner.config.edge == Edge::Right {
                    placement.x + placement.width - thickness
                } else {
                    placement.x
                };
                Placement {
                    x,
                    y,
                    width: thickness,
                    height: along,
                }
            }
            Edge::Top | Edge::Bottom => {
                let x = placement.x + ((placement.width - along).max(0) as f64 * offset) as i32;
                let y = if inner.config.edge == Edge::Bottom {
                    placement.y + placement.height - thickness
                } else {
                    placement.y
                };
                Placement {
                    x,
                    y,
                    width: along,
                    height: thickness,
                }
            }
        }
    }

    /// Poll the cursor and open or close the notch to match.
    ///
    /// Returns true when the hover state changed.
    fn poll_cursor(&self) -> Result<bool> {
        let Some((x, y)) = platform::cursor_pos() else {
            // No cursor source (non-Windows): the webview's own mouse events
            // drive hover instead.
            return Ok(false);
        };
        // Where a window can't take clicks in one part and not another
        // (macOS), it switches as the pointer moves.
        platform::pointer_at(x, y);
        let Some(rect) = self.hover_rect() else {
            return Ok(false);
        };

        let inside =
            x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;

        let next = {
            let mut inner = self.inner.lock().expect("hud lock");
            if inside {
                inner.outside_since = None;
                (!inner.hovering).then_some(true)
            } else if inner.hovering {
                let since = *inner.outside_since.get_or_insert_with(Instant::now);
                (since.elapsed() >= LEAVE_GRACE).then_some(false)
            } else {
                None
            }
        };
        match next {
            Some(hovering) => {
                self.set_hover(hovering)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Called from the slow poll loop: stay on top (or behind, see `below`).
    ///
    /// The topmost re-assert matters because other always-on-top windows (and
    /// apps going full-screen) can quietly push us down the z-order.
    pub fn tick(&self) -> Result<()> {
        if self.config().hidden {
            return Ok(());
        }
        let (below, open) = {
            let inner = self.inner.lock().expect("hud lock");
            (inner.below, inner.open())
        };
        if let Some(handle) = self.handle() {
            self.restate_layer(handle, below);
        }
        // Re-assert the trim while resting: WebView2 drifts back to holding
        // things after activity, and the notch rests most of the time.
        if !open {
            crate::webview::set_low_memory(&self.window, true);
        }
        Ok(())
    }

    /// Topmost, or tucked just beneath the window we're staying behind.
    fn restate_layer(&self, handle: WindowHandle, below: Option<WindowHandle>) {
        let _ = match below {
            Some(other) => platform::place_below(handle, other),
            None => platform::set_topmost(handle),
        };
    }

    /// Decide whether to stay behind the app in front, per the user's list and
    /// the full-screen switch. Runs a few times a second from the tracker.
    pub fn check_foreground(&self) -> Result<()> {
        let (fullscreen_rule, apps, current, interactive) = {
            let inner = self.inner.lock().expect("hud lock");
            if inner.config.hidden {
                return Ok(());
            }
            (
                inner.config.stay_below_fullscreen,
                inner.config.stay_below_apps.clone(),
                inner.below,
                inner.interactive,
            )
        };

        let target = match platform::foreground() {
            // Our own window (a settings field being edited) never counts, and
            // while the user is editing one the notch stays where it is.
            Some(fg) if fg.pid != std::process::id() && !interactive => {
                let listed = apps.iter().any(|a| a.eq_ignore_ascii_case(&fg.image));
                (listed || (fullscreen_rule && fg.fullscreen)).then_some(fg.window)
            }
            Some(fg) if fg.pid == std::process::id() => current,
            _ => None,
        };

        let tucking = self.inner.lock().expect("hud lock").tucking;
        match (current, target) {
            // Nothing in front that we stay behind, and nothing pending.
            (None, None) if tucking.is_none() => return Ok(()),

            // Something came to the front. First slide away; the window only
            // drops a check later (a quarter of a second), when the webview
            // has finished moving it out of sight.
            (None, Some(window)) => {
                if tucking == Some(window) {
                    let mut inner = self.inner.lock().expect("hud lock");
                    inner.tucking = None;
                    inner.below = Some(window);
                    drop(inner);
                    if let Some(handle) = self.handle() {
                        self.restate_layer(handle, Some(window));
                    }
                } else {
                    self.inner.lock().expect("hud lock").tucking = Some(window);
                    self.publish(self.state());
                }
            }

            // The game went away before the slide finished: come straight
            // back, nothing was ever dropped.
            (None, None) => {
                self.inner.lock().expect("hud lock").tucking = None;
                self.publish(self.state());
            }

            // It went away while we were behind it. Come back up first — still
            // tucked, so nothing flashes — then let the webview slide out.
            (Some(_), None) => {
                {
                    let mut inner = self.inner.lock().expect("hud lock");
                    inner.below = None;
                    inner.tucking = None;
                }
                if let Some(handle) = self.handle() {
                    self.restate_layer(handle, None);
                }
                self.publish(self.state());
            }

            // Behind one thing, now behind another: no need to come out.
            (Some(before), Some(after)) if before != after => {
                self.inner.lock().expect("hud lock").below = Some(after);
                if let Some(handle) = self.handle() {
                    self.restate_layer(handle, Some(after));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Re-dock if the display under the notch changed: resolution, scale,
    /// taskbar position, or a monitor being plugged in or out. Cheap enough to
    /// run every second or so.
    pub fn check_display(&self) -> Result<()> {
        let (monitor, last) = {
            let inner = self.inner.lock().expect("hud lock");
            if inner.config.hidden {
                return Ok(());
            }
            (inner.monitor(), inner.last_area)
        };
        if last.is_some() && platform::work_area_for(monitor) != last {
            self.apply()?;
        }
        Ok(())
    }

    /// Called from the fast tracker (~every 80 ms): follow the cursor and
    /// expire peeks.
    ///
    /// This has to run far more often than provider polling. While resting the
    /// notch is click-through, so the webview gets no mouse events at all and
    /// this poll is the only thing that can open it.
    pub fn track(&self) -> Result<()> {
        if self.config().hidden {
            return Ok(());
        }

        let expired = {
            let mut inner = self.inner.lock().expect("hud lock");
            match inner.peek_until {
                Some(t) if Instant::now() >= t => {
                    inner.peek_until = None;
                    true
                }
                _ => false,
            }
        };

        let moved = self.poll_cursor()?;

        if expired && !moved {
            self.apply()?;
        }
        Ok(())
    }

    /// Apply new settings, re-running the appearance calls the change affects.
    pub fn set_config(&self, config: Config) -> Result<()> {
        let accent_changed;
        {
            let mut inner = self.inner.lock().expect("hud lock");
            accent_changed = inner.config.accent_hex() != config.accent_hex();
            // Size and edge changes move everything the webview reported.
            if inner.config.size != config.size || inner.config.edge != config.edge {
                inner.regions = None;
            }
            inner.config = config.clone();
        }

        if let Some(handle) = self.handle() {
            // A field being edited (a colour being picked) keeps its focus:
            // the chrome leaves the notch activatable while that lasts.
            platform::apply_hud_chrome(handle, config.click_through_when_collapsed)?;
            #[cfg(target_os = "macos")]
            platform::set_over_fullscreen(handle, !config.stay_below_fullscreen);
            // It does re-assert topmost; undo that if we're meant to be behind
            // the app in front.
            let below = self.inner.lock().expect("hud lock").below;
            self.restate_layer(handle, below);
            if accent_changed {
                platform::apply_appearance(handle, Backdrop::None);
                platform::refresh_frame(handle);
            }
        }
        self.apply()
    }

    pub fn state(&self) -> HudState {
        let inner = self.inner.lock().expect("hud lock");
        let (width, height) = inner
            .last_area
            .map_or((0.0, 0.0), |area| inner.extent(area));
        HudState {
            open: inner.open(),
            pinned: inner.pinned,
            hidden: inner.config.hidden,
            peeking: inner.peek_until.is_some_and(|t| Instant::now() < t),
            tucked: inner.below.is_some() || inner.tucking.is_some(),
            width,
            height,
        }
    }

    /// Which edge the strip is docked to, for the webview's own layout.
    pub fn edge(&self) -> Edge {
        self.inner.lock().expect("hud lock").config.edge
    }
}
