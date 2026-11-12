// En version finale, pas de console noire à côté de la fenêtre sous Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clipboard;
mod commands;
mod config;
mod files;
mod host;
mod net;
mod state;
mod update;
mod viewer;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent,
};

use crate::{config::Config, state::AppState};

/// Nom de ce poste, affiché chez le correspondant.
pub fn hostname() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Poste".into())
}

/// Affiche la fenêtre principale (depuis l'icône de notification).
fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Icône dans la zone de notification : l'application reste joignable
/// quand sa fenêtre est fermée.
fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Ouvrir Distant", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quitter Distant", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("Distant")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
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
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "distant=info".into()),
        )
        .init();

    // `DISTANT_PROFILE=b` : réglages (et donc ID) séparés, pratique pour lancer
    // deux instances sur la même machine pendant le développement.
    let profile = std::env::var("DISTANT_PROFILE").ok().filter(|p| !p.is_empty());

    let mut builder = tauri::Builder::default();
    if profile.is_none() {
        // Relancer Distant alors qu'il tourne déjà ré-affiche simplement sa fenêtre.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)));
    }
    builder
        // Lancé au démarrage de la session : démarre réduit dans la zone de notification.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let file = match &profile {
                Some(p) => format!("config-{p}.json"),
                None => "config.json".into(),
            };
            let config_path = app.path().app_config_dir()?.join(file);
            let mut config = Config::load(&config_path);
            tracing::info!(
                "Distant {} — ID {} — réglages : {}",
                env!("CARGO_PKG_VERSION"),
                config.id,
                config_path.display()
            );
            // Relancé par une mise à jour installée fenêtre fermée : on reste réduit, une fois.
            let resumed_hidden = std::mem::take(&mut config.start_hidden_once);
            if resumed_hidden {
                if let Err(e) = config.save(&config_path) {
                    tracing::warn!("impossible d'enregistrer les réglages : {e:#}");
                }
            }

            app.manage(AppState::new(config_path, config));
            host::spawn_service(app.handle().clone());
            update::spawn_service(app.handle().clone());
            build_tray(app)?;

            // La fenêtre est créée masquée (tauri.conf.json) pour ne pas clignoter
            // quand l'application démarre réduite.
            if !resumed_hidden && !std::env::args().any(|a| a == "--minimized") {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Fermer la fenêtre principale la masque : le poste reste joignable.
            // « Quitter Distant » dans l'icône de notification arrête l'application.
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Destroyed => {
                if let Some(sid) = window.label().strip_prefix("viewer-").and_then(|s| s.parse().ok()) {
                    viewer::disconnect(window.app_handle(), sid);
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_overview,
            commands::regenerate_password,
            commands::save_settings,
            commands::set_accept_incoming,
            commands::set_unattended_password,
            commands::set_autostart,
            commands::forget_machine,
            commands::get_permissions,
            commands::open_permission_settings,
            commands::reset_permissions,
            commands::restart_app,
            commands::end_incoming,
            commands::connect,
            commands::connect_saved,
            commands::viewer_attach,
            commands::viewer_ack,
            commands::viewer_input,
            commands::viewer_set_monitor,
            commands::viewer_set_quality,
            commands::viewer_pick_files,
            commands::viewer_send_files,
            commands::viewer_cancel_file,
            commands::viewer_disconnect,
            commands::viewer_focus,
            commands::viewer_set_clipboard,
            update::update_check,
            update::update_install,
            update::set_auto_update,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Distant");
}
