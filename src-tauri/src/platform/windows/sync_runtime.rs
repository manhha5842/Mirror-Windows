use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, MapWindowPoints, ScreenToClient};
use windows::Win32::System::SystemServices::{
  MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_F12, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
  CallNextHookEx, ChildWindowFromPointEx, DispatchMessageW, GetAncestor, GetClientRect,
  GetCursorPos, GetMessageW, HHOOK, IsWindow, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED,
  MSLLHOOKSTRUCT, MSG, PostMessageW,
  PostThreadMessageW, SendMessageTimeoutW, SetCursorPos, SetForegroundWindow,
  SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WindowFromPoint,
  CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, GA_ROOT, HC_ACTION,
  SMTO_ABORTIFHUNG, SMTO_BLOCK, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN,
  WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL,
  WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
};

use crate::coordinate_mapper::{normalize_client_point, project_to_client, NormalizedPoint};
use crate::game_input_dispatcher::GameInputDispatcher;
use crate::models::{Bounds, DispatchMode, LayoutMode, LogLevel, SessionConfig};
use crate::state::{push_runtime_log, unix_time_ms};

use super::{bring_window_to_front, ensure_bool, parse_hwnd};

const PRE_CLICK_MOVE_SETTLE_MS: u64 = 8;
const INTER_TARGET_CLICK_SETTLE_MS: u64 = 4;
const CLICK_SLOP_PX: i32 = 6;
const FOREGROUND_TARGET_SETTLE_MS: u64 = 70;
const GAME_MODE_FOREGROUND_TARGET_SETTLE_MS: u64 = 130;
const PRIMARY_CLICK_SETTLE_MS: u64 = 45;
const SUMMARY_LOG_COOLDOWN_MS: u128 = 3_000;
const ISSUE_LOG_COOLDOWN_MS: u128 = 5_000;
const TARGET_REMOVAL_LOG_COOLDOWN_MS: u128 = 15_000;
const CLICK_TRACE_LOG_COOLDOWN_MS: u128 = 10_000;
const PRIMARY_INVALID_LOG_COOLDOWN_MS: u128 = 10_000;
const DEFAULT_GESTURE_POINT_DELTA: f64 = 0.001;
const GAME_MODE_GESTURE_POINT_DELTA: f64 = 0.01;
const MAX_GAME_MODE_GESTURE_POINTS: usize = 48;
const EMERGENCY_STOP_KEY: u32 = VK_F12.0 as u32;
const GAME_MODE_REPLAY_WATCHDOG_MS: u128 = 1_500;

static RUNTIME_HANDLE: OnceLock<Mutex<Option<SyncRuntimeHandle>>> = OnceLock::new();
static HOOK_CONTEXT: OnceLock<Mutex<Option<Arc<SyncRuntimeContext>>>> = OnceLock::new();

pub fn start_input_sync(session_id: &str, config: SessionConfig) -> Result<()> {
  let _ = stop_input_sync(None)?;

  let context = Arc::new(SyncRuntimeContext::new(session_id, config)?);
  context.log_session_targets();
  let stop_flag = Arc::new(AtomicBool::new(false));
  let (started_tx, started_rx) = mpsc::sync_channel::<Result<u32, String>>(1);

  let thread_context = Arc::clone(&context);
  let thread_stop_flag = Arc::clone(&stop_flag);
  let join_handle = thread::Builder::new()
    .name(format!("mirror-sync-{}", session_id))
    .spawn(move || run_message_loop(thread_context, thread_stop_flag, started_tx))
    .map_err(|error| anyhow!("Failed to start sync thread: {error}"))?;

  let thread_id = match started_rx.recv_timeout(Duration::from_secs(5)) {
    Ok(Ok(thread_id)) => thread_id,
    Ok(Err(error)) => {
      let _ = join_handle.join();
      return Err(anyhow!(error));
    }
    Err(error) => {
      let _ = join_handle.join();
      return Err(anyhow!("Timed out while starting sync hooks: {error}"));
    }
  };

  let mut runtime_slot = runtime_handle_slot()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  *runtime_slot = Some(SyncRuntimeHandle {
    session_id: session_id.to_string(),
    thread_id,
    stop_flag,
    join_handle: Some(join_handle),
  });

  Ok(())
}

pub fn stop_input_sync(session_id: Option<&str>) -> Result<bool> {
  let runtime = {
    let mut runtime_slot = runtime_handle_slot()
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());

    match runtime_slot.as_ref() {
      Some(handle) if session_id.map(|expected| expected == handle.session_id).unwrap_or(true) => {
        runtime_slot.take()
      }
      Some(_) => return Ok(false),
      None => return Ok(false),
    }
  };

  if let Some(mut runtime) = runtime {
    runtime.stop()?;
    return Ok(true);
  }

  Ok(false)
}

struct SyncRuntimeHandle {
  session_id: String,
  thread_id: u32,
  stop_flag: Arc<AtomicBool>,
  join_handle: Option<JoinHandle<()>>,
}

impl SyncRuntimeHandle {
  fn stop(&mut self) -> Result<()> {
    self.stop_flag.store(true, Ordering::Relaxed);
    unsafe {
      let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
    }

    if let Some(join_handle) = self.join_handle.take() {
      join_handle
        .join()
        .map_err(|_| anyhow!("Sync thread panicked while stopping."))?;
    }

    Ok(())
  }
}

struct SyncRuntimeContext {
  session_id: String,
  config: SessionConfig,
  primary_root: usize,
  target_roots: Mutex<Vec<usize>>,
  pointer_state: Mutex<PointerState>,
  logged_dispatch_targets: Mutex<HashSet<usize>>,
  throttled_logs: Mutex<HashMap<String, u128>>,
  circuit_breaker_open: AtomicBool,
  replay_in_progress: AtomicBool,
}

#[derive(Default)]
struct PointerState {
  buttons_down: u32,
  drag_from_primary: bool,
  drag_origin: Option<POINT>,
  buffered_gesture: Option<BufferedPointerGesture>,
}

#[derive(Clone)]
struct BufferedPointerGesture {
  button_mask: u32,
  down_message: u32,
  up_message: u32,
  points: Vec<NormalizedPoint>,
}

