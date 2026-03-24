import { useDeferredValue, useEffect, useMemo, useState, startTransition } from "react";
import {
  applyLayout,
  createSession,
  getLogs,
  getPermissionStatus,
  listMonitors,
  listSessions,
  scanWindows,
  startSync,
  stopSync,
  updateSession,
} from "./api";
import type {
  LayoutMode,
  LogEntry,
  MonitorInfo,
  PermissionStatus,
  SessionConfig,
  SessionInfo,
  WindowInfo,
} from "./types";

const MAX_WINDOWS = 20;

export default function App() {
  const [windows, setWindows] = useState<WindowInfo[]>([]);
  const [monitors, setMonitors] = useState<MonitorInfo[]>([]);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [permissionStatus, setPermissionStatus] = useState<PermissionStatus | null>(null);
  const [selectedWindowIds, setSelectedWindowIds] = useState<string[]>([]);
  const [primaryWindowId, setPrimaryWindowId] = useState("");
  const [layoutMode, setLayoutMode] = useState<LayoutMode>("tile");
  const [gameMode, setGameMode] = useState(false);
  const [selectedMonitorId, setSelectedMonitorId] = useState<string | null>(null);
  const [currentSessionId, setCurrentSessionId] = useState<string | null>(null);
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const deferredLogs = useDeferredValue(logs);
  const monitorLookup = useMemo(
    () => new Map(monitors.map((monitor) => [monitor.id, monitor] as const)),
    [monitors],
  );
  const selectedWindows = useMemo(
    () => windows.filter((window) => selectedWindowIds.includes(window.id)),
    [selectedWindowIds, windows],
  );
  const windowGroups = useMemo(() => {
    const groups = new Map<string, { label: string; windows: WindowInfo[] }>();

    for (const window of windows) {
      const appLabel = inferWindowGroupLabel(window);
      const key = appLabel.toLowerCase();
      const existing = groups.get(key);

      if (existing) {
        existing.windows.push(window);
        continue;
      }

      groups.set(key, {
        label: appLabel,
        windows: [window],
      });
    }

    return Array.from(groups.values()).sort((left, right) => left.label.localeCompare(right.label, "vi"));
  }, [windows]);
  const selectedPrimaryWindow = useMemo(
    () => windows.find((window) => window.id === primaryWindowId) ?? null,
    [primaryWindowId, windows],
  );
  const primaryMonitor = useMemo(
    () => monitors.find((monitor) => monitor.is_primary) ?? null,
    [monitors],
  );
  const activeLayoutMonitor = useMemo(() => {
    if (selectedMonitorId) {
      return monitorLookup.get(selectedMonitorId) ?? null;
    }

    return primaryMonitor;
  }, [monitorLookup, primaryMonitor, selectedMonitorId]);
  const activeSession = useMemo(
    () => sessions.find((session) => session.id === currentSessionId) ?? null,
    [currentSessionId, sessions],
  );

  useEffect(() => {
    void refreshAll();
  }, []);

  async function refreshAll() {
    setBusyAction("Đang quét hệ thống");
    setErrorMessage(null);

    try {
      const [windowsResult, monitorsResult, sessionsResult, logsResult, permissionsResult] =
        await Promise.all([
          scanWindows(),
          listMonitors(),
          listSessions(),
          getLogs(),
          getPermissionStatus(),
        ]);

      startTransition(() => {
        setWindows(windowsResult);
        setMonitors(monitorsResult);
        setSessions(sessionsResult);
        setLogs(logsResult);
        setPermissionStatus(permissionsResult);
      });

      if (!selectedMonitorId && monitorsResult.length > 0) {
        setSelectedMonitorId(monitorsResult.find((monitor) => monitor.is_primary)?.id ?? monitorsResult[0].id);
      }

      if (!currentSessionId && sessionsResult.length > 0) {
        setCurrentSessionId(sessionsResult[0].id);
      }
    } catch (error) {
      setErrorMessage(readError(error));
    } finally {
      setBusyAction(null);
    }
  }

  function buildConfig(): SessionConfig {
    return {
      primary_window_id: primaryWindowId,
      target_window_ids: selectedWindowIds.filter((windowId) => windowId !== primaryWindowId),
      monitor_id: selectedMonitorId,
      layout_mode: layoutMode,
      coordinate_mode: "normalized_client",
      dispatch_mode: gameMode ? "send_input" : "auto",
      sync_mouse_move: true,
      sync_wheel: true,
      sync_keyboard: false,
      game_mode: gameMode,
    };
  }

  async function ensureSession(): Promise<SessionInfo> {
    const config = buildConfig();
    if (!config.primary_window_id) {
      throw new Error("Hãy chọn một cửa sổ chính trước.");
    }

    const session = currentSessionId
      ? await updateSession(currentSessionId, config)
      : await createSession(config);

    const sessionsResult = await listSessions();
    const logsResult = await getLogs();

    startTransition(() => {
      setSessions(sessionsResult);
      setLogs(logsResult);
      setCurrentSessionId(session.id);
    });

    return session;
  }

  async function handleApplyLayout() {
    await runAction("Đang áp layout", async () => {
      const session = await ensureSession();
      await applyLayout(session.id);
      const [sessionsResult, logsResult, windowsResult] = await Promise.all([
        listSessions(),
        getLogs(),
        scanWindows(),
      ]);
      startTransition(() => {
        setSessions(sessionsResult);
        setLogs(logsResult);
        setWindows(windowsResult);
      });
    });
  }

  async function handleStartSync() {
    await runAction("Đang bật đồng bộ", async () => {
      const session = await ensureSession();
      await startSync(session.id);
      const [sessionsResult, logsResult] = await Promise.all([listSessions(), getLogs()]);
      startTransition(() => {
        setSessions(sessionsResult);
        setLogs(logsResult);
      });
    });
  }

  async function handleStopSync() {
    if (!currentSessionId) {
      return;
    }

    await runAction("Đang dừng đồng bộ", async () => {
      await stopSync(currentSessionId);
      const [sessionsResult, logsResult] = await Promise.all([listSessions(), getLogs()]);
      startTransition(() => {
        setSessions(sessionsResult);
        setLogs(logsResult);
      });
    });
  }

  function toggleWindowSelection(windowId: string) {
    setErrorMessage(null);
    setSelectedWindowIds((current) => {
      const exists = current.includes(windowId);
      if (exists) {
        const next = current.filter((id) => id !== windowId);
        if (primaryWindowId === windowId) {
          setPrimaryWindowId(next[0] ?? "");
        }
        return next;
      }

      if (current.length >= MAX_WINDOWS) {
        setErrorMessage(`Hiện tại chỉ hỗ trợ tối đa ${MAX_WINDOWS} cửa sổ.`);
        return current;
      }

      const next = [...current, windowId];
      if (!primaryWindowId) {
        setPrimaryWindowId(windowId);
      }
      return next;
    });
  }

  function choosePrimary(windowId: string) {
    if (!selectedWindowIds.includes(windowId)) {
      toggleWindowSelection(windowId);
    }
    setPrimaryWindowId(windowId);
  }

  function toggleGroupSelection(groupWindows: WindowInfo[]) {
    setErrorMessage(null);
    setSelectedWindowIds((current) => {
      const groupIds = groupWindows.map((window) => window.id);
      const allSelected = groupIds.every((windowId) => current.includes(windowId));

      if (allSelected) {
        const next = current.filter((windowId) => !groupIds.includes(windowId));
        if (primaryWindowId && groupIds.includes(primaryWindowId)) {
          setPrimaryWindowId(next[0] ?? "");
        }
        return next;
      }

      const availableSlots = MAX_WINDOWS - current.length;
      const missingIds = groupIds.filter((windowId) => !current.includes(windowId));

      if (missingIds.length > availableSlots) {
        setErrorMessage(`Hiện tại chỉ hỗ trợ tối đa ${MAX_WINDOWS} cửa sổ.`);
        return current;
      }

      const next = [...current, ...missingIds];
      if (!primaryWindowId && next.length > 0) {
        setPrimaryWindowId(next[0]);
      }
      return next;
    });
  }

  async function runAction(label: string, action: () => Promise<void>) {
    setBusyAction(label);
    setErrorMessage(null);
    try {
      await action();
    } catch (error) {
      setErrorMessage(readError(error));
    } finally {
      setBusyAction(null);
    }
  }

  return (
    <main className="shell">
      <section className="hero">
        <div>
          <p className="eyebrow">Windows-first MVP</p>
          <h1>Mirror Windows</h1>
          <p className="hero-copy">
            Quét cửa sổ desktop đang mở, chọn một cửa sổ chính, sắp xếp theo tile hoặc stack, rồi đồng
            bộ thao tác click chuột từ cửa sổ chính sang các cửa sổ đích trên Windows.
          </p>
        </div>

        <div className="hero-actions">
          <div className={`status-pill ${activeSession?.is_running ? "running" : "idle"}`}>
            {activeSession?.is_running ? "Đang đồng bộ" : "Đang chờ"}
          </div>
        </div>
      </section>

      {errorMessage ? <section className="error-panel">{errorMessage}</section> : null}

      <section className="grid">
        <div className="panel panel-large">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Window Registry</p>
              <h2>Chọn tối đa {MAX_WINDOWS} cửa sổ cần đồng bộ</h2>
              <p className="panel-note">
                Ưu tiên chọn cửa sổ đang mở đủ lớn. Cửa sổ chính sẽ là nguồn nhận thao tác gốc.
              </p>
            </div>
            <div className="panel-header-actions">
              <span className="subtle-text">
                Đã chọn {selectedWindowIds.length}/{MAX_WINDOWS} · Phát hiện {windows.length} cửa sổ
              </span>
              <button className="ghost-button" onClick={() => void refreshAll()} disabled={busyAction !== null}>
                Quét lại cửa sổ
              </button>
            </div>
          </div>

          <div className="window-group-list">
            {windowGroups.map((group) => (
              <section key={group.label} className="window-group">
                <div className="window-group-header">
                  <div>
                    <p className="window-group-title">{group.label}</p>
                    <p className="window-group-count">{group.windows.length} cửa sổ</p>
                  </div>
                  <button
                    className="group-action"
                    onClick={() => toggleGroupSelection(group.windows)}
                    disabled={
                      !group.windows.every((window) => selectedWindowIds.includes(window.id)) &&
                      selectedWindowIds.length + group.windows.filter((window) => !selectedWindowIds.includes(window.id)).length >
                        MAX_WINDOWS
                    }
                  >
                    {group.windows.every((window) => selectedWindowIds.includes(window.id))
                      ? "Bỏ chọn"
                      : "Chọn tất cả"}
                  </button>
                </div>

                <div className="window-list">
                  {group.windows.map((window) => {
                    const selected = selectedWindowIds.includes(window.id);
                    const primary = primaryWindowId === window.id;
                    const title = window.title.trim() || "Không có tiêu đề";

                    return (
                      <label
                        key={window.id}
                        className={`window-card compact ${selected ? "selected" : ""} ${primary ? "primary" : ""}`}
                      >
                        <div className="window-card-top">
                          <div className="window-select">
                            <input
                              type="checkbox"
                              checked={selected}
                              onChange={() => toggleWindowSelection(window.id)}
                            />
                            {window.icon_data_url ? (
                              <div className="window-icon" aria-hidden="true">
                                <img src={window.icon_data_url} alt="" />
                              </div>
                            ) : null}
                            <div className="window-copy">
                              <div className="window-title-row">
                                <strong title={title}>{title}</strong>
                                {primary ? <span className="primary-badge">Chính</span> : null}
                                {!window.is_visible ? <span className="state-chip">Ẩn</span> : null}
                                {window.is_minimized ? <span className="state-chip">Thu nhỏ</span> : null}
                              </div>
                            </div>
                          </div>

                          <button
                            className="tiny-button"
                            disabled={!selected || primary}
                            onClick={(event) => {
                              event.preventDefault();
                              choosePrimary(window.id);
                            }}
                          >
                            {primary ? "Đang chính" : "Chọn chính"}
                          </button>
                        </div>
                      </label>
                    );
                  })}
                </div>
              </section>
            ))}
          </div>
        </div>

        <div className="panel">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Phiên đồng bộ</p>
              <h2>Cửa sổ chính và bố cục</h2>
            </div>
          </div>

          <div className="stack">
            <div className="field">
              <label>Cửa sổ chính</label>
              <div className="value-box">
                {selectedPrimaryWindow ? selectedPrimaryWindow.title : "Hãy chọn một cửa sổ để đặt làm chính"}
              </div>
            </div>

            <div className="field">
              <label>Cửa sổ đích</label>
              <div className="value-box">
                {selectedWindows.filter((window) => window.id !== primaryWindowId).length} cửa sổ đã chọn
              </div>
            </div>

            <div className="field">
              <label>Màn hình sắp xếp</label>
              <select
                value={selectedMonitorId ?? ""}
                onChange={(event) => setSelectedMonitorId(event.target.value || null)}
              >
                <option value="">Dùng màn hình của cửa sổ chính</option>
                {monitors.map((monitor) => (
                  <option key={monitor.id} value={monitor.id}>
                    {monitor.name} {monitor.is_primary ? "(Chính)" : ""} · {monitor.work_area.width}×
                    {monitor.work_area.height}
                  </option>
                ))}
              </select>
              <div className="value-box">
                <div className="window-title-row">
                  <strong>{primaryMonitor ? primaryMonitor.name : "Chua nhan dien duoc man hinh chinh"}</strong>
                  {primaryMonitor ? <span className="primary-badge">Primary</span> : null}
                </div>
                <p className="field-hint">
                  {activeLayoutMonitor
                    ? `Dang sap xep tren: ${activeLayoutMonitor.name} (${activeLayoutMonitor.work_area.width}x${activeLayoutMonitor.work_area.height})`
                    : "Chua co man hinh sap xep duoc chon."}
                </p>
              </div>
            </div>

            <div className="field">
              <label>Chế độ bố cục</label>
              <div className="segmented">
                <button
                  className={`layout-button ${layoutMode === "tile" ? "active" : ""}`}
                  onClick={() => setLayoutMode("tile")}
                >
                  Chia ô
                </button>
                <button
                  className={`layout-button ${layoutMode === "stack" ? "active" : ""}`}
                  onClick={() => setLayoutMode("stack")}
                >
                  Chồng lớp
                </button>
              </div>
            </div>

            <div className="field">
              <label>Game Mode</label>
              <label className="toggle">
                <input
                  type="checkbox"
                  checked={gameMode}
                  onChange={(event) => setGameMode(event.target.checked)}
                />
                <span>Game Mode</span>
              </label>
              <p className="field-hint">
                Bat de uu tien SendInput va tang timing focus/click cho game.
              </p>
            </div>

            <div className="action-row">
              <button onClick={() => void handleApplyLayout()} disabled={busyAction !== null}>
                Áp layout
              </button>
              <button
                onClick={() =>
                  void (activeSession?.is_running ? handleStopSync() : handleStartSync())
                }
                className={`sync-toggle-button ${activeSession?.is_running ? "stop" : "start"}`}
                disabled={busyAction !== null || (activeSession?.is_running ? !currentSessionId : false)}
              >
                {activeSession?.is_running ? "Dừng" : "Bắt đầu"}
              </button>
            </div>
          </div>
        </div>

        <div className="panel">
          <div className="panel-header">
            <div>
              <p className="eyebrow">Trạng thái</p>
              <h2>Phiên chạy và nhật ký</h2>
            </div>
          </div>

          <div className="stack">
            <div className="session-list">
              {sessions.length === 0 ? (
                <p className="subtle-text">Chưa có phiên đồng bộ nào được tạo.</p>
              ) : (
                sessions.map((session) => (
                  <button
                    key={session.id}
                    className={`session-card ${session.id === currentSessionId ? "selected" : ""}`}
                    onClick={() => setCurrentSessionId(session.id)}
                  >
                    <strong>{session.id}</strong>
                    <span>{describeSession(session)}</span>
                  </button>
                ))
              )}
            </div>

            <div className="log-list">
              {deferredLogs.length === 0 ? (
                <p className="subtle-text">Nhật ký sẽ hiện ở đây sau khi bạn quét, áp layout hoặc bật đồng bộ.</p>
              ) : (
                deferredLogs
                  .slice()
                  .reverse()
                  .map((entry) => (
                    <article key={entry.id} className={`log-entry ${entry.level}`}>
                      <div className="log-head">
                        <span>{translateLogLevel(entry.level)}</span>
                        <time>{formatTime(entry.timestamp_ms)}</time>
                      </div>
                      <p>{entry.message}</p>
                    </article>
                  ))
              )}
            </div>
          </div>
        </div>
      </section>

      {permissionStatus ? (
        <section className="system-note">
          <p className="system-note-title">
            {permissionStatus.window_management_supported
              ? "Sẵn sàng quản lý cửa sổ."
              : "Thiết bị hiện chưa hỗ trợ quản lý cửa sổ."}
          </p>
          <div className="warning-list compact">
            {permissionStatus.warnings.map((warning) => (
              <span key={warning} className="warning-chip compact">
                {warning}
              </span>
            ))}
          </div>
        </section>
      ) : null}

      {busyAction ? <div className="busy-indicator">{busyAction}</div> : null}
    </main>
  );
}

