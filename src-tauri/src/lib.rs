mod commands;
mod coordinate_mapper;
mod game_input_dispatcher;
mod input_capture;
mod input_dispatcher;
mod layout_engine;
mod models;
mod permissions;
mod platform;
mod profile_store;
mod state;
mod sync_session;
mod window_registry;

use tauri::Manager;

use models::LogLevel;
use state::{push_log, AppState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .manage(AppState::default())
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }

      if let Err(error) = permissions::initialize_platform() {
        log::warn!("Platform bootstrap warning: {error:#}");
      }

      let state = app.state::<AppState>();
      push_log(
        &app.handle(),
        &state,
        LogLevel::Info,
        "Mirror Windows shell ready. Window enumeration, layout control, and live Windows input sync are enabled.",
      );
      push_log(
        &app.handle(),
        &state,
        LogLevel::Warn,
        "Background mouse and keyboard mirroring now run best-effort through Windows hooks plus message dispatch. Browser, Electron, IME, and custom-rendered targets can still behave differently.",
      );

      if let Ok(permission_status) = permissions::current_permission_status() {
        if permission_status.is_process_elevated {
          push_log(
            &app.handle(),
            &state,
            LogLevel::Warn,
            "The control panel is running elevated. Future input replay will only be compatible with elevated target apps at the same integrity level.",
          );
        }
      }

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      commands::scan_windows,
      commands::list_monitors,
      commands::get_permission_status,
      commands::create_session,
      commands::update_session,
      commands::list_sessions,
      commands::apply_layout,
      commands::start_sync,
      commands::stop_sync,
      commands::save_profile,
      commands::load_profiles,
      commands::get_logs
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