#[derive(Clone, Copy, Default)]
struct DispatchSummary {
  attempted: usize,
  succeeded: usize,
  failed: usize,
  window_message: usize,
  send_input: usize,
}

enum DispatchOutcome {
  Success(DispatchMode),
  Failed(DispatchMode, String),
  Skipped(String),
}

impl SyncRuntimeContext {
  fn new(session_id: &str, mut config: SessionConfig) -> Result<Self> {
    let primary_root = parse_hwnd(&config.primary_window_id)?;
    if unsafe { !IsWindow(primary_root).as_bool() } {
      return Err(anyhow!("Primary window for session '{session_id}' is no longer valid."));
    }

    let mut seen = HashSet::new();
    let mut target_roots = Vec::new();
    for window_id in &config.target_window_ids {
      let target = parse_hwnd(window_id)?;
      let handle = hwnd_to_handle(target);
      if unsafe { IsWindow(target).as_bool() } && seen.insert(handle) {
        target_roots.push(handle);
      }
    }

    config.target_window_ids = config
      .target_window_ids
      .into_iter()
      .filter(|window_id| parse_hwnd(window_id).map(|hwnd| unsafe { IsWindow(hwnd).as_bool() }).unwrap_or(false))
      .collect();

    if target_roots.is_empty() {
      return Err(anyhow!(
        "No live target windows were available when starting sync."
      ));
    }

    Ok(Self {
      session_id: session_id.to_string(),
      config,
      primary_root: hwnd_to_handle(primary_root),
      target_roots: Mutex::new(target_roots),
      pointer_state: Mutex::new(PointerState::default()),
      logged_dispatch_targets: Mutex::new(HashSet::new()),
      throttled_logs: Mutex::new(HashMap::new()),
      circuit_breaker_open: AtomicBool::new(false),
      replay_in_progress: AtomicBool::new(false),
    })
  }

  fn handle_mouse(&self, message: u32, hook: &MSLLHOOKSTRUCT) -> bool {
    let target_roots = self.target_roots_snapshot();
    if (hook.flags & LLMHF_INJECTED) != 0 || target_roots.is_empty() {
      return false;
    }

    if self.replay_in_progress.load(Ordering::Acquire) {
      return is_physical_mouse_message(message);
    }

    if self.circuit_breaker_open.load(Ordering::Acquire) {
      return false;
    }

    if unsafe { !IsWindow(handle_to_hwnd(self.primary_root)).as_bool() } {
      self.log_throttled(
        LogLevel::Warn,
        "primary_window_invalid",
        format!(
          "Session {} paused mirroring because the primary window is no longer valid.",
          self.session_id
        ),
        PRIMARY_INVALID_LOG_COOLDOWN_MS,
      );
      return false;
    }

    if message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
      self.handle_wheel(message, hook, &target_roots);
      return false;
    }

