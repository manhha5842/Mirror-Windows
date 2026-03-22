use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::models::{LogEntry, LogLevel, SessionInfo};

pub struct AppState {
  sessions: Mutex<HashMap<String, SessionInfo>>,
  logs: Mutex<Vec<LogEntry>>,
  next_session_id: AtomicU64,
  next_log_id: AtomicU64,
}

impl Default for AppState {
  fn default() -> Self {
    Self {
      sessions: Mutex::new(HashMap::new()),
      logs: Mutex::new(Vec::new()),
      next_session_id: AtomicU64::new(1),
      next_log_id: AtomicU64::new(1),
    }
  }
}

impl AppState {
  pub fn allocate_session_id(&self) -> String {
    format!(
      "session-{}",
      self.next_session_id.fetch_add(1, Ordering::Relaxed)
    )
  }

  pub fn upsert_session(&self, session: SessionInfo) {
    self
      .sessions
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .insert(session.id.clone(), session);
  }

  pub fn read_session(&self, session_id: &str) -> Option<SessionInfo> {
    self
      .sessions
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .get(session_id)
      .cloned()
  }

  pub fn mutate_session<F>(&self, session_id: &str, mutate: F) -> Option<SessionInfo>
  where
    F: FnOnce(&mut SessionInfo),
  {
    let mut sessions = self
      .sessions
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    let session = sessions.get_mut(session_id)?;
    mutate(session);
    Some(session.clone())
  }

  pub fn list_sessions(&self) -> Vec<SessionInfo> {
    let mut sessions = self
      .sessions
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .values()
      .cloned()
      .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    sessions
  }

  pub fn push_log_entry(&self, level: LogLevel, message: impl Into<String>) -> LogEntry {
    let entry = LogEntry {
      id: self.next_log_id.fetch_add(1, Ordering::Relaxed),
      level,
      message: message.into(),
      timestamp_ms: unix_time_ms(),
    };

    let mut logs = self
      .logs
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    logs.push(entry.clone());
    if logs.len() > 300 {
      let remove_count = logs.len() - 300;
      logs.drain(0..remove_count);
    }

    entry
  }

  pub fn list_logs(&self) -> Vec<LogEntry> {
    self
      .logs
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .clone()
  }
}

pub fn unix_time_ms() -> u128 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis()
}

pub fn push_log(
  _app: &AppHandle,
  state: &AppState,
  level: LogLevel,
  message: impl Into<String>,
) -> LogEntry {
  let message = message.into();
  match level {
    LogLevel::Info => log::info!("{message}"),
    LogLevel::Warn => log::warn!("{message}"),
    LogLevel::Error => log::error!("{message}"),
  }
  state.push_log_entry(level, message)
}
