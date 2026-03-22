use anyhow::Result;

use crate::models::{MonitorInfo, WindowInfo};
use crate::platform;

pub fn scan_windows() -> Result<Vec<WindowInfo>> {
  platform::scan_windows()
}

pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
  platform::list_monitors()
}

#[allow(dead_code)]
pub fn window_exists(window_id: &str) -> bool {
  platform::window_exists(window_id)
}
