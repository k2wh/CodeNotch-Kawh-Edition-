//! Platform integration for the HUD window.
//!
//! Windows, Linux (X11) and macOS have real implementations; the stub exists
//! so the crate still type-checks anywhere else.

#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::*;

#[cfg(target_os = "linux")]
mod linux_impl;
#[cfg(target_os = "linux")]
pub use linux_impl::*;

#[cfg(target_os = "macos")]
mod macos_impl;
#[cfg(target_os = "macos")]
pub use macos_impl::*;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod stub;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub use stub::*;

/// A native window handle, passed around as an integer so the signature is the
/// same on every platform: an `HWND` on Windows, an X11 window id on Linux, an
/// `NSWindow` for the notch on macOS and a process id for other apps there.
pub type WindowHandle = isize;

/// Which desktop compositor effect to request behind the HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    /// Don't touch the backdrop.
    ///
    /// This is the default, because `tauri.conf.json` already declares an
    /// `acrylic` window effect and Tauri applies it through the path that also
    /// works on Windows 10. Setting `DWMWA_SYSTEMBACKDROP_TYPE` on top of that
    /// means two different mechanisms fighting over the same surface, with the
    /// result depending on which ran last.
    Inherit,
    /// Win11 "transient window" acrylic: the frosted look, best over content.
    Acrylic,
    /// Win11 Mica: tints from the desktop wallpaper, cheaper to composite.
    Mica,
    /// Explicitly no backdrop: the webview paints its own background.
    None,
}
