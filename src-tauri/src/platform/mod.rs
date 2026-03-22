#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{
  bring_window_to_front, initialize, list_monitors, move_resize_window, permission_status,
  scan_windows, start_input_sync, stop_input_sync, window_exists,
};

#[cfg(not(target_os = "windows"))]
mod unsupported {
  use anyhow::{anyhow, Result};

  use crate::models::{Bounds, MonitorInfo, PermissionStatus, SessionConfig, WindowInfo};

  pub fn initialize() -> Result<()> {
    Ok(())
  }

  pub fn scan_windows() -> Result<Vec<WindowInfo>> {
    Err(anyhow!("This preview build only supports Windows."))
  }

  pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
    Err(anyhow!("This preview build only supports Windows."))
  }

  pub fn move_resize_window(_window_id: &str, _bounds: &Bounds) -> Result<()> {
    Err(anyhow!("This preview build only supports Windows."))
  }

  pub fn bring_window_to_front(_window_id: &str) -> Result<()> {
    Err(anyhow!("This preview build only supports Windows."))
  }

  pub fn window_exists(_window_id: &str) -> bool {
    false
  }

  pub fn start_input_sync(_session_id: &str, _config: SessionConfig) -> Result<()> {
    Err(anyhow!("Live input sync is only implemented on Windows."))
  }

  pub fn stop_input_sync(_session_id: Option<&str>) -> Result<bool> {
    Ok(false)
  }

  pub fn permission_status() -> Result<PermissionStatus> {
    Ok(PermissionStatus {
      platform: std::env::consts::OS.to_string(),
      window_management_supported: false,
      input_capture_supported: false,
      input_dispatch_supported: false,
      is_process_elevated: false,
      warnings: vec!["Mirror Windows MVP is currently Windows-only.".to_string()],
    })
  }
}

#[cfg(not(target_os = "windows"))]
pub use unsupported::{
  bring_window_to_front, initialize, list_monitors, move_resize_window, permission_status,
  scan_windows, start_input_sync, stop_input_sync, window_exists,
};
