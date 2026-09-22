//! Win32 window management for the notch.
//!
//! Four things matter here, and all of them are things a normal window gets
//! wrong for this use case:
//!
//! 1. **Never steal focus.** `WS_EX_NOACTIVATE` plus `SWP_NOACTIVATE` on every
//!    move means typing in the IDE or terminal is never interrupted, even as
//!    the HUD resizes underneath the pointer. The style is guarded, because
//!    the window layer underneath rewrites it (see `guard_hud_style`).
//! 2. **Stay out of the way.** `WS_EX_TOOLWINDOW` (and clearing
//!    `WS_EX_APPWINDOW`) keeps the notch out of Alt+Tab and off the taskbar.
//! 3. **Stay on top.** `HWND_TOPMOST`, reasserted periodically because other
//!    topmost windows (and full-screen apps) can displace us.
//! 4. **Click-through when resting.** `WS_EX_TRANSPARENT` is toggled so the
//!    collapsed pill doesn't swallow clicks meant for whatever is underneath.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{anyhow, Result};

use codenotch_core::config::Edge;
use codenotch_core::layout::{place, Placement, WorkArea};

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CLOAKED,
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWM_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, CreateRectRgn, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, MonitorFromWindow, ReleaseDC,
    SelectObject, SetBrushOrgEx, SetStretchBltMode, SetWindowRgn, StretchBlt, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HALFTONE, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, SRCCOPY,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow,
    GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindow, IsWindowVisible,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GA_ROOTOWNER, GWL_EXSTYLE, GW_OWNER, HWND_NOTOPMOST, HWND_TOPMOST, LWA_ALPHA, STYLESTRUCT,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_RESTORE, WM_STYLECHANGING,
    WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
};

use super::{Backdrop, WindowHandle};

/// Win11 build 22621 names these; older SDK headers don't have the constants.
const DWMSBT_NONE: i32 = 1;
/// `DWMWA_COLOR_NONE`: suppress the frame border entirely.
const DWM_COLOR_NONE: u32 = 0xFFFF_FFFE;
const DWMSBT_MAINWINDOW: i32 = 2; // Mica
const DWMSBT_TRANSIENTWINDOW: i32 = 3; // Acrylic

fn hwnd(handle: WindowHandle) -> HWND {
    HWND(handle as *mut core::ffi::c_void)
}

/// Whether the notch lets clicks through to what is underneath right now.
static CLICK_THROUGH: AtomicBool = AtomicBool::new(false);
/// Whether the notch may take the focus right now: a field is being edited.
static ACTIVATABLE: AtomicBool = AtomicBool::new(false);

/// The extended style the notch must have, whatever else was asked of it:
/// out of Alt+Tab and the taskbar, layered for its alpha, never activated by
/// a click unless a field is being edited, and click-through while resting.
fn hud_ex_style(requested: u32) -> u32 {
    let activatable = ACTIVATABLE.load(Ordering::Relaxed);
    let mut style = (requested | WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0) & !WS_EX_APPWINDOW.0;
    if activatable {
        style &= !WS_EX_NOACTIVATE.0;
    } else {
        style |= WS_EX_NOACTIVATE.0;
    }
    if CLICK_THROUGH.load(Ordering::Relaxed) && !activatable {
        style | WS_EX_TRANSPARENT.0
    } else {
        style & !WS_EX_TRANSPARENT.0
    }
}

/// Bring the window's extended style in line with [`hud_ex_style`].
fn restyle(hwnd: HWND) {
    // SAFETY: `hwnd` comes from Tauri and is valid for the window's lifetime.
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
        let style = hud_ex_style(current);
        if style != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style as isize);
        }
    }
}

