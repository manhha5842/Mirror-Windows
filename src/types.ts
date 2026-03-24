export type Bounds = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type WindowInfo = {
  id: string;
  native_handle: number;
  title: string;
  process_name: string | null;
  icon_data_url: string | null;
  pid: number;
  monitor_id: string | null;
  rect: Bounds;
  is_visible: boolean;
  is_minimized: boolean;
};

export type MonitorInfo = {
  id: string;
  name: string;
  rect: Bounds;
  work_area: Bounds;
  is_primary: boolean;
  scale_factor: number;
};

export type LayoutMode = "tile" | "stack";
export type CoordinateMode = "normalized_client";
export type DispatchMode = "window_message" | "send_input" | "auto";

export type SessionConfig = {
  primary_window_id: string;
  target_window_ids: string[];
  monitor_id: string | null;
  layout_mode: LayoutMode;
  coordinate_mode: CoordinateMode;
  dispatch_mode: DispatchMode;
  sync_mouse_move: boolean;
  sync_wheel: boolean;
  sync_keyboard: boolean;
  game_mode: boolean;
};

export type SessionInfo = {
  id: string;
  config: SessionConfig;
  is_running: boolean;
  layout_applied: boolean;
  created_at_ms: number;
  updated_at_ms: number;
};

export type ProfileDraft = {
  id: string | null;
  name: string;
  config: SessionConfig;
};

export type ProfileRecord = {
  id: string;
  name: string;
  config: SessionConfig;
  created_at_ms: number;
  updated_at_ms: number;
};

export type LogLevel = "info" | "warn" | "error";

export type LogEntry = {
  id: number;
  level: LogLevel;
  message: string;
  timestamp_ms: number;
};

export type PermissionStatus = {
  platform: string;
  window_management_supported: boolean;
  input_capture_supported: boolean;
  input_dispatch_supported: boolean;
  is_process_elevated: boolean;
  warnings: string[];
};
