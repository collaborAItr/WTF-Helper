//! WTF Helper: read-only computer checks for a collaborAItr guide, each approved by the person.

pub mod checks;
pub mod flavor;
pub mod gate;
pub mod helper;
pub mod protocol;
pub mod relay;
pub mod settings;
pub mod timeutil;
pub mod tokens;

use std::sync::{Arc, OnceLock};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent, Wry};

use checks::osquery::{Osquery, RealMachine};
use checks::redact::Redactor;
use flavor::FLAVOR;
use helper::{Connection, Helper, StatusView};
use relay::Api;
use settings::SettingsFile;
use tokens::KeychainStore;

type AppHelper = Arc<Helper<RealMachine>>;

static PAUSE_ITEM: OnceLock<CheckMenuItem<Wry>> = OnceLock::new();
static TRAY_ID: &str = "wtf-helper-tray";

fn tooltip(status: &StatusView) -> String {
    let state = if status.paired.is_none() {
        "not connected"
    } else if status.paused {
        "paused"
    } else {
        match status.connection {
            Connection::Connected => "connected",
            Connection::Connecting | Connection::Reconnecting => "reconnecting",
            Connection::ServiceOff => "not switched on yet",
            Connection::NotPaired => "not connected",
        }
    };
    format!("{}: {state}", FLAVOR.product_name)
}

fn reflect(app: &AppHandle, status: &StatusView) {
    let _ = app.emit("status", status);
    if let Some(item) = PAUSE_ITEM.get() {
        let _ = item.set_checked(status.paused);
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(tooltip(status)));
    }
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn get_status(helper: tauri::State<'_, AppHelper>) -> StatusView {
    helper.status()
}

#[tauri::command]
async fn pair(code: String, helper: tauri::State<'_, AppHelper>) -> Result<StatusView, String> {
    helper.inner().pair(&code).await
}

#[tauri::command]
fn set_paused(paused: bool, helper: tauri::State<'_, AppHelper>) -> Result<StatusView, String> {
    helper.set_paused(paused)
}

#[tauri::command]
fn disconnect(helper: tauri::State<'_, AppHelper>) -> StatusView {
    helper.disconnect(None)
}

fn build_tray(app: &AppHandle, helper: &AppHelper) -> tauri::Result<()> {
    let open = MenuItem::with_id(
        app,
        "open",
        format!("Open {}", FLAVOR.product_name),
        true,
        None::<&str>,
    )?;
    let pause = CheckMenuItem::with_id(
        app,
        "pause",
        "Pause all checks",
        true,
        helper.is_paused(),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        format!("Quit {}", FLAVOR.product_name),
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[&open, &pause, &PredefinedMenuItem::separator(app)?, &quit],
    )?;
    let _ = PAUSE_ITEM.set(pause);

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip(tooltip(&helper.status()))
        .show_menu_on_left_click(cfg!(target_os = "macos"));
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .icon(tauri::image::Image::from_bytes(include_bytes!(
                "../icons/tray-template.png"
            ))?)
            .icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
    }
    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_window(app),
            "pause" => {
                let helper = app.state::<AppHelper>();
                let _ = helper.set_paused(!helper.is_paused());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if !cfg!(target_os = "macos") {
                    show_window(tray.app_handle());
                }
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app)
        }))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let config_dir = app.path().app_config_dir()?;
            let scratch_home = app.path().app_cache_dir()?.join("osquery-home");
            let api = Api::official();
            let machine = RealMachine {
                osquery: Osquery::locate(scratch_home),
                redactor: Redactor::for_this_computer(),
                api: api.clone(),
            };
            let notify_handle = handle.clone();
            let helper: AppHelper = Helper::new(
                api,
                machine,
                Box::new(KeychainStore),
                SettingsFile::new(&config_dir),
                Box::new(move |status| reflect(&notify_handle, status)),
            );
            app.manage(helper.clone());
            build_tray(&handle, &helper)?;
            tauri::async_runtime::spawn(async move { helper.start_stream() });
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps the helper running in the tray / menu bar.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status, pair, set_paused, disconnect
        ])
        .run(tauri::generate_context!())
        .expect("error while running WTF Helper");
}