    let mut pointer_state = self
      .pointer_state
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());

    let primary_screen_point = hook.pt;
    let source_root = root_window_from_point(primary_screen_point);
    let inside_primary = source_root == self.primary_root;
    let started_drag = pointer_state.drag_from_primary;

    if !should_process_mouse_message(
      message,
      inside_primary,
      started_drag,
      self.config.sync_mouse_move,
      self.config.game_mode,
    ) {
      return false;
    }

    let Some((primary_client_point, primary_client_rect)) = screen_to_client_bounds(handle_to_hwnd(self.primary_root), primary_screen_point) else {
      return false;
    };

    if !started_drag && !inside_primary && !is_button_up_message(message) {
      return false;
    }

    if !started_drag && !is_point_inside_client(primary_client_point, primary_client_rect) && !is_button_up_message(message) {
      return false;
    }

    let Some(normalized_point) = normalize_client_point(
      primary_client_point.x,
      primary_client_point.y,
      &bounds_from_client_rect(primary_client_rect),
    ) else {
      return false;
    };

    let mut gesture_to_replay = None;

    match mouse_button_from_message(message) {
      Some(button) if is_button_down_message(message) => {
        pointer_state.buttons_down |= button;
        pointer_state.drag_from_primary = true;
        pointer_state.drag_origin = Some(primary_client_point);
        pointer_state.buffered_gesture = Some(BufferedPointerGesture {
          button_mask: button,
          down_message: message,
          up_message: button_up_message_for(message),
          points: vec![normalized_point],
        });
        return false;
      }
      Some(_) if message == WM_MOUSEMOVE && pointer_state.buttons_down != 0 => {
        if !should_treat_as_click_hold(&pointer_state, primary_client_point) {
          if let Some(gesture) = pointer_state.buffered_gesture.as_mut() {
            if should_append_buffered_point(gesture, normalized_point, self.config.game_mode) {
              gesture.points.push(normalized_point);
            }
          }
        }
        return false;
      }
      Some(button) if is_button_up_message(message) => {
        if let Some(gesture) = pointer_state.buffered_gesture.as_mut() {
          if gesture.button_mask == button
            && should_append_buffered_point(gesture, normalized_point, self.config.game_mode)
          {
            gesture.points.push(normalized_point);
          }
        }
        pointer_state.buttons_down &= !button;
        if pointer_state.buttons_down == 0 {
          pointer_state.drag_from_primary = false;
          pointer_state.drag_origin = None;
        }
        if let Some(gesture) = pointer_state.buffered_gesture.take() {
          if gesture.button_mask == button {
            gesture_to_replay = Some(gesture);
          }
        }
      }
      _ => {}
    }

    if let Some(gesture) = gesture_to_replay {
      drop(pointer_state);
      if self
        .replay_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
      {
        return true;
      }
      if let Some(context) = current_hook_context() {
        let mouse_data = hook.mouseData;
        thread::spawn(move || {
          thread::sleep(Duration::from_millis(PRIMARY_CLICK_SETTLE_MS));
          context.replay_buffered_gesture(gesture, mouse_data);
          if context.config.layout_mode == LayoutMode::Stack && is_click_message(message) {
            let _ = bring_window_to_front(&context.config.primary_window_id);
          }
        });
      } else {
        self.replay_buffered_gesture(gesture, hook.mouseData);
        if self.config.layout_mode == LayoutMode::Stack && is_click_message(message) {
          let _ = bring_window_to_front(&self.config.primary_window_id);
        }
      }
      return false;
    }

    if self.config.layout_mode == LayoutMode::Stack && is_click_message(message) {
      let _ = bring_window_to_front(&self.config.primary_window_id);
    }

    false
  }

  fn log_session_targets(&self) {
    let target_roots = self.target_roots_snapshot();
    let targets = if target_roots.is_empty() {
      "[]".to_string()
    } else {
      format!(
        "[{}]",
        target_roots
          .iter()
          .map(|target| format_handle_handle(*target))
          .collect::<Vec<_>>()
          .join(", ")
      )
    };

    push_runtime_log(
      LogLevel::Info,
      format!(
        "Session {} active targets resolved. primary={}, targets={}, count={}.",
        self.session_id,
        format_handle_handle(self.primary_root),
        targets,
        target_roots.len()
      ),
    );
  }

  fn handle_wheel(&self, message: u32, hook: &MSLLHOOKSTRUCT, target_roots: &[usize]) {
    if self.circuit_breaker_open.load(Ordering::Acquire) {
      return;
    }

    let primary_screen_point = hook.pt;
    let source_root = root_window_from_point(primary_screen_point);
    if source_root != self.primary_root {
      return;
    }

    let Some((primary_client_point, primary_client_rect)) =
      screen_to_client_bounds(handle_to_hwnd(self.primary_root), primary_screen_point)
    else {
      return;
    };

    if !is_point_inside_client(primary_client_point, primary_client_rect) {
      return;
    }

    let Some(normalized_point) = normalize_client_point(
      primary_client_point.x,
      primary_client_point.y,
      &bounds_from_client_rect(primary_client_rect),
    ) else {
      return;
    };

    let mut dummy_pointer_state = PointerState::default();
    let mut summary = DispatchSummary::default();
    for &target_root in target_roots {
      summary.attempted += 1;
      let outcome = self.dispatch_mouse_to_target(
        target_root,
        message,
        normalized_point,
        0,
        hook.mouseData,
        &mut dummy_pointer_state,
      );
      self.apply_dispatch_outcome(&mut summary, target_root, message, outcome);
    }

    self.log_summary_if_needed(message, summary);
  }

  fn replay_buffered_gesture(&self, gesture: BufferedPointerGesture, mouse_data: u32) {
    struct ReplayGuard<'a> {
      flag: &'a AtomicBool,
    }

    impl Drop for ReplayGuard<'_> {
      fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
      }
    }

    let _replay_guard = ReplayGuard {
      flag: &self.replay_in_progress,
    };
    let target_roots = self.target_roots_snapshot();
    if target_roots.is_empty() {
      return;
    }
    let replay_started_at = Instant::now();
    let mut summary = DispatchSummary::default();
    let cursor_before = current_cursor_pos();
    self.log_throttled(
      LogLevel::Info,
      format!(
        "replay:{}:{}:{}",
        self.session_id,
        gesture.down_message,
        gesture.points.len()
      ),
      format!(
        "Session {} replaying buffered {} gesture with {} points to {} targets.",
        self.session_id,
        mouse_message_name(gesture.down_message),
        gesture.points.len(),
        target_roots.len()
      ),
      SUMMARY_LOG_COOLDOWN_MS,
    );

    for &target_root in &target_roots {
      if self.circuit_breaker_open.load(Ordering::Acquire) {
        break;
      }
      summary.attempted += 1;
      let outcome = self.replay_gesture_to_target(target_root, &gesture, mouse_data);
      self.apply_dispatch_outcome(&mut summary, target_root, gesture.up_message, outcome);
    }

    self.focus_window(handle_to_hwnd(self.primary_root));
    restore_cursor_pos(cursor_before);

    if self.config.game_mode && replay_started_at.elapsed().as_millis() > GAME_MODE_REPLAY_WATCHDOG_MS {
      self.trip_circuit_breaker(format!(
        "Game mode watchdog stopped mirroring because one replay took more than {} ms.",
        GAME_MODE_REPLAY_WATCHDOG_MS
      ));
    }

    self.log_summary_if_needed(gesture.up_message, summary);
  }

  fn replay_gesture_to_target(
    &self,
    target_root: usize,
    gesture: &BufferedPointerGesture,
    mouse_data: u32,
  ) -> DispatchOutcome {
    let replay_points = if self.config.game_mode {
      downsample_gesture_points(&gesture.points, MAX_GAME_MODE_GESTURE_POINTS)
    } else {
      gesture.points.clone()
    };

    let Some(first_point) = replay_points.first().copied() else {
      return DispatchOutcome::Skipped("buffered gesture had no points".to_string());
    };

    let target_hwnd = handle_to_hwnd(target_root);
    if unsafe { !IsWindow(target_hwnd).as_bool() } {
      self.mark_target_unavailable(target_root, "target window is no longer valid");
      return DispatchOutcome::Skipped("target window is no longer valid".to_string());
    }

    if self.config.game_mode {
      let Ok(target_client_rect) = get_client_rect(target_hwnd) else {
        self.mark_target_unavailable(target_root, "failed to read target client rect");
        return DispatchOutcome::Skipped("failed to read target client rect".to_string());
      };
      let target_bounds = bounds_from_client_rect(target_client_rect);
      return match GameInputDispatcher::dispatch_gesture(
        target_hwnd,
        &replay_points,
        &target_bounds,
        gesture.down_message,
        gesture.up_message,
        true,
      ) {
        Ok(()) => DispatchOutcome::Success(DispatchMode::SendInput),
        Err(error) => DispatchOutcome::Failed(DispatchMode::SendInput, error.to_string()),
      };
    }

    self.focus_window(target_hwnd);

    let down_outcome =
      self.dispatch_send_input_to_target(target_root, gesture.down_message, first_point, mouse_data);
    if !matches!(down_outcome, DispatchOutcome::Success(_)) {
      return down_outcome;
    }

    for point in replay_points
      .iter()
      .copied()
      .skip(1)
      .take(replay_points.len().saturating_sub(2))
    {
      let move_outcome =
        self.dispatch_send_input_to_target(target_root, WM_MOUSEMOVE, point, mouse_data);
      if !matches!(move_outcome, DispatchOutcome::Success(_)) {
        return move_outcome;
      }
    }

    let last_point = replay_points.last().copied().unwrap_or(first_point);
    self.dispatch_send_input_to_target(target_root, gesture.up_message, last_point, mouse_data)
  }

  fn apply_dispatch_outcome(
    &self,
    summary: &mut DispatchSummary,
    target_root: usize,
    message: u32,
    outcome: DispatchOutcome,
  ) {
    match outcome {
      DispatchOutcome::Success(mode) => {
        summary.succeeded += 1;
        match mode {
          DispatchMode::WindowMessage => summary.window_message += 1,
          DispatchMode::SendInput => summary.send_input += 1,
          DispatchMode::Auto => {}
        }
      }
      DispatchOutcome::Failed(mode, error) => {
        summary.failed += 1;
        match mode {
          DispatchMode::WindowMessage => summary.window_message += 1,
          DispatchMode::SendInput => summary.send_input += 1,
          DispatchMode::Auto => {}
        }
        self.log_dispatch_issue(target_root, message, mode, &error);
      }
      DispatchOutcome::Skipped(reason) => {
        summary.failed += 1;
        self.log_dispatch_skip(target_root, message, &reason);
      }
    }
  }

  fn log_summary_if_needed(&self, message: u32, summary: DispatchSummary) {
    if summary.failed == 0 {
      return;
    }

    self.log_throttled(
      LogLevel::Warn,
      format!("summary:{}:{}", self.session_id, message),
      format!(
        "Session {} mirrored {} to {}/{} targets. window_message={}, send_input={}, failed={}.",
        self.session_id,
        mouse_message_name(message),
        summary.succeeded,
        summary.attempted,
        summary.window_message,
        summary.send_input,
        summary.failed
      ),
      SUMMARY_LOG_COOLDOWN_MS,
    );
  }

  fn dispatch_mouse_to_target(
    &self,
    target_root_handle: usize,
    message: u32,
    normalized_point: NormalizedPoint,
    buttons_down: u32,
    mouse_data: u32,
    pointer_state: &mut PointerState,
  ) -> DispatchOutcome {
    let target_root = handle_to_hwnd(target_root_handle);
    if unsafe { !IsWindow(target_root).as_bool() } {
      self.mark_target_unavailable(target_root_handle, "target window is no longer valid");
      return DispatchOutcome::Skipped("target window is no longer valid".to_string());
    }

    let Ok(target_client_rect) = get_client_rect(target_root) else {
      self.mark_target_unavailable(target_root_handle, "failed to read target client rect");
      return DispatchOutcome::Skipped("failed to read target client rect".to_string());
    };
    let target_bounds = bounds_from_client_rect(target_client_rect);

    // Quyết định dispatch mode
    let dispatch_mode = match self.config.dispatch_mode {
      DispatchMode::WindowMessage => DispatchMode::WindowMessage,
      DispatchMode::SendInput => DispatchMode::SendInput,
      DispatchMode::Auto => detect_dispatch_mode(target_root),
    };
    self.log_dispatch_mode_once(target_root_handle, dispatch_mode);

    // Sử dụng SendInput cho game
    if dispatch_mode == DispatchMode::SendInput {
      return match GameInputDispatcher::dispatch_mouse(
        target_root,
        normalized_point,
        &target_bounds,
        message,
        mouse_data,
        self.config.game_mode,
      ) {
        Ok(()) => DispatchOutcome::Success(DispatchMode::SendInput),
        Err(error) => DispatchOutcome::Failed(DispatchMode::SendInput, error.to_string()),
      };
    }

    // Sử dụng WindowMessage cho browser và ứng dụng thông thường
    let (target_client_x, target_client_y) = project_to_client(normalized_point, &target_bounds);
    let root_client_point = POINT {
      x: target_client_x,
      y: target_client_y,
    };

    let (dispatch_hwnd, dispatch_point) = resolve_deepest_dispatch_hwnd(target_root, root_client_point);
    let key_state = current_mouse_modifier_state(buttons_down);

    let _ = pointer_state;

    if message != WM_MOUSEMOVE {
      let moved = dispatch_window_message_sync(
        dispatch_hwnd,
        WM_MOUSEMOVE,
        WPARAM(key_state as usize),
        LPARAM(pack_point(dispatch_point)),
      );
      if !moved {
        return DispatchOutcome::Failed(
          DispatchMode::WindowMessage,
          "pre-click mouse move dispatch returned failure".to_string(),
        );
      }
      self.log_click_trace(
        target_root_handle,
        "pre_click_move",
        DispatchMode::WindowMessage,
      );
      thread::sleep(Duration::from_millis(PRE_CLICK_MOVE_SETTLE_MS));
    }

    if message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
      let mut screen_point = root_client_point;
      if ensure_bool(unsafe { ClientToScreen(target_root, &mut screen_point) }, "ClientToScreen").is_err() {
        return DispatchOutcome::Failed(
          DispatchMode::WindowMessage,
          "ClientToScreen failed for wheel dispatch".to_string(),
        );
      }

      let delta = hiword(mouse_data as usize) as i16;
      let wparam = (((delta as u16 as u32) << 16) | key_state as u32) as usize;
      let dispatched = dispatch_window_message(
        dispatch_hwnd,
        message,
        WPARAM(wparam),
        LPARAM(pack_point(screen_point)),
      );
      thread::sleep(Duration::from_millis(INTER_TARGET_CLICK_SETTLE_MS));
      return if dispatched {
        self.log_click_trace(target_root_handle, mouse_message_name(message), DispatchMode::WindowMessage);
        DispatchOutcome::Success(DispatchMode::WindowMessage)
      } else {
        DispatchOutcome::Failed(
          DispatchMode::WindowMessage,
          "wheel message dispatch returned failure".to_string(),
        )
      };
    }

    let dispatched = dispatch_window_message(
      dispatch_hwnd,
      message,
      WPARAM(key_state as usize),
      LPARAM(pack_point(dispatch_point)),
    );
    if is_click_message(message) {
      thread::sleep(Duration::from_millis(INTER_TARGET_CLICK_SETTLE_MS));
    }
    if dispatched {
      self.log_click_trace(target_root_handle, mouse_message_name(message), DispatchMode::WindowMessage);
      DispatchOutcome::Success(DispatchMode::WindowMessage)
    } else {
      DispatchOutcome::Failed(
        DispatchMode::WindowMessage,
        "mouse message dispatch returned failure".to_string(),
      )
    }
  }

  fn log_dispatch_mode_once(&self, target_root_handle: usize, dispatch_mode: DispatchMode) {
    let mut logged_targets = self
      .logged_dispatch_targets
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !logged_targets.insert(target_root_handle) {
      return;
    }

    push_runtime_log(
      LogLevel::Info,
      format!(
        "Session {} target {} using {} dispatch mode.",
        self.session_id,
        format_handle_handle(target_root_handle),
        dispatch_mode_name(dispatch_mode)
      ),
    );
  }

  fn log_dispatch_issue(
    &self,
    target_root_handle: usize,
    message: u32,
    dispatch_mode: DispatchMode,
    error: &str,
  ) {
    let level = if is_click_message(message) || message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
      LogLevel::Warn
    } else {
      LogLevel::Info
    };
    self.log_throttled(
      level,
      format!(
        "dispatch_issue:{}:{}:{}:{}",
        self.session_id,
        target_root_handle,
        message,
        error
      ),
      format!(
        "Session {} failed to mirror {} to target {} via {}: {}.",
        self.session_id,
        mouse_message_name(message),
        format_handle_handle(target_root_handle),
        dispatch_mode_name(dispatch_mode),
        error
      ),
      ISSUE_LOG_COOLDOWN_MS,
    );
  }

  fn log_dispatch_skip(&self, target_root_handle: usize, message: u32, reason: &str) {
    let level = if is_click_message(message) || message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
      LogLevel::Warn
    } else {
      LogLevel::Info
    };
    self.log_throttled(
      level,
      format!(
        "dispatch_skip:{}:{}:{}:{}",
        self.session_id,
        target_root_handle,
        message,
        reason
      ),
      format!(
        "Session {} skipped {} for target {}: {}.",
        self.session_id,
        mouse_message_name(message),
        format_handle_handle(target_root_handle),
        reason
      ),
      ISSUE_LOG_COOLDOWN_MS,
    );
  }

  fn log_click_trace(&self, target_root_handle: usize, stage: &str, dispatch_mode: DispatchMode) {
    if stage == "mouse_move" {
      return;
    }

    self.log_throttled(
      LogLevel::Info,
      format!(
        "click_trace:{}:{}:{}:{}",
        self.session_id,
        target_root_handle,
        stage,
        dispatch_mode_name(dispatch_mode)
      ),
      format!(
        "Session {} target {} {} via {}.",
        self.session_id,
        format_handle_handle(target_root_handle),
        stage,
        dispatch_mode_name(dispatch_mode)
      ),
      CLICK_TRACE_LOG_COOLDOWN_MS,
    );
  }

  fn focus_window(&self, hwnd: HWND) {
    unsafe {
      let _ = SetForegroundWindow(hwnd);
    }
    let settle_ms = if self.config.game_mode {
      GAME_MODE_FOREGROUND_TARGET_SETTLE_MS
    } else {
      FOREGROUND_TARGET_SETTLE_MS
    };
    thread::sleep(Duration::from_millis(settle_ms));
  }

  fn dispatch_send_input_to_target(
    &self,
    target_root_handle: usize,
    message: u32,
    normalized_point: NormalizedPoint,
    mouse_data: u32,
  ) -> DispatchOutcome {
    let target_root = handle_to_hwnd(target_root_handle);
    if unsafe { !IsWindow(target_root).as_bool() } {
      self.mark_target_unavailable(target_root_handle, "target window is no longer valid");
      return DispatchOutcome::Skipped("target window is no longer valid".to_string());
    }

    let Ok(target_client_rect) = get_client_rect(target_root) else {
      self.mark_target_unavailable(target_root_handle, "failed to read target client rect");
      return DispatchOutcome::Skipped("failed to read target client rect".to_string());
    };
    let target_bounds = bounds_from_client_rect(target_client_rect);

    match GameInputDispatcher::dispatch_mouse(
      target_root,
      normalized_point,
      &target_bounds,
      message,
      mouse_data,
      self.config.game_mode,
    ) {
      Ok(()) => {
        self.log_click_trace(target_root_handle, mouse_message_name(message), DispatchMode::SendInput);
        if is_click_message(message) {
          thread::sleep(Duration::from_millis(INTER_TARGET_CLICK_SETTLE_MS));
        }
        DispatchOutcome::Success(DispatchMode::SendInput)
      }
      Err(error) => DispatchOutcome::Failed(DispatchMode::SendInput, error.to_string()),
    }
  }

  fn target_roots_snapshot(&self) -> Vec<usize> {
    self
      .target_roots
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
      .clone()
  }

  fn mark_target_unavailable(&self, target_root_handle: usize, reason: &str) {
    let removed = {
      let mut targets = self
        .target_roots
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
      let original_len = targets.len();
      targets.retain(|handle| *handle != target_root_handle);
      targets.len() != original_len
    };

    if removed {
      self
        .logged_dispatch_targets
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&target_root_handle);
    }

    self.log_throttled(
      LogLevel::Warn,
      format!(
        "target_removed:{}:{}:{}",
        self.session_id,
        target_root_handle,
        reason
      ),
      format!(
        "Session {} removed target {} from live mirroring: {}.",
        self.session_id,
        format_handle_handle(target_root_handle),
        reason
      ),
      TARGET_REMOVAL_LOG_COOLDOWN_MS,
    );
  }

  fn log_throttled(
    &self,
    level: LogLevel,
    key: impl Into<String>,
    message: impl Into<String>,
    cooldown_ms: u128,
  ) {
    let key = key.into();
    let now = unix_time_ms();
    let should_log = {
      let mut throttled_logs = self
        .throttled_logs
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
      match throttled_logs.get(&key).copied() {
        Some(last_logged_at) if now.saturating_sub(last_logged_at) < cooldown_ms => false,
        _ => {
          throttled_logs.insert(key, now);
          true
        }
      }
    };

    if should_log {
      push_runtime_log(level, message);
    }
  }

  fn handle_emergency_stop_hotkey(&self, message: u32, keyboard: &KBDLLHOOKSTRUCT) -> bool {
    if (keyboard.flags & LLKHF_INJECTED).0 != 0 {
      return false;
    }

    if !matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
      return false;
    }

    if keyboard.vkCode != EMERGENCY_STOP_KEY {
      return false;
    }

    let ctrl_down = (unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000) != 0;
    let shift_down = (unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } as u16 & 0x8000) != 0;
    if !(ctrl_down && shift_down) {
      return false;
    }

    self.trip_circuit_breaker(
      "Emergency stop activated by Ctrl+Shift+F12. Mirroring has been cut off immediately."
        .to_string(),
    );
    true
  }

  fn trip_circuit_breaker(&self, reason: String) {
    let was_open = self.circuit_breaker_open.swap(true, Ordering::AcqRel);
    if !was_open {
      self.log_throttled(
        LogLevel::Error,
        format!("circuit_breaker:{}", self.session_id),
        reason,
        1_000,
      );
    }
  }

}

