use tauri::{AppHandle, State};

use crate::models::{
  LogEntry, MonitorInfo, PermissionStatus, ProfileDraft, ProfileRecord, SessionConfig, SessionInfo,
  WindowInfo,
};
use crate::state::{list_all_logs, push_log, AppState};
use crate::{permissions, profile_store, sync_session, window_registry};

type CommandResult<T> = Result<T, String>;

#[tauri::command]
pub fn scan_windows() -> CommandResult<Vec<WindowInfo>> {
  window_registry::scan_windows().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_monitors() -> CommandResult<Vec<MonitorInfo>> {
  window_registry::list_monitors().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_permission_status() -> CommandResult<PermissionStatus> {
  permissions::current_permission_status().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn create_session(
  config: SessionConfig,
  app: AppHandle,
  state: State<'_, AppState>,
) -> CommandResult<SessionInfo> {
  sync_session::create_session(&app, &state, config).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn update_session(
  session_id: String,
  config: SessionConfig,
  app: AppHandle,
  state: State<'_, AppState>,
) -> CommandResult<SessionInfo> {
  sync_session::update_session(&app, &state, &session_id, config).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>) -> Vec<SessionInfo> {
  state.list_sessions()
}

#[tauri::command]
pub fn apply_layout(
  session_id: String,
  app: AppHandle,
  state: State<'_, AppState>,
) -> CommandResult<SessionInfo> {
  sync_session::apply_layout(&app, &state, &session_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn start_sync(
  session_id: String,
  app: AppHandle,
  state: State<'_, AppState>,
) -> CommandResult<SessionInfo> {
  sync_session::start_sync(&app, &state, &session_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn stop_sync(
  session_id: String,
  app: AppHandle,
  state: State<'_, AppState>,
) -> CommandResult<SessionInfo> {
  sync_session::stop_sync(&app, &state, &session_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_profile(
  profile: ProfileDraft,
  app: AppHandle,
  state: State<'_, AppState>,
) -> CommandResult<Vec<ProfileRecord>> {
  let profiles = profile_store::save_profile(&app, profile.clone()).map_err(|error| error.to_string())?;
  push_log(
    &app,
    &state,
    crate::models::LogLevel::Info,
    format!("Saved profile '{}'.", profile.name.trim()),
  );
  Ok(profiles)
}

#[tauri::command]
pub fn load_profiles(app: AppHandle) -> CommandResult<Vec<ProfileRecord>> {
  profile_store::load_profiles(&app).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_logs(state: State<'_, AppState>) -> Vec<LogEntry> {
  list_all_logs(&state)
}
