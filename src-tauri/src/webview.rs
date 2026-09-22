//! WebView2 memory tuning.
//!
//! Part of it is in `tauri.conf.json`'s `additionalBrowserArgs`, which switch
//! off browser machinery a notch has no use for: extensions, sync, component
//! updates, background networking. (The `msWebOOUI` flags in that string are
//! Tauri's own defaults, which the setting replaces.) GPU rendering stays on:
//! measured here, the idle trimming below reclaims what the GPU process holds
//! anyway, so turning it off saved nothing and only cost smoothness.
//!
//! The notch spends nearly all its time resting, with nothing to draw and
//! nobody looking at it. WebView2 can be told so: `MemoryUsageTargetLevel`
//! asks it to trim what it keeps around, and it restores itself on demand.
//! The caches come back when the notch opens, so the only cost is a little
//! extra work on the first frame after a long rest.

use tauri::WebviewWindow;

/// Tell WebView2 to hold on to as little as it can (`low`), or to run
/// normally again.
#[cfg(windows)]
pub fn set_low_memory(window: &WebviewWindow, low: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    };
    use windows_core::Interface;

    let level = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };

    // Older WebView2 runtimes don't have the interface; the cast fails and
    // the HUD carries on unchanged.
    let _ = window.with_webview(move |webview| unsafe {
        if let Ok(core) = webview.controller().CoreWebView2() {
            if let Ok(memory) = core.cast::<ICoreWebView2_19>() {
                let _ = memory.SetMemoryUsageTargetLevel(level);
            }
        }
    });
}

#[cfg(not(windows))]
pub fn set_low_memory(_window: &WebviewWindow, _low: bool) {}