fn run_message_loop(
  context: Arc<SyncRuntimeContext>,
  stop_flag: Arc<AtomicBool>,
  started_tx: SyncSender<Result<u32, String>>,
) {
  let thread_id = unsafe { GetCurrentThreadId() };

  let mouse_hook = match install_mouse_hook(Arc::clone(&context)) {
    Ok(hook) => hook,
    Err(error) => {
      let _ = started_tx.send(Err(error.to_string()));
      return;
    }
  };
  let keyboard_hook = match install_keyboard_hook(Arc::clone(&context)) {
    Ok(hook) => hook,
    Err(error) => {
      unsafe {
        let _ = UnhookWindowsHookEx(mouse_hook);
      }
      let _ = started_tx.send(Err(error.to_string()));
      clear_hook_context();
      return;
    }
  };

  let _ = started_tx.send(Ok(thread_id));

  let mut message = MSG::default();
  loop {
    if stop_flag.load(Ordering::Relaxed) {
      break;
    }

    let status = unsafe { GetMessageW(&mut message, HWND(null_mut()), 0, 0) };
    if status.0 == -1 {
      push_runtime_log(
        LogLevel::Error,
        format!(
          "Session {} sync loop stopped because GetMessageW returned an error.",
          context.session_id
        ),
      );
      break;
    }
    if status.0 == 0 || message.message == WM_QUIT {
      break;
    }

    unsafe {
      let _ = TranslateMessage(&message);
      let _ = DispatchMessageW(&message);
    }
  }

  unsafe {
    let _ = UnhookWindowsHookEx(keyboard_hook);
    let _ = UnhookWindowsHookEx(mouse_hook);
  }

  clear_hook_context();
}