function translateLogLevel(level: string) {
  switch (level) {
    case "info":
      return "THÔNG TIN";
    case "warn":
      return "CẢNH BÁO";
    case "error":
      return "LỖI";
    default:
      return level.toUpperCase();
  }
}

function describeSession(session: SessionInfo) {
  const layout = session.config.layout_mode === "tile" ? "Chia ô" : "Chồng lớp";
  const profile = session.config.game_mode ? "game mode" : "chuẩn";
  const state = session.is_running ? "đang chạy" : "đang chờ";
  return `${layout} · ${profile} · ${state}`;
}

function formatTime(timestampMs: number) {
  return new Intl.DateTimeFormat("vi-VN", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(timestampMs);
}

function formatAppName(processName: string) {
  const normalized = processName.trim();
  if (!normalized || normalized === "other") {
    return "Ứng dụng khác";
  }

  const withoutExtension = normalized.replace(/\.exe$/i, "");
  return withoutExtension.charAt(0).toUpperCase() + withoutExtension.slice(1);
}

function inferWindowGroupLabel(window: WindowInfo) {
  const processLabel = formatAppName(window.process_name?.trim() || "other");
  if (processLabel !== "Ứng dụng khác") {
    return processLabel;
  }

  const title = window.title.trim();
  if (!title) {
    return "Ứng dụng khác";
  }

  const titleParts = title
    .split(" - ")
    .map((part) => part.trim())
    .filter(Boolean);

  for (let index = titleParts.length - 1; index >= 0; index -= 1) {
    const candidate = titleParts[index];
    if (looksLikeAppLabel(candidate)) {
      return candidate;
    }
  }

  return titleParts.length > 1 ? titleParts[titleParts.length - 1] : "Ứng dụng khác";
}

function looksLikeAppLabel(value: string) {
  const normalized = value.trim();
  if (!normalized) {
    return false;
  }

  const lowered = normalized.toLowerCase();
  if (
    lowered === "file explorer" ||
    lowered === "microsoft edge" ||
    lowered === "google chrome" ||
    lowered === "visual studio code" ||
    lowered === "ultraviewer" ||
    lowered === "zalo"
  ) {
    return true;
  }

  return normalized.split(/\s+/).length <= 4 && !/[\\/]/.test(normalized);
}

function readError(error: unknown) {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  return "Đã xảy ra lỗi không xác định";
}