/// Keep the notch's style from being undone behind its back.
///
/// The window layer underneath Tauri rewrites a window's whole extended style
/// from flags of its own whenever one of them changes (showing the window is
/// enough) and knows nothing of these bits. Unguarded, the first show after
/// launch left the notch activatable: a click on it took the focus from the
/// app the user was in, and it turned up in Alt+Tab. Now every rewrite, from
/// whoever, passes through [`hud_ex_style`] on its way in.
///
/// Call once, from the thread that owns the window.
pub fn guard_hud_style(handle: WindowHandle) {
    unsafe extern "system" fn guard(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if message == WM_STYLECHANGING && wparam.0 as i32 == GWL_EXSTYLE.0 {
            // SAFETY: for WM_STYLECHANGING, lParam points at the STYLESTRUCT
            // the system reads the new style back from once this returns.
            let styles = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
            styles.styleNew = hud_ex_style(styles.styleNew);
        }
        // SAFETY: the message goes on to the window's own handler.
        unsafe { DefSubclassProc(window, message, wparam, lparam) }
    }

    // SAFETY: `guard` is a plain function, so it outlives the window.
    unsafe {
        let _ = SetWindowSubclass(hwnd(handle), Some(guard), 1, 0);
    }
}

/// Apply the extended styles that make this window behave like a HUD (see
/// [`hud_ex_style`]), letting clicks through or not.
///
/// Safe to call repeatedly; each call re-reads the current style bits so it
/// composes with whatever the webview host has set.
pub fn apply_hud_chrome(handle: WindowHandle, click_through: bool) -> Result<()> {
    CLICK_THROUGH.store(click_through, Ordering::Relaxed);
    let hwnd = hwnd(handle);
    restyle(hwnd);

    // SAFETY: `hwnd` comes from Tauri and is valid for the window's lifetime.
    unsafe {
        // WS_EX_LAYERED windows start fully transparent until an alpha is set.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
    }

    set_topmost(handle)?;
    Ok(())
}

/// Toggle click-through without disturbing the other style bits.
pub fn set_click_through(handle: WindowHandle, enabled: bool) -> Result<()> {
    CLICK_THROUGH.store(enabled, Ordering::Relaxed);
    restyle(hwnd(handle));
    Ok(())
}

/// The window region already decides which clicks the notch takes.
pub fn pointer_at(_x: i32, _y: i32) {}

