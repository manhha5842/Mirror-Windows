use std::collections::HashSet;

use anyhow::{anyhow, Result};
use tauri::AppHandle;

use crate::layout_engine;
use crate::models::{LogLevel, SessionConfig, SessionInfo};
use crate::platform;
use crate::state::{push_log, unix_time_ms, AppState};
use crate::window_registry;

pub fn create_session(app: &AppHandle, state: &AppState, config: SessionConfig) -> Result<SessionInfo> {
  validate_config(&config)?;

  let session = SessionInfo {
    id: state.allocate_session_id(),
    config,
    is_running: false,
    layout_applied: false,
    created_at_ms: unix_time_ms(),
    updated_at_ms: unix_time_ms(),
  };
  state.upsert_session(session.clone());
  push_log(
    app,
    state,
    LogLevel::Info,
    format!("Created session {}.", session.id),
  );
  Ok(session)
}

pub fn update_session(
  app: &AppHandle,
  state: &AppState,
  session_id: &str,
  config: SessionConfig,
) -> Result<SessionInfo> {
  validate_config(&config)?;
  let updated = state
    .mutate_session(session_id, |session| {
      session.config = config;
      session.updated_at_ms = unix_time_ms();
    })
    .ok_or_else(|| anyhow!("Session '{session_id}' was not found."))?;

  push_log(
    app,
    state,
    LogLevel::Info,
    format!("Updated session {}.", updated.id),
  );
  Ok(updated)
}

pub fn apply_layout(app: &AppHandle, state: &AppState, session_id: &str) -> Result<SessionInfo> {
  let session = state
    .read_session(session_id)
    .ok_or_else(|| anyhow!("Session '{session_id}' was not found."))?;

  let preview = layout_engine::apply_layout(&session.config)?;
  let updated = state
    .mutate_session(session_id, |existing| {
      existing.layout_applied = true;
      existing.updated_at_ms = unix_time_ms();
    })
    .ok_or_else(|| anyhow!("Session '{session_id}' was not found after layout."))?;

  push_log(
    app,
    state,
    LogLevel::Info,
    format!(
      "Applied {:?} layout for {} managed windows on session {}.",
      updated.config.layout_mode,
      preview.len(),
      updated.id
    ),
  );
  Ok(updated)
}

pub fn start_sync(app: &AppHandle, state: &AppState, session_id: &str) -> Result<SessionInfo> {
  let session = state
    .read_session(session_id)
    .ok_or_else(|| anyhow!("Session '{session_id}' was not found."))?;
  if session.config.target_window_ids.is_empty() {
    return Err(anyhow!(
      "Select at least one target window before starting sync."
    ));
  }

  for running_session in state
    .list_sessions()
    .into_iter()
    .filter(|item| item.is_running && item.id != session_id)
  {
    let _ = platform::stop_input_sync(Some(&running_session.id));
    let _ = state.mutate_session(&running_session.id, |existing| {
      existing.is_running = false;
      existing.updated_at_ms = unix_time_ms();
    });
  }

  let preview = layout_engine::apply_layout(&session.config)?;
  platform::start_input_sync(session_id, session.config.clone())?;

  for item in &preview {
    let _ = platform::bring_window_to_front(&item.window_id);
  }

  let updated = state
    .mutate_session(session_id, |existing| {
      existing.layout_applied = true;
      existing.is_running = true;
      existing.updated_at_ms = unix_time_ms();
    })
    .ok_or_else(|| anyhow!("Session '{session_id}' was not found when starting sync."))?;

  push_log(
    app,
    state,
    LogLevel::Info,
    format!(
      "Applied {:?} layout for {} managed windows on session {} before starting sync.",
      updated.config.layout_mode,
      preview.len(),
      updated.id
    ),
  );
  push_log(
    app,
    state,
    LogLevel::Info,
    format!(
      "Started live sync for session {}. Click mirroring now runs in simplified Windows mode.",
      updated.id
    ),
  );
  Ok(updated)
}

pub fn stop_sync(app: &AppHandle, state: &AppState, session_id: &str) -> Result<SessionInfo> {
  let _ = platform::stop_input_sync(Some(session_id))?;

  let updated = state
    .mutate_session(session_id, |existing| {
      existing.is_running = false;
      existing.updated_at_ms = unix_time_ms();
    })
    .ok_or_else(|| anyhow!("Session '{session_id}' was not found."))?;

  push_log(
    app,
    state,
    LogLevel::Info,
    format!("Stopped session {}.", updated.id),
  );
  Ok(updated)
}

fn validate_config(config: &SessionConfig) -> Result<()> {
  if config.primary_window_id.trim().is_empty() {
    return Err(anyhow!("A primary window is required."));
  }

  let known_windows = window_registry::scan_windows()?;
  let known_window_ids = known_windows
    .iter()
    .map(|window| window.id.clone())
    .collect::<HashSet<_>>();

  if !known_window_ids.contains(&config.primary_window_id) {
    return Err(anyhow!(
      "The selected primary window is no longer available."
    ));
  }

  let mut managed = HashSet::new();
  managed.insert(config.primary_window_id.clone());

  for target_id in &config.target_window_ids {
    if target_id == &config.primary_window_id {
      return Err(anyhow!("Primary window cannot also be a target."));
    }
    if !known_window_ids.contains(target_id) {
      return Err(anyhow!("One or more target windows are no longer available."));
    }
    managed.insert(target_id.clone());
  }

  if managed.len() > 20 {
    return Err(anyhow!(
      "Mirror Windows currently supports at most 20 managed windows per session."
    ));
  }

  if let Some(monitor_id) = &config.monitor_id {
    let monitors = window_registry::list_monitors()?;
    if !monitors.iter().any(|monitor| monitor.id == *monitor_id) {
      return Err(anyhow!("The selected monitor is no longer available."));
    }
  }

  Ok(())
}