fn install_mouse_hook(context: Arc<SyncRuntimeContext>) -> Result<HHOOK> {
  set_hook_context(Some(context));

  let module = unsafe { GetModuleHandleW(None) }
    .map_err(|error| anyhow!("Failed to resolve current module for hooks: {error}"))?;
  let hook_instance = HINSTANCE(module.0);
  let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), hook_instance, 0) };
  mouse_hook.map_err(|error| {
    clear_hook_context();
    anyhow!("Failed to install mouse hook: {error}")
  })
}

fn install_keyboard_hook(context: Arc<SyncRuntimeContext>) -> Result<HHOOK> {
  set_hook_context(Some(context));

  let module = unsafe { GetModuleHandleW(None) }
    .map_err(|error| anyhow!("Failed to resolve current module for keyboard hooks: {error}"))?;
  let hook_instance = HINSTANCE(module.0);
  let keyboard_hook =
    unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), hook_instance, 0) };
  keyboard_hook.map_err(|error| {
    clear_hook_context();
    anyhow!("Failed to install keyboard hook: {error}")
  })
}

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
  if code == HC_ACTION as i32 {
    if let Some(context) = current_hook_context() {
      let hook = &*(lparam.0 as *const MSLLHOOKSTRUCT);
      let hook_result = catch_unwind(AssertUnwindSafe(|| context.handle_mouse(wparam.0 as u32, hook)));
      match hook_result {
        Ok(true) => return LRESULT(1),
        Ok(false) => {}
        Err(_) => {
          push_runtime_log(
            LogLevel::Error,
            "Recovered from an unexpected panic inside the Windows mouse hook.",
          );
        }
      }
    }
  }

  CallNextHookEx(HHOOK(null_mut()), code, wparam, lparam)
}

unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
  if code == HC_ACTION as i32 {
    if let Some(context) = current_hook_context() {
      let keyboard = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
      let hook_result = catch_unwind(AssertUnwindSafe(|| {
        context.handle_emergency_stop_hotkey(wparam.0 as u32, keyboard)
      }));
      match hook_result {
        Ok(true) => return LRESULT(1),
        Ok(false) => {}
        Err(_) => {
          push_runtime_log(
            LogLevel::Error,
            "Recovered from an unexpected panic inside the Windows keyboard hook.",
          );
        }
      }
    }
  }

  CallNextHookEx(HHOOK(null_mut()), code, wparam, lparam)
}

fn set_hook_context(context: Option<Arc<SyncRuntimeContext>>) {
  let mut slot = hook_context_slot()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner());
  *slot = context;
}

fn clear_hook_context() {
  set_hook_context(None);
}

fn current_hook_context() -> Option<Arc<SyncRuntimeContext>> {
  hook_context_slot()
    .lock()
    .unwrap_or_else(|poisoned| poisoned.into_inner())
    .clone()
}

fn runtime_handle_slot() -> &'static Mutex<Option<SyncRuntimeHandle>> {
  RUNTIME_HANDLE.get_or_init(|| Mutex::new(None))
}

fn hook_context_slot() -> &'static Mutex<Option<Arc<SyncRuntimeContext>>> {
  HOOK_CONTEXT.get_or_init(|| Mutex::new(None))
}