/// Let the window take keyboard focus (for editing a field), or return it to
/// never-activate. Turning it on also brings it to the foreground so the
/// keystrokes that follow land here rather than in the app underneath.
pub fn set_activatable(handle: WindowHandle, enabled: bool) -> Result<()> {
    ACTIVATABLE.store(enabled, Ordering::Relaxed);
    let hwnd = hwnd(handle);
    restyle(hwnd);
    if enabled {
        // SAFETY: see `apply_hud_chrome`.
        unsafe {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    Ok(())
}

/// Re-assert topmost z-order without moving, resizing or activating.
pub fn set_topmost(handle: WindowHandle) -> Result<()> {
    // SAFETY: flags guarantee position/size arguments are ignored.
    unsafe {
        SetWindowPos(
            hwnd(handle),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

/// Drop out of the topmost band and sit directly beneath `other`, for when
/// the user has asked the notch to stay behind the app in front.
pub fn place_below(handle: WindowHandle, other: WindowHandle) -> Result<()> {
    // SAFETY: flags guarantee position/size arguments are ignored; `other` is
    // validated by the API and a stale handle just fails the call.
    unsafe {
        SetWindowPos(
            hwnd(handle),
            Some(HWND_NOTOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )?;
        let _ = SetWindowPos(
            hwnd(handle),
            Some(hwnd(other)),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    Ok(())
}

/// Move and resize in physical pixels, without activating the window.
///
/// This is the call that makes hover-expand feel right: the window can grow
/// under the pointer while the user keeps typing somewhere else. It leaves the
/// z-order alone: that belongs to `set_topmost` / `place_below`, and a resize
/// must not pull the notch back over an app it was told to stay behind.
pub fn move_no_activate(handle: WindowHandle, placement: Placement) -> Result<()> {
    // SAFETY: `hwnd` is valid; SWP_NOACTIVATE keeps focus where it is.
    unsafe {
        SetWindowPos(
            hwnd(handle),
            None,
            placement.x,
            placement.y,
            placement.width,
            placement.height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )?;
    }
    Ok(())
}

/// The window the user is working in right now.
#[derive(Debug, Clone)]
pub struct Foreground {
    pub window: WindowHandle,
    pub pid: u32,
    /// Executable name, e.g. `chrome.exe`.
    pub image: String,
    /// Covers its whole monitor (a game, a video, a presentation).
    pub fullscreen: bool,
}

/// Window classes that fill the screen without being an app: the desktop.
const SHELL_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 128];
    // SAFETY: the buffer length is passed alongside it.
    let len = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..len.max(0) as usize])
}

/// Describe the foreground window, or `None` when there isn't a real one.
pub fn foreground() -> Option<Foreground> {
    // SAFETY: every handle is validated by the API that receives it.
    unsafe {
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let image = process_image_name(pid)?;

        let fullscreen = !SHELL_CLASSES.contains(&class_name(window).as_str()) && {
            let mut rect = RECT::default();
            let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            GetWindowRect(window, &mut rect).is_ok()
                && GetMonitorInfoW(monitor, &mut info).as_bool()
                && rect.left <= info.rcMonitor.left
                && rect.top <= info.rcMonitor.top
                && rect.right >= info.rcMonitor.right
                && rect.bottom >= info.rcMonitor.bottom
        };

        Some(Foreground {
            window: window.0 as WindowHandle,
            pid,
            image,
            fullscreen,
        })
    }
}

/// The Windows display language as one of the interface languages, for the
/// "auto" setting: `pt`, `es`, or `en` for anything else.
pub fn system_language() -> &'static str {
    // SAFETY: no arguments; returns a LANGID.
    let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    match langid & 0x3ff {
        0x16 => "pt", // LANG_PORTUGUESE
        0x0a => "es", // LANG_SPANISH
        _ => "en",
    }
}

/// One open window, as the "stay behind" picker shows it.
#[derive(Debug, Clone)]
pub struct WindowShot {
    pub id: WindowHandle,
    pub title: String,
    /// Executable name, e.g. `chrome.exe`.
    pub image: String,
    /// PNG thumbnail, or `None` for a minimised or uncapturable window.
    pub thumbnail: Option<Vec<u8>>,
    pub minimized: bool,
}

/// Real app windows a person would recognise in Alt+Tab: visible, titled,
/// unowned, not a tool palette, not cloaked (suspended UWP apps and windows on
/// other virtual desktops are "visible" but not on screen), and not ours.
fn is_app_window(window: HWND) -> bool {
    // SAFETY: `window` comes from EnumWindows and is validated by each API.
    unsafe {
        if !IsWindowVisible(window).as_bool() || GetWindowTextLengthW(window) <= 0 {
            return false;
        }
        if GetWindow(window, GW_OWNER).is_ok_and(|owner| !owner.0.is_null()) {
            return false;
        }
        let ex = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
        if cloaked != 0 {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        pid != std::process::id()
    }
}

/// Every app window with a thumbnail `thumb_width` physical pixels wide, in
/// z-order (most recently used first).
pub fn open_windows(thumb_width: i32) -> Vec<WindowShot> {
    unsafe extern "system" fn callback(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the &mut Vec passed to EnumWindows.
        let out = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
        if is_app_window(window) {
            out.push(window);
        }
        TRUE
    }

    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `windows` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(
            Some(callback),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        );
    }

    windows
        .into_iter()
        .filter_map(|window| {
            // SAFETY: plain queries on a window handle the API validates.
            unsafe {
                let len = GetWindowTextLengthW(window);
                let mut buffer = vec![0u16; len.max(0) as usize + 1];
                let written = GetWindowTextW(window, &mut buffer);
                let title = String::from_utf16_lossy(&buffer[..written.max(0) as usize]);

                let mut pid = 0u32;
                GetWindowThreadProcessId(window, Some(&mut pid));
                let image = process_image_name(pid)?;

                let minimized = IsIconic(window).as_bool();
                // A hung app would block PrintWindow (it waits on the target's
                // message loop), freezing the picker with it.
                let thumbnail = if minimized || IsHungAppWindow(window).as_bool() {
                    None
                } else {
                    capture_thumbnail(window, thumb_width)
                };

                Some(WindowShot {
                    id: window.0 as WindowHandle,
                    title,
                    image,
                    thumbnail,
                    minimized,
                })
            }
        })
        .collect()
}

/// Render a window off-screen and scale it to a PNG thumbnail.
///
/// `PrintWindow` with `PW_RENDERFULLCONTENT` asks DWM for the composed frame,
/// which is what makes GPU-drawn windows (browsers, Electron, UWP) come out as
/// something other than black. The invisible resize border Windows 10/11 puts
/// around every window is cropped away using the DWM frame bounds.
fn capture_thumbnail(window: HWND, thumb_width: i32) -> Option<Vec<u8>> {
    const PW_RENDERFULLCONTENT: u32 = 0x2;

    // SAFETY: every GDI object created here is released on every path below.
    unsafe {
        let mut outer = RECT::default();
        GetWindowRect(window, &mut outer).ok()?;
        let mut visible = outer;
        let _ = DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut visible as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        );

        let (w, h) = (outer.right - outer.left, outer.bottom - outer.top);
        let (vw, vh) = (visible.right - visible.left, visible.bottom - visible.top);
        if w < 32 || h < 32 || vw < 32 || vh < 32 {
            return None;
        }
        let (crop_x, crop_y) = (visible.left - outer.left, visible.top - outer.top);

        let tw = thumb_width.min(vw);
        let th = ((vh as f64 * tw as f64 / vw as f64).round() as i32).clamp(1, tw * 2);

        let screen = GetDC(None);
        let full_dc = CreateCompatibleDC(Some(screen));
        let full_bmp = CreateCompatibleBitmap(screen, w, h);
        let thumb_dc = CreateCompatibleDC(Some(screen));
        let thumb_bmp = CreateCompatibleBitmap(screen, tw, th);

        let old_full = SelectObject(full_dc, full_bmp.into());
        let old_thumb = SelectObject(thumb_dc, thumb_bmp.into());

        let printed =
            PrintWindow(window, full_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        if printed {
            SetStretchBltMode(thumb_dc, HALFTONE);
            let _ = SetBrushOrgEx(thumb_dc, 0, 0, None);
            let _ = StretchBlt(
                thumb_dc,
                0,
                0,
                tw,
                th,
                Some(full_dc),
                crop_x,
                crop_y,
                vw,
                vh,
                SRCCOPY,
            );
        }

        // GetDIBits needs the bitmap *out* of any device context.
        SelectObject(thumb_dc, old_thumb);
        SelectObject(full_dc, old_full);

        let mut pixels = vec![0u8; (tw * th * 4) as usize];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: tw,
                // Negative height: top-down rows, the order PNG wants.
                biHeight: -th,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = if printed {
            GetDIBits(
                thumb_dc,
                thumb_bmp,
                0,
                th as u32,
                Some(pixels.as_mut_ptr() as *mut core::ffi::c_void),
                &mut info,
                DIB_RGB_COLORS,
            )
        } else {
            0
        };

        let _ = DeleteObject(thumb_bmp.into());
        let _ = DeleteObject(full_bmp.into());
        let _ = DeleteDC(thumb_dc);
        let _ = DeleteDC(full_dc);
        ReleaseDC(None, screen);

        if lines <= 0 {
            return None;
        }

        // BGRA -> RGB. A frame that came back entirely black is a failed
        // capture (some protected or exclusive-fullscreen windows), not a
        // thumbnail worth showing.
        let mut rgb = Vec::with_capacity((tw * th * 3) as usize);
        let mut lit = false;
        for px in pixels.chunks_exact(4) {
            lit |= px[0] > 8 || px[1] > 8 || px[2] > 8;
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        if !lit {
            return None;
        }
        encode_png(&rgb, tw as u32, th as u32)
    }
}

fn encode_png(rgb: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgb).ok()?;
    }
    Some(out)
}

/// Executables that currently have a visible, titled window, for the "stay
/// behind these apps" picker. Sorted, de-duplicated, and without CodeNotch.
pub fn open_apps() -> Vec<String> {
    unsafe extern "system" fn callback(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the &mut Vec passed to EnumWindows.
        let out = unsafe { &mut *(data.0 as *mut Vec<String>) };
        // SAFETY: `window` is valid for the duration of the callback.
        unsafe {
            if !IsWindowVisible(window).as_bool() || GetWindowTextLengthW(window) <= 0 {
                return TRUE;
            }
            let ex = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                return TRUE; // floating palettes and HUDs, not apps
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if pid == std::process::id() {
                return TRUE;
            }
            if let Some(image) = process_image_name(pid) {
                out.push(image);
            }
        }
        TRUE
    }

    let mut found: Vec<String> = Vec::new();
    // SAFETY: `found` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(
            Some(callback),
            LPARAM(&mut found as *mut Vec<String> as isize),
        );
    }
    found.sort_by_key(|s| s.to_lowercase());
    found.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    found
}

