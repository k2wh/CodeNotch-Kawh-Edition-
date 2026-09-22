//! No-op platform layer for non-Windows hosts.
//!
//! CodeNotch is a Windows application; this exists purely so the crate
//! type-checks elsewhere (editors, CI containers without the Windows SDK).
//! Every function keeps the Windows signature and does nothing useful.

use anyhow::Result;

use codenotch_core::config::Edge;
use codenotch_core::layout::{place, Placement, WorkArea};

use super::{Backdrop, WindowHandle};

pub fn apply_hud_chrome(_handle: WindowHandle, _click_through: bool) -> Result<()> {
    Ok(())
}

pub fn set_click_through(_handle: WindowHandle, _enabled: bool) -> Result<()> {
    Ok(())
}

pub fn set_activatable(_handle: WindowHandle, _enabled: bool) -> Result<()> {
    Ok(())
}

pub fn set_topmost(_handle: WindowHandle) -> Result<()> {
    Ok(())
}

pub fn move_no_activate(_handle: WindowHandle, _placement: Placement) -> Result<()> {
    Ok(())
}

pub fn place_below(_handle: WindowHandle, _other: WindowHandle) -> Result<()> {
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Foreground {
    pub window: WindowHandle,
    pub pid: u32,
    pub image: String,
    pub fullscreen: bool,
}

pub fn foreground() -> Option<Foreground> {
    None
}

pub fn system_language() -> &'static str {
    "en"
}

pub fn open_apps() -> Vec<String> {
    Vec::new()
}

#[derive(Debug, Clone)]
pub struct WindowShot {
    pub id: WindowHandle,
    pub title: String,
    pub image: String,
    pub thumbnail: Option<Vec<u8>>,
    pub minimized: bool,
}

pub fn open_windows(_thumb_width: i32) -> Vec<WindowShot> {
    Vec::new()
}

pub fn apply_appearance(_handle: WindowHandle, _backdrop: Backdrop) {}

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

/// A plausible single 1080p monitor, so layout code has something to work with.
pub fn monitors() -> Vec<WorkArea> {
    vec![WorkArea::new(0, 0, 1920, 1040, 1.0)]
}

pub fn primary_work_area() -> Option<WorkArea> {
    monitors().first().copied()
}

pub fn work_area_for(index: Option<usize>) -> Option<WorkArea> {
    match index {
        Some(i) => monitors().get(i).copied().or_else(primary_work_area),
        None => primary_work_area(),
    }
}

pub fn window_scale(_handle: WindowHandle) -> f64 {
    1.0
}

pub fn dock(
    handle: WindowHandle,
    monitor: Option<usize>,
    edge: Edge,
    offset: f32,
    margin: f64,
    logical_width: f64,
    logical_height: f64,
) -> Result<Placement> {
    let area = work_area_for(monitor).unwrap_or(WorkArea::new(0, 0, 1920, 1040, 1.0));
    let placement = place(area, edge, offset, margin, logical_width, logical_height);
    move_no_activate(handle, placement)?;
    Ok(placement)
}

pub fn set_region(_handle: WindowHandle, _rect: Option<(i32, i32, i32, i32)>) -> Result<()> {
    Ok(())
}

pub fn provider_window(_process_names: &[&str], _title_hint: Option<&str>) -> Option<WindowHandle> {
    None
}

pub fn bring_to_front(_window: WindowHandle) -> bool {
    false
}

pub fn same_window(a: WindowHandle, b: WindowHandle) -> bool {
    a == b
}

pub fn is_foreground(_window: WindowHandle) -> bool {
    false
}

pub fn foreground_is_ours() -> bool {
    false
}

pub fn guard_hud_style(_handle: WindowHandle) {}

pub fn pointer_at(_x: i32, _y: i32) {}

pub fn can_return_to(_window: WindowHandle) -> bool {
    false
}

pub fn foreground_app() -> Option<WindowHandle> {
    None
}

pub fn activate_app(_app_id: &str) -> bool {
    false
}

pub fn launch_at_login() -> bool {
    false
}

pub fn set_launch_at_login(_enabled: bool) -> Result<()> {
    Ok(())
}

pub fn refresh_frame(_handle: WindowHandle) {}

/// No cursor to report; the stub relies on the webview's own hover events.
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}
