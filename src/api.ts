import { invoke } from "@tauri-apps/api/core";
import type {
  LogEntry,
  MonitorInfo,
  PermissionStatus,
  ProfileDraft,
  ProfileRecord,
  SessionConfig,
  SessionInfo,
  WindowInfo,
} from "./types";

export const scanWindows = () => invoke<WindowInfo[]>("scan_windows");
export const listMonitors = () => invoke<MonitorInfo[]>("list_monitors");
export const getPermissionStatus = () =>
  invoke<PermissionStatus>("get_permission_status");
export const createSession = (config: SessionConfig) =>
  invoke<SessionInfo>("create_session", { config });
export const updateSession = (sessionId: string, config: SessionConfig) =>
  invoke<SessionInfo>("update_session", { sessionId, config });
export const listSessions = () => invoke<SessionInfo[]>("list_sessions");
export const applyLayout = (sessionId: string) =>
  invoke<SessionInfo>("apply_layout", { sessionId });
export const startSync = (sessionId: string) =>
  invoke<SessionInfo>("start_sync", { sessionId });
export const stopSync = (sessionId: string) =>
  invoke<SessionInfo>("stop_sync", { sessionId });
export const saveProfile = (profile: ProfileDraft) =>
  invoke<ProfileRecord[]>("save_profile", { profile });
export const loadProfiles = () => invoke<ProfileRecord[]>("load_profiles");
export const getLogs = () => invoke<LogEntry[]>("get_logs");
