//! System tray icon and menu.
//!
//! The tray is the only always-visible affordance when the HUD is hidden, so it
//! carries the show/hide toggle and a way out of the app.

use anyhow::Result;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use codenotch_core::config::Config;

use crate::commands::app_state;
use crate::platform;

const ID_SHOW: &str = "toggle-visible";
const ID_PIN: &str = "toggle-pin";
const ID_REFRESH: &str = "refresh";
const ID_SETTINGS: &str = "open-config";
const ID_QUIT: &str = "quit";

/// The menu's items, kept so a settings change can re-label them and every
/// show/hide path can update the checkmark, not just the menu's own handler.
pub struct TrayMenu {
    show: CheckMenuItem<Wry>,
    pin: MenuItem<Wry>,
    refresh: MenuItem<Wry>,
    settings: MenuItem<Wry>,
    quit: MenuItem<Wry>,
}

/// Menu text in one language.
struct Labels {
    show: &'static str,
    pin: &'static str,
    refresh: &'static str,
    settings: &'static str,
    quit: &'static str,
}

/// The language a setting resolves to: "auto" follows Windows.
fn resolve(setting: &str) -> &'static str {
    match setting {
        "en" => "en",
        "pt" => "pt",
        "es" => "es",
        _ => platform::system_language(),
    }
}

fn labels(setting: &str) -> Labels {
    match resolve(setting) {
        "pt" => Labels {
            show: "Mostrar notch",
            pin: "Manter expandido",
            refresh: "Atualizar agora",
            settings: "Abrir pasta de configuração",
            quit: "Sair do CodeNotch",
        },
        "es" => Labels {
            show: "Mostrar notch",
            pin: "Mantener expandido",
            refresh: "Actualizar ahora",
            settings: "Abrir carpeta de configuración",
            quit: "Salir de CodeNotch",
        },
        _ => Labels {
            show: "Show notch",
            pin: "Keep expanded",
            refresh: "Refresh now",
            settings: "Open config folder",
            quit: "Quit CodeNotch",
        },
    }
}

/// Bring the menu in line with the config: its language and the show check.
pub fn sync(app: &AppHandle, config: &Config) {
    let Some(menu) = app.try_state::<TrayMenu>() else {
        return;
    };
    let text = labels(&config.language);
    let _ = menu.show.set_text(text.show);
    let _ = menu.pin.set_text(text.pin);
    let _ = menu.refresh.set_text(text.refresh);
    let _ = menu.settings.set_text(text.settings);
    let _ = menu.quit.set_text(text.quit);
    let _ = menu.show.set_checked(!config.hidden);
}

/// Build the tray icon and wire its menu.
pub fn build(app: &AppHandle) -> Result<()> {
    let config = app_state(app).map(|s| s.hud.config()).unwrap_or_default();
    let text = labels(&config.language);

    let show = CheckMenuItem::with_id(app, ID_SHOW, text.show, true, !config.hidden, None::<&str>)?;
    let pin = MenuItem::with_id(app, ID_PIN, text.pin, true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, ID_REFRESH, text.refresh, true, None::<&str>)?;
    let settings = MenuItem::with_id(app, ID_SETTINGS, text.settings, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, ID_QUIT, text.quit, true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &pin,
            &PredefinedMenuItem::separator(app)?,
            &refresh,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    app.manage(TrayMenu {
        show: show.clone(),
        pin: pin.clone(),
        refresh: refresh.clone(),
        settings: settings.clone(),
        quit: quit.clone(),
    });

    TrayIconBuilder::with_id("codenotch-tray")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no default window icon was bundled"))?,
        )
        .tooltip("CodeNotch")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            let handle = app.app_handle().clone();
            match event.id().as_ref() {
                ID_SHOW => {
                    if let Some(state) = app.try_state::<crate::commands::AppState>() {
                        let hidden = state.hud.config().hidden;
                        tauri::async_runtime::spawn(async move {
                            let _ = crate::commands::set_hidden_everywhere(&handle, !hidden).await;
                        });
                    }
                }
                ID_PIN => {
                    if let Some(state) = app.try_state::<crate::commands::AppState>() {
                        let _ = state.hud.toggle_pin();
                    }
                }
                ID_REFRESH => {
                    // The command is async; spawn so the menu handler returns.
                    tauri::async_runtime::spawn(async move {
                        crate::poll::refresh(&handle).await;
                    });
                }
                ID_SETTINGS => {
                    let _ = crate::commands::open_config_dir(handle);
                }
                ID_QUIT => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            // Left-clicking the tray icon peeks the HUD, which is the quickest
            // way to check usage when the notch is hidden.
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::commands::set_hidden_everywhere(&app, false).await;
                    if let Some(state) = app.try_state::<crate::commands::AppState>() {
                        let _ = state.hud.peek(std::time::Duration::from_secs(6));
                    }
                });
            }
        })
        .build(app)?;

    Ok(())
}
