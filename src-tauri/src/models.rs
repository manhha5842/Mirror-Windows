use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bounds {
  pub x: i32,
  pub y: i32,
  pub width: i32,
  pub height: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WindowInfo {
  pub id: String,
  pub native_handle: i64,
  pub title: String,
  pub process_name: Option<String>,
  pub icon_data_url: Option<String>,
  pub pid: u32,
  pub monitor_id: Option<String>,
  pub rect: Bounds,
  pub is_visible: bool,
  pub is_minimized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonitorInfo {
  pub id: String,
  pub name: String,
  pub rect: Bounds,
  pub work_area: Bounds,
  pub is_primary: bool,
  pub scale_factor: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
  Tile,
  Stack,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateMode {
  NormalizedClient,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
  /// Sử dụng PostMessage/SendMessage (tốt cho browser, ứng dụng thông thường)
  WindowMessage,
  /// Sử dụng SendInput API (tốt cho game, DirectX/OpenGL apps)
  SendInput,
  /// Tự động phát hiện và chọn mode phù hợp
  Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionConfig {
  pub primary_window_id: String,
  pub target_window_ids: Vec<String>,
  pub monitor_id: Option<String>,
  pub layout_mode: LayoutMode,
  #[serde(default = "default_coordinate_mode")]
  pub coordinate_mode: CoordinateMode,
  #[serde(default = "default_dispatch_mode")]
  pub dispatch_mode: DispatchMode,
  #[serde(default = "default_true")]
  pub sync_mouse_move: bool,
  #[serde(default = "default_true")]
  pub sync_wheel: bool,
  #[serde(default = "default_true")]
  pub sync_keyboard: bool,
  #[serde(default)]
  pub game_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionInfo {
  pub id: String,
  pub config: SessionConfig,
  pub is_running: bool,
  pub layout_applied: bool,
  pub created_at_ms: u128,
  pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileDraft {
  pub id: Option<String>,
  pub name: String,
  pub config: SessionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileRecord {
  pub id: String,
  pub name: String,
  pub config: SessionConfig,
  pub created_at_ms: u128,
  pub updated_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
  Info,
  Warn,
  Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogEntry {
  pub id: u64,
  pub level: LogLevel,
  pub message: String,
  pub timestamp_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionStatus {
  pub platform: String,
  pub window_management_supported: bool,
  pub input_capture_supported: bool,
  pub input_dispatch_supported: bool,
  pub is_process_elevated: bool,
  pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutPreview {
  pub window_id: String,
  pub bounds: Bounds,
}

fn default_true() -> bool {
  true
}

fn default_coordinate_mode() -> CoordinateMode {
  CoordinateMode::NormalizedClient
}

fn default_dispatch_mode() -> DispatchMode {
  DispatchMode::Auto
}
