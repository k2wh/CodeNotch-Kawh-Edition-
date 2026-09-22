//! CodeNotch: a Windows HUD overlay for AI coding-assistant usage limits.
//!
//! The Tauri app is thin on purpose. All the collection logic lives in
//! `codenotch-core` (which builds and tests anywhere); this crate owns the
//! window, the Win32 integration, the tray, and the IPC surface.

pub mod commands;
pub mod focus;
pub mod hooks;
pub mod hud;
pub mod platform;
pub mod poll;
pub mod tray;
pub mod webview;

use std::sync::{Arc, Mutex};

use tauri::Manager;
use tokio::sync::Mutex as AsyncMutex;

use codenotch_core::config::Config;
use codenotch_core::model::Telemetry;
use codenotch_core::Collector;

use commands::AppState;
use hud::Hud;

/// Label of the HUD window declared in `tauri.conf.json`.
const HUD_WINDOW: &str = "hud";

/// Start again outside the MSIX container this was launched in, if any.
///
/// CodeNotch installs as a plain desktop app, so it never has a package
/// identity of its own; being given one means it was started from inside a
/// packaged app — Claude Desktop, say, launching it from a terminal of its
/// own. A container is not somewhere it can work from. Writes to `%APPDATA%`
/// are redirected into the host app's private store, so settings save to a
/// copy nothing else reads and appear to reset on the next normal launch; and
/// named objects are virtualised too, so the single-instance guard can't see
/// the notch already running outside, and you end up with two.
///
/// Explorer runs outside every container, so what it starts does too.
#[cfg(windows)]
fn relaunch_outside_container() -> bool {
    use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

    let mut len = 0u32;
    // Asking with no buffer: "too small" means there is a name to give, which
    // only a packaged process has.
    let packaged =
        unsafe { GetCurrentPackageFullName(&mut len, None) } == ERROR_INSUFFICIENT_BUFFER;
    if !packaged {
        return false;
    }

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    eprintln!("codenotch: started inside an app container; relaunching outside it");
    std::process::Command::new("explorer.exe")
        .arg(exe)
        .spawn()
        .is_ok()
}

/// Build and run the application.
pub fn run() {
    // Invoked as an agent's hook rather than as the app: report the event and
    // get out of the agent's way. Before anything else, so no window, tray
    // icon or single-instance check ever happens on this path.
    if hooks::run_as_hook_if_asked() {
        return;
    }

    // Started from inside another app's sandbox: start again outside it and
    // leave. Before `repair`, which writes to the agents' config files and
    // would write them into the sandbox's private copy.
    #[cfg(windows)]
    if relaunch_outside_container() {
        return;
    }

    // Linux: the notch is an X11 window (see `platform`), so under Wayland it
    // runs through XWayland rather than as a Wayland window it could neither
    // place nor keep on top. And WebKitGTK hands its frames over through the
    // graphics driver, which some drivers (NVIDIA's, which Pop!_OS ships) get
    // wrong, leaving the page blank; shared memory sidesteps that, at no cost
    // worth counting for a page this small. (Not the usual advice of turning
    // that renderer off: the older one never clears a transparent window, so
    // a card that closes stays on screen.) Both only when the user hasn't
    // said otherwise, and before any thread exists to read the environment.
    #[cfg(target_os = "linux")]
    for (name, value) in [
        ("GDK_BACKEND", "x11"),
        ("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1"),
    ] {
        if std::env::var_os(name).is_none() {
            std::env::set_var(name, value);
        }
    }

    // An update or a move leaves the agents calling a path that has gone;
    // point them back at this copy.
    hooks::repair();

    // The default async runtime starts a worker per CPU core: sixteen threads
    // to poll a few HTTP endpoints and read some files. Two is plenty, and
    // each thread saved is stack and bookkeeping not paid for.
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    if let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
    {
        tauri::async_runtime::set(RUNTIME.get_or_init(|| runtime).handle().clone());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("CODENOTCH_LOG")
                .unwrap_or_else(|_| "codenotch=info,codenotch_core=info".into()),
        )
        .with_target(false)
        .init();

    tauri::Builder::default()
        // A second launch should surface the existing notch, not start a rival
        // one fighting over the same screen corner.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // `codenotch.exe --quit`: ask the copy that is running to stop.
            // Killing it instead leaves its tray icon behind — Windows only
            // reaps an icon whose owner is gone when the pointer next passes
            // over it, so an installer that force-kills leaves the user
            // looking at two notches where there is one.
            if argv.iter().any(|arg| arg == "--quit") {
                app.exit(0);
                return;
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let _ = commands::set_hidden_everywhere(&app, false).await;
                if let Some(state) = app.try_state::<AppState>() {
                    let _ = state.hud.peek(std::time::Duration::from_secs(5));
                }
            });
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::hud_ready,
            commands::hud_hover,
            commands::hud_set_regions,
            commands::hud_set_interactive,
            commands::hud_toggle_pin,
            commands::get_config,
            commands::get_metrics,
            commands::set_config,
            commands::refresh_now,
            commands::focus_provider,
            commands::set_hidden,
            commands::peek,
            commands::list_monitors,
            commands::set_agent_hooks,
            commands::agent_hooks_installed,
            commands::list_open_apps,
            commands::list_open_windows,
            commands::open_config_dir,
            commands::quit_app,
        ])
        .setup(|app| {
            // `--quit` with nothing to quit: the single-instance plugin found
            // no other copy, so this one became it. Stop before there is a
            // notch on screen rather than after.
            if std::env::args().any(|arg| arg == "--quit") {
                app.handle().exit(0);
                return Ok(());
            }

            let config = Config::load();

            // A Mac app lives in the Dock unless it says otherwise. The notch
            // is its whole interface, with the menu bar icon beside it, as
            // the original's is.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let window = app
                .get_webview_window(HUD_WINDOW)
                .ok_or_else(|| format!("window `{HUD_WINDOW}` is missing from tauri.conf.json"))?;

            let hud = Arc::new(Hud::new(window.clone(), config.clone()));
            hud.initialise()?;

            // Moving between monitors with different scaling makes Windows
            // resize and move the window itself (WM_DPICHANGED). Re-dock
            // afterwards, or the notch drifts off the edge and the hover area no
            // longer matches it.
            // The resize Windows suggests is applied after this event returns,
            // so the re-dock waits a moment to land on top of it.
            let redock = hud.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::ScaleFactorChanged { scale_factor, .. } = event {
                    // Linux works its work areas out from it.
                    #[cfg(target_os = "linux")]
                    platform::note_scale(*scale_factor);
                    #[cfg(not(target_os = "linux"))]
                    let _ = scale_factor;
                    let redock = redock.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                        let _ = redock.apply();
                    });
                }
            });

            app.manage(AppState {
                hud: hud.clone(),
                collector: Arc::new(AsyncMutex::new(Collector::new(config))),
                latest: Arc::new(Mutex::new(Telemetry::empty())),
            });

            if let Err(err) = tray::build(&app.handle().clone()) {
                // A missing tray is survivable; a missing HUD is not.
                tracing::warn!(%err, "could not create the tray icon");
            }

            poll::spawn(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running CodeNotch");
}