fn should_process_mouse_message(
  message: u32,
  inside_primary: bool,
  drag_from_primary: bool,
  sync_mouse_move: bool,
  game_mode: bool,
) -> bool {
  match message {
    WM_MOUSEMOVE => {
      if game_mode && !drag_from_primary {
        return false;
      }
      if drag_from_primary {
        sync_mouse_move
      } else {
        inside_primary && sync_mouse_move
      }
    }
    WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => inside_primary,
    WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => inside_primary || drag_from_primary,
    _ => false,
  }
}

fn is_physical_mouse_message(message: u32) -> bool {
  matches!(
    message,
    WM_MOUSEMOVE
      | WM_LBUTTONDOWN
      | WM_LBUTTONUP
      | WM_RBUTTONDOWN
      | WM_RBUTTONUP
      | WM_MBUTTONDOWN
      | WM_MBUTTONUP
      | WM_MOUSEWHEEL
      | WM_MOUSEHWHEEL
  )
}

fn resolve_deepest_dispatch_hwnd(root: HWND, root_point: POINT) -> (HWND, POINT) {
  let mut current = root;
  let mut current_point = root_point;

  loop {
    let child = unsafe {
      ChildWindowFromPointEx(
        current,
        current_point,
        CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT,
      )
    };

    if child.0.is_null() || child == current {
      return (current, current_point);
    }

    let mut mapped_points = [current_point];
    unsafe {
      let _ = MapWindowPoints(current, child, &mut mapped_points);
    }
    current = child;
    current_point = mapped_points[0];
  }
}

fn screen_to_client_bounds(hwnd: HWND, screen_point: POINT) -> Option<(POINT, RECT)> {
  let mut client_point = screen_point;
  if ensure_bool(unsafe { ScreenToClient(hwnd, &mut client_point) }, "ScreenToClient").is_err() {
    return None;
  }

  let Ok(client_rect) = get_client_rect(hwnd) else {
    return None;
  };

  Some((client_point, client_rect))
}

fn get_client_rect(hwnd: HWND) -> Result<RECT> {
  let mut rect = RECT::default();
  unsafe {
    GetClientRect(hwnd, &mut rect)?;
  }
  Ok(rect)
}

fn current_mouse_modifier_state(buttons_down: u32) -> u16 {
  let mut key_state = 0u16;

  if (buttons_down & button_mask(MouseButton::Left)) != 0 {
    key_state |= MK_LBUTTON.0 as u16;
  }
  if (buttons_down & button_mask(MouseButton::Right)) != 0 {
    key_state |= MK_RBUTTON.0 as u16;
  }
  if (buttons_down & button_mask(MouseButton::Middle)) != 0 {
    key_state |= MK_MBUTTON.0 as u16;
  }
  if (unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } as u16 & 0x8000) != 0 {
    key_state |= MK_SHIFT.0 as u16;
  }
  if (unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000) != 0 {
    key_state |= MK_CONTROL.0 as u16;
  }

  key_state
}

fn dispatch_window_message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
  if should_send_mouse_synchronously(message) {
    return dispatch_window_message_sync(hwnd, message, wparam, lparam);
  }

  unsafe { PostMessageW(hwnd, message, wparam, lparam).is_ok() }
}

fn dispatch_window_message_sync(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
  let mut result = 0usize;
  unsafe {
    SendMessageTimeoutW(
      hwnd,
      message,
      wparam,
      lparam,
      SMTO_ABORTIFHUNG | SMTO_BLOCK,
      60,
      Some(&mut result),
    )
    .0 != 0
  }
}

fn pack_point(point: POINT) -> isize {
  let x = point.x as u16 as u32;
  let y = point.y as u16 as u32;
  ((y << 16) | x) as isize
}

fn hiword(value: usize) -> u16 {
  ((value >> 16) & 0xffff) as u16
}

fn current_cursor_pos() -> Option<POINT> {
  let mut point = POINT::default();
  if unsafe { GetCursorPos(&mut point) }.is_ok() {
    Some(point)
  } else {
    None
  }
}

fn restore_cursor_pos(point: Option<POINT>) {
  let Some(point) = point else {
    return;
  };

  unsafe {
    let _ = SetCursorPos(point.x, point.y);
  }
}

fn root_window_from_point(screen_point: POINT) -> usize {
  let hwnd = unsafe { WindowFromPoint(screen_point) };
  root_window(hwnd)
}

fn root_window(hwnd: HWND) -> usize {
  if hwnd.0.is_null() {
    return 0;
  }

  let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
  if root.0.is_null() {
    hwnd_to_handle(hwnd)
  } else {
    hwnd_to_handle(root)
  }
}

fn hwnd_to_handle(hwnd: HWND) -> usize {
  hwnd.0 as usize
}

fn handle_to_hwnd(handle: usize) -> HWND {
  HWND(handle as *mut _)
}

fn bounds_from_client_rect(rect: RECT) -> Bounds {
  Bounds {
    x: 0,
    y: 0,
    width: rect.right - rect.left,
    height: rect.bottom - rect.top,
  }
}