/// Ask DWM for rounded corners, a dark frame and a backdrop.
///
/// Every call is best-effort: these attributes are Windows 11 era, and on
/// Windows 10 they simply return an error we ignore rather than failing setup.
pub fn apply_appearance(handle: WindowHandle, backdrop: Backdrop) {
    let hwnd = hwnd(handle);

    // SAFETY: each call passes a correctly sized value for its attribute.
    unsafe {
        let dark: BOOL = TRUE;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        );

        // The window is a mostly transparent box; rounding it only clips the
        // notch drawn inside, so let the webview shape the corners itself.
        let corner = DWMWCP_DONOTROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );

        // Suppress the frame border rather than tinting it.
        //
        // DWM draws this border around the *whole window rectangle*. The notch
        // window is mostly transparent -- it is sized to hold the popover
        // alongside the strip -- so a coloured border outlines that empty
        // rectangle on the desktop, which reads as a stray box floating next to
        // the notch rather than as trim on the notch itself. The accent belongs
        // on the rings, where the webview paints it.
        let border = COLORREF(DWM_COLOR_NONE);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &border as *const _ as *const _,
            std::mem::size_of::<COLORREF>() as u32,
        );

        // `Inherit` deliberately leaves the backdrop alone; see `Backdrop`.
        if let Some(kind) = match backdrop {
            Backdrop::Inherit => None,
            Backdrop::Acrylic => Some(DWMSBT_TRANSIENTWINDOW),
            Backdrop::Mica => Some(DWMSBT_MAINWINDOW),
            Backdrop::None => Some(DWMSBT_NONE),
        } {
            let value = DWM_SYSTEMBACKDROP_TYPE(kind);
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &value as *const _ as *const _,
                std::mem::size_of::<DWM_SYSTEMBACKDROP_TYPE>() as u32,
            );
        }
    }
}