fn is_point_inside_client(point: POINT, rect: RECT) -> bool {
  point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

fn should_treat_as_click_hold(pointer_state: &PointerState, current_point: POINT) -> bool {
  if pointer_state.buttons_down == 0 {
    return false;
  }

  let Some(origin) = pointer_state.drag_origin else {
    return false;
  };

  let dx = (current_point.x - origin.x).abs();
  let dy = (current_point.y - origin.y).abs();
  dx <= CLICK_SLOP_PX && dy <= CLICK_SLOP_PX
}

fn is_button_down_message(message: u32) -> bool {
  matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
}

fn is_button_up_message(message: u32) -> bool {
  matches!(message, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP)
}

fn button_up_message_for(message: u32) -> u32 {
  match message {
    WM_LBUTTONDOWN => WM_LBUTTONUP,
    WM_RBUTTONDOWN => WM_RBUTTONUP,
    WM_MBUTTONDOWN => WM_MBUTTONUP,
    _ => message,
  }
}

fn is_click_message(message: u32) -> bool {
  is_button_down_message(message) || is_button_up_message(message)
}

fn should_send_mouse_synchronously(message: u32) -> bool {
  matches!(
    message,
    WM_LBUTTONDOWN
      | WM_LBUTTONUP
      | WM_RBUTTONDOWN
      | WM_RBUTTONUP
      | WM_MBUTTONDOWN
      | WM_MBUTTONUP
      | WM_MOUSEWHEEL
      | WM_MOUSEHWHEEL
  )
}

fn mouse_button_from_message(message: u32) -> Option<u32> {
  match message {
    WM_LBUTTONDOWN | WM_LBUTTONUP => Some(button_mask(MouseButton::Left)),
    WM_RBUTTONDOWN | WM_RBUTTONUP => Some(button_mask(MouseButton::Right)),
    WM_MBUTTONDOWN | WM_MBUTTONUP => Some(button_mask(MouseButton::Middle)),
    _ => None,
  }
}

fn should_append_buffered_point(
  gesture: &BufferedPointerGesture,
  point: NormalizedPoint,
  game_mode: bool,
) -> bool {
  let Some(last) = gesture.points.last() else {
    return true;
  };

  let minimum_delta = if game_mode {
    GAME_MODE_GESTURE_POINT_DELTA
  } else {
    DEFAULT_GESTURE_POINT_DELTA
  };
  let dx = (last.rx - point.rx).abs();
  let dy = (last.ry - point.ry).abs();
  dx > minimum_delta || dy > minimum_delta
}

fn downsample_gesture_points(points: &[NormalizedPoint], max_points: usize) -> Vec<NormalizedPoint> {
  if points.len() <= max_points || max_points < 2 {
    return points.to_vec();
  }

  let last_index = points.len() - 1;
  let interior_slots = max_points - 2;
  let mut reduced = Vec::with_capacity(max_points);
  reduced.push(points[0]);

  for slot in 1..=interior_slots {
    let index = (slot * last_index) / (interior_slots + 1);
    let clamped_index = index.clamp(1, last_index.saturating_sub(1));
    let point = points[clamped_index];
    if reduced.last().copied() != Some(point) {
      reduced.push(point);
    }
  }

  if reduced.last().copied() != Some(points[last_index]) {
    reduced.push(points[last_index]);
  }

  reduced
}

fn button_mask(button: MouseButton) -> u32 {
  match button {
    MouseButton::Left => 1,
    MouseButton::Right => 2,
    MouseButton::Middle => 4,
  }
}

/// Phát hiện dispatch mode phù hợp dựa trên class name và process name
fn mouse_message_name(message: u32) -> &'static str {
  match message {
    WM_MOUSEMOVE => "mouse_move",
    WM_LBUTTONDOWN => "left_down",
    WM_LBUTTONUP => "left_up",
    WM_RBUTTONDOWN => "right_down",
    WM_RBUTTONUP => "right_up",
    WM_MBUTTONDOWN => "middle_down",
    WM_MBUTTONUP => "middle_up",
    WM_MOUSEWHEEL => "wheel",
    WM_MOUSEHWHEEL => "hwheel",
    _ => "unknown_mouse",
  }
}

fn dispatch_mode_name(dispatch_mode: DispatchMode) -> &'static str {
  match dispatch_mode {
    DispatchMode::WindowMessage => "window_message",
    DispatchMode::SendInput => "send_input",
    DispatchMode::Auto => "auto",
  }
}

fn format_handle_handle(handle: usize) -> String {
  format!("0x{handle:X}")
}

fn detect_dispatch_mode(hwnd: HWND) -> DispatchMode {
  use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;
  use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
  
  // Lấy class name
  let mut class_name_buf = [0u16; 256];
  let class_len = unsafe { GetClassNameW(hwnd, &mut class_name_buf) };
  let class_name = if class_len > 0 {
    String::from_utf16_lossy(&class_name_buf[..class_len as usize]).to_lowercase()
  } else {
    String::new()
  };

  // Lấy process name
  let mut pid = 0u32;
  unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
  
  let process_name = if pid != 0 {
    unsafe {
      if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
        let mut path_buf = [0u16; 1024];
        let mut size = path_buf.len() as u32;
        let process_name = if QueryFullProcessImageNameW(
          handle,
          PROCESS_NAME_WIN32,
          windows::core::PWSTR(path_buf.as_mut_ptr()),
          &mut size,
        )
        .is_ok()
        {
          let path = String::from_utf16_lossy(&path_buf[..size as usize]);
          path.split('\\').last().unwrap_or("").to_lowercase()
        } else {
          String::new()
        };
        let _ = windows::Win32::Foundation::CloseHandle(handle);
        process_name
      } else {
        String::new()
      }
    }
  } else {
    String::new()
  };

  // Danh sách các ứng dụng cần dùng SendInput (game engines, game launchers)
  let game_indicators = [
    // Game engines
    "unityplayer", "ue4", "ue5", "unreal",
    // Game launchers
    "steam", "epic", "origin", "uplay", "battlenet",
    // DirectX/OpenGL windows
    "d3d", "opengl", "vulkan",
    // Các game phổ biến
    "godot",
    "dxgi",
  ];

  // Browser và ứng dụng thông thường dùng WindowMessage
  let browser_indicators = [
    "chrome", "firefox", "edge", "msedge", "brave", "opera",
    "electron", "cef", "notepad", "explorer", "code.exe", "vscode",
  ];

  // Kiểm tra browser trước
  for indicator in &browser_indicators {
    if process_name.contains(indicator) || class_name.contains(indicator) {
      return DispatchMode::WindowMessage;
    }
  }

  // Kiểm tra game
  for indicator in &game_indicators {
    if process_name.contains(indicator) || class_name.contains(indicator) {
      return DispatchMode::SendInput;
    }
  }

  // Mặc định dùng WindowMessage (an toàn hơn)
  DispatchMode::WindowMessage
}

#[derive(Clone, Copy)]
enum MouseButton {
  Left,
  Right,
  Middle,
}