/// `0xRRGGBB` -> `0x00BBGGRR`, the byte order COLORREF uses.
pub fn swap_rgb(rgb: u32) -> u32 {
    let r = (rgb >> 16) & 0xFF;
    let g = (rgb >> 8) & 0xFF;
    let b = rgb & 0xFF;
    (b << 16) | (g << 8) | r
}

/// Parse `#rrggbb` into `0xRRGGBB`.
pub fn parse_hex_colour(hex: &str) -> Option<u32> {
    let hex = hex.trim().strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok()
}

/// `MONITORINFOF_PRIMARY`.
const MONITOR_PRIMARY_FLAG: u32 = 1;

/// Every monitor's work area, in the order Windows reports them, paired with
/// whether it is the primary display.
///
/// The work area excludes the taskbar, which is what "docked near the taskbar"
/// has to mean if the notch isn't going to sit underneath it. Each area carries
/// its own monitor's DPI scale: the notch has to be sized for the display it is
/// going *to*, not the one it happens to be on now.
fn enumerate_monitors() -> Vec<(WorkArea, bool)> {
    let mut found: Vec<(WorkArea, bool)> = Vec::new();

    unsafe extern "system" fn callback(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the &mut Vec we passed to EnumDisplayMonitors, which
        // outlives the enumeration.
        let out = unsafe { &mut *(data.0 as *mut Vec<(WorkArea, bool)>) };

        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info.cbSize` is set as the API requires.
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
            // SAFETY: plain out-parameters; a failure leaves them at 0.
            let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
            let scale = if dpi_x == 0 { 1.0 } else { dpi_x as f64 / 96.0 };

            let work = info.rcWork;
            out.push((
                WorkArea::new(
                    work.left,
                    work.top,
                    work.right - work.left,
                    work.bottom - work.top,
                    scale,
                ),
                info.dwFlags & MONITOR_PRIMARY_FLAG != 0,
            ));
        }
        TRUE
    }

    // SAFETY: `found` outlives the synchronous enumeration below.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(callback),
            LPARAM(&mut found as *mut Vec<(WorkArea, bool)> as isize),
        );
    }
    found
}

/// Monitor work areas in the order Windows reports them.
pub fn monitors() -> Vec<WorkArea> {
    enumerate_monitors()
        .into_iter()
        .map(|(area, _)| area)
        .collect()
}

/// The primary monitor's work area, as Windows flags it. A taskbar docked to
/// the top or left moves the work area off the origin, so position alone
/// can't identify it.
pub fn primary_work_area() -> Option<WorkArea> {
    let all = enumerate_monitors();
    all.iter()
        .find(|(_, primary)| *primary)
        .or_else(|| all.first())
        .map(|(area, _)| *area)
}

/// Work area for a monitor index, falling back to the primary.
pub fn work_area_for(index: Option<usize>) -> Option<WorkArea> {
    match index {
        Some(i) => monitors().get(i).copied().or_else(primary_work_area),
        None => primary_work_area(),
    }
}

/// The DPI scale currently applied to this window (1.0 = 96 DPI).
pub fn window_scale(handle: WindowHandle) -> f64 {
    // SAFETY: GetDpiForWindow returns 0 for an invalid handle, handled below.
    let dpi = unsafe { GetDpiForWindow(hwnd(handle)) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f64 / 96.0
    }
}

/// Compute where the HUD belongs and move it there, in one call.
pub fn dock(
    handle: WindowHandle,
    monitor: Option<usize>,
    edge: Edge,
    offset: f32,
    margin: f64,
    logical_width: f64,
    logical_height: f64,
) -> Result<Placement> {
    // The area carries the target monitor's own DPI scale (see `monitors`).
    let area = work_area_for(monitor).ok_or_else(|| anyhow!("no monitors reported a work area"))?;

    let placement = place(area, edge, offset, margin, logical_width, logical_height);
    move_no_activate(handle, placement)?;
    Ok(placement)
}

/// Only X11 hands a window's real position back; elsewhere the window is
/// where it was put, and the caller's own record is the truth.
pub fn window_origin(_handle: WindowHandle) -> Option<(i32, i32)> {
    None
}

/// Clip the window to one rectangle (window-relative physical pixels), or
/// lift the clip with `None`.
///
/// Outside the region the window neither draws nor takes clicks, which is what
/// lets a mostly transparent, edge-long window sit on top of everything
/// without swallowing the clicks meant for the apps underneath.
pub fn set_region(handle: WindowHandle, rect: Option<(i32, i32, i32, i32)>) -> Result<()> {
    // SAFETY: on success the system owns the region, so it is never freed here;
    // on failure it is deleted below.
    unsafe {
        match rect {
            Some((left, top, right, bottom)) => {
                let region = CreateRectRgn(left, top, right, bottom);
                if region.is_invalid() {
                    return Err(anyhow!("CreateRectRgn failed"));
                }
                if SetWindowRgn(hwnd(handle), Some(region), true) == 0 {
                    let _ = DeleteObject(region.into());
                    return Err(anyhow!("SetWindowRgn failed"));
                }
            }
            None => {
                let _ = SetWindowRgn(hwnd(handle), None, true);
            }
        }
    }
    Ok(())
}

/// Where the mouse is, in physical screen pixels.
///
/// A click-through window (`WS_EX_TRANSPARENT`) receives no mouse messages at
/// all -- not even hover -- so the webview cannot detect the pointer arriving.
/// Polling the cursor is what lets the notch stay click-through while resting
/// and still open when you move onto it.
pub fn cursor_pos() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    // SAFETY: `point` is a plain out-parameter owned by this frame.
    unsafe { GetCursorPos(&mut point).ok()? };
    Some((point.x, point.y))
}

/// Basename of a process's executable, e.g. `Cursor.exe`.
fn process_image_name(pid: u32) -> Option<String> {
    // SAFETY: the handle is closed by its Owned wrapper when it drops.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buffer = [0u16; 260];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
        .is_ok();

        if !ok {
            return None;
        }
        let full = String::from_utf16_lossy(&buffer[..len as usize]);
        full.rsplit(['\\', '/']).next().map(str::to_string)
    }
}

/// A candidate window found while searching for a provider's UI.
struct Candidate {
    hwnd: HWND,
    title: String,
    image: String,
}

/// Find visible top-level windows whose executable matches `process_names`.
fn find_windows(process_names: &[&str]) -> Vec<Candidate> {
    struct Search {
        wanted: Vec<String>,
        found: Vec<Candidate>,
    }

    unsafe extern "system" fn callback(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the &mut Search passed to EnumWindows.
        let search = unsafe { &mut *(data.0 as *mut Search) };

        // SAFETY: `window` is valid for the duration of the callback.
        unsafe {
            if !IsWindowVisible(window).as_bool() {
                return TRUE;
            }
            let len = GetWindowTextLengthW(window);
            if len <= 0 {
                return TRUE; // untitled windows are tool/host windows, not the UI
            }

            let mut buffer = vec![0u16; len as usize + 1];
            let written = GetWindowTextW(window, &mut buffer);
            let title = String::from_utf16_lossy(&buffer[..written as usize]);

            let mut pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut pid));
            let Some(image) = process_image_name(pid) else {
                return TRUE;
            };

            if search.wanted.iter().any(|w| w.eq_ignore_ascii_case(&image)) {
                search.found.push(Candidate {
                    hwnd: window,
                    title,
                    image,
                });
            }
        }
        TRUE
    }

    let mut search = Search {
        wanted: process_names.iter().map(|s| s.to_string()).collect(),
        found: Vec::new(),
    };

    // SAFETY: `search` outlives the synchronous enumeration.
    unsafe {
        let _ = EnumWindows(Some(callback), LPARAM(&mut search as *mut Search as isize));
    }
    search.found
}

/// Bring a window to the foreground.
///
/// Windows only lets the *foreground* thread set the foreground window, so the
/// usual trick is to attach our input queue to the current foreground thread
/// for the duration of the call. Without it `SetForegroundWindow` silently
/// no-ops and the taskbar button just flashes.
fn focus_window(target: HWND) -> bool {
    // SAFETY: every handle is validated by the API; the attach is undone below.
    unsafe {
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }

        let foreground = GetForegroundWindow();
        let foreground_thread = GetWindowThreadProcessId(foreground, None);
        let our_thread = GetCurrentThreadId();

        let attached = foreground_thread != 0
            && foreground_thread != our_thread
            && AttachThreadInput(our_thread, foreground_thread, true).as_bool();

        let _ = BringWindowToTop(target);
        let ok = SetForegroundWindow(target).as_bool();
        let _ = SetFocus(Some(target));

        if attached {
            let _ = AttachThreadInput(our_thread, foreground_thread, false);
        }
        ok
    }
}

/// The window a provider's card raises, or `None` when none of
/// `process_names` has a window open.
///
/// `title_hint` (typically the project folder) picks between several windows of
/// the same application, so clicking a card lands on the right project rather
/// than an arbitrary editor window.
pub fn provider_window(process_names: &[&str], title_hint: Option<&str>) -> Option<WindowHandle> {
    let candidates = find_windows(process_names);

    let chosen = title_hint
        .and_then(|hint| {
            let hint = hint.to_lowercase();
            candidates
                .iter()
                .find(|c| c.title.to_lowercase().contains(&hint))
        })
        // Otherwise prefer the earliest-listed executable, which is the order
        // the caller ranked them in.
        .or_else(|| {
            process_names.iter().find_map(|name| {
                candidates
                    .iter()
                    .find(|c| c.image.eq_ignore_ascii_case(name))
            })
        })
        .or_else(|| candidates.first());

    chosen.map(|c| c.hwnd.0 as WindowHandle)
}

/// Bring a window to the front. False when Windows refused.
pub fn bring_to_front(window: WindowHandle) -> bool {
    focus_window(hwnd(window))
}

/// The app window `window` belongs to: a dialog or a palette counts as the
/// window that owns it.
fn root(window: HWND) -> HWND {
    // SAFETY: a stale handle yields null, which falls back to the handle.
    let owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    if owner.0.is_null() {
        window
    } else {
        owner
    }
}

/// Whether two handles are the same app window, a dialog counting as its owner.
pub fn same_window(a: WindowHandle, b: WindowHandle) -> bool {
    root(hwnd(a)) == root(hwnd(b))
}

/// Whether `window`, or a dialog of it, is the one the user is in.
pub fn is_foreground(window: WindowHandle) -> bool {
    // SAFETY: no arguments; a null result never matches.
    let current = unsafe { GetForegroundWindow() };
    !current.0.is_null() && root(current) == root(hwnd(window))
}

/// Whether the window in front is ours: the notch, holding the focus while a
/// field on it is being edited.
pub fn foreground_is_ours() -> bool {
    // SAFETY: no pointer arguments; a null window belongs to no process.
    unsafe {
        let current = GetForegroundWindow();
        let mut pid = 0u32;
        !current.0.is_null()
            && GetWindowThreadProcessId(current, Some(&mut pid)) != 0
            && pid == std::process::id()
    }
}

/// Whether the user could be sent back to `window`: still open, on screen
/// rather than minimised, and an app window of the Alt+Tab kind — not the
/// desktop, the taskbar, a palette or the notch itself.
pub fn can_return_to(window: WindowHandle) -> bool {
    let window = hwnd(window);
    // SAFETY: `IsWindow` vets the handle before anything else reads from it.
    unsafe {
        IsWindow(Some(window)).as_bool()
            && !IsIconic(window).as_bool()
            && !SHELL_CLASSES.contains(&class_name(window).as_str())
            && is_app_window(window)
    }
}

/// The app window the user is working in, when it is one they could be sent
/// back to later (see [`can_return_to`]).
pub fn foreground_app() -> Option<WindowHandle> {
    // SAFETY: no arguments; a null result is handled.
    let current = unsafe { GetForegroundWindow() };
    if current.0.is_null() {
        return None;
    }
    let window = root(current).0 as WindowHandle;
    can_return_to(window).then_some(window)
}

/// Open (or bring forward) a Store app by its Application User Model ID, the
/// way the Start menu does. Returns false when it isn't installed.
pub fn activate_app(app_id: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, AO_NONE,
    };

    // SAFETY: COM is initialised for this thread first (a repeat call is
    // harmless); the interface is released when it drops.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Ok(manager) = CoCreateInstance::<_, IApplicationActivationManager>(
            &ApplicationActivationManager,
            None,
            CLSCTX_LOCAL_SERVER,
        ) else {
            return false;
        };
        manager
            .ActivateApplication(&HSTRING::from(app_id), None, AO_NONE)
            .is_ok()
    }
}

/// Registry path for per-user startup entries.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "CodeNotch";

/// Whether CodeNotch is registered to start at sign-in.
pub fn launch_at_login() -> bool {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegGetValueW, HKEY, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
    };

    // SAFETY: buffer size is passed and updated by the API.
    unsafe {
        let mut key = HKEY::default();
        let sub = wide(RUN_KEY);
        if windows::Win32::System::Registry::RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            None,
            windows::Win32::System::Registry::KEY_READ,
            &mut key,
        )
        .is_err()
        {
            return false;
        }

        let name = wide(RUN_VALUE);
        let mut size = 0u32;
        let present = RegGetValueW(
            key,
            None,
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .is_ok();

        let _ = RegCloseKey(key);
        present && size > 0
    }
}

/// Add or remove the startup entry.
pub fn set_launch_at_login(enabled: bool) -> Result<()> {
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_SZ,
    };

    let exe = std::env::current_exe()?;
    // Quote the path: Run entries are parsed as command lines, and
    // `C:\Program Files\...` would otherwise split at the space.
    let command = format!("\"{}\"", exe.display());

    // SAFETY: handles are closed on every path; buffers are NUL-terminated.
    unsafe {
        let mut key = HKEY::default();
        let sub = wide(RUN_KEY);
        windows::Win32::System::Registry::RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(sub.as_ptr()),
            None,
            KEY_SET_VALUE,
            &mut key,
        )
        .ok()?;

        let name = wide(RUN_VALUE);
        let result = if enabled {
            let value = wide(&command);
            let bytes = std::slice::from_raw_parts(
                value.as_ptr() as *const u8,
                value.len() * std::mem::size_of::<u16>(),
            );
            RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)).ok()
        } else {
            // Removing an entry that isn't there is a success, not a failure.
            let _ = RegDeleteValueW(key, PCWSTR(name.as_ptr()));
            Ok(())
        };

        let _ = RegCloseKey(key);
        result?;
    }
    Ok(())
}

/// NUL-terminated UTF-16, as every `W` API expects.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Nudge the window so DWM repaints the backdrop after a style change.
pub fn refresh_frame(handle: WindowHandle) {
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_THEMECHANGED};
    // SAFETY: a theme-changed notification carries no pointer payload.
    unsafe {
        let _ = SendMessageW(
            hwnd(handle),
            WM_THEMECHANGED,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
    }
}
