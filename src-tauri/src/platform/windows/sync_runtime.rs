use std::collections::HashSet;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, MapWindowPoints, ScreenToClient};
use windows::Win32::System::SystemServices::{
  MK_CONTROL, MK_LBUTTON, MK_MBUTTON, MK_RBUTTON, MK_SHIFT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
  CallNextHookEx, ChildWindowFromPointEx, DispatchMessageW, GetAncestor, GetClientRect,
  GetMessageW, HHOOK, IsWindow, LLMHF_INJECTED, MSLLHOOKSTRUCT, MSG, PostMessageW,
  PostThreadMessageW, SendMessageTimeoutW, SetWindowsHookExW, TranslateMessage,
  UnhookWindowsHookEx, WindowFromPoint, CWP_SKIPDISABLED, CWP_SKIPINVISIBLE,
  CWP_SKIPTRANSPARENT, GA_ROOT, HC_ACTION, SMTO_ABORTIFHUNG, SMTO_BLOCK, WH_MOUSE_LL,
  WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE,
  WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

use crate::coordinate_mapper::{normalize_client_point, project_to_client, NormalizedPoint};
use crate::models::{Bounds, LayoutMode, SessionConfig};

use super::{bring_window_to_front, ensure_bool, parse_hwnd};

static RUNTIME_HANDLE: OnceLock<Mutex<Option<SyncRuntimeHandle>>> = OnceLock::new();
static HOOK_CONTEXT: OnceLock<Mutex<Option<Arc<SyncRuntimeContext>>>> = OnceLock::new();

pub fn start_input_sync(session_id: &str, config: SessionConfig) -> Result<()> {
  let _ = stop_input_sync(None)?;

  let context = Arc::new(SyncRuntimeContext::new(session_id, config)?);
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
  config: SessionConfig,
  primary_root: usize,
  target_roots: Vec<usize>,
  pointer_state: Mutex<PointerState>,
}

#[derive(Default)]
struct PointerState {
  buttons_down: u32,
  drag_from_primary: bool,
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

    Ok(Self {
      config,
      primary_root: hwnd_to_handle(primary_root),
      target_roots,
      pointer_state: Mutex::new(PointerState::default()),
    })
  }

  fn handle_mouse(&self, message: u32, hook: &MSLLHOOKSTRUCT) {
    if (hook.flags & LLMHF_INJECTED) != 0 || self.target_roots.is_empty() {
      return;
    }

    let mut pointer_state = self
      .pointer_state
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner());

    let primary_screen_point = hook.pt;
    let source_root = root_window_from_point(primary_screen_point);
    let inside_primary = source_root == self.primary_root;
    let started_drag = pointer_state.drag_from_primary;

    if !should_process_mouse_message(message, inside_primary, started_drag, self.config.sync_mouse_move) {
      return;
    }

    let Some((primary_client_point, primary_client_rect)) = screen_to_client_bounds(handle_to_hwnd(self.primary_root), primary_screen_point) else {
      return;
    };

    if !started_drag && !inside_primary && !is_button_up_message(message) {
      return;
    }

    if !started_drag && !is_point_inside_client(primary_client_point, primary_client_rect) && !is_button_up_message(message) {
      return;
    }

    let Some(normalized_point) = normalize_client_point(
      primary_client_point.x,
      primary_client_point.y,
      &bounds_from_client_rect(primary_client_rect),
    ) else {
      return;
    };

    match mouse_button_from_message(message) {
      Some(button) if is_button_down_message(message) => {
        pointer_state.buttons_down |= button;
        pointer_state.drag_from_primary = true;
      }
      Some(button) if is_button_up_message(message) => {
        pointer_state.buttons_down &= !button;
        if pointer_state.buttons_down == 0 {
          pointer_state.drag_from_primary = false;
        }
      }
      _ => {}
    }

    let button_state = pointer_state.buttons_down;
    for &target_root in &self.target_roots {
      self.dispatch_mouse_to_target(
        target_root,
        message,
        normalized_point,
        button_state,
        hook.mouseData,
        &mut pointer_state,
      );
    }

    if self.config.layout_mode == LayoutMode::Stack && is_click_message(message) {
      let _ = bring_window_to_front(&self.config.primary_window_id);
    }
  }

  fn dispatch_mouse_to_target(
    &self,
    target_root_handle: usize,
    message: u32,
    normalized_point: NormalizedPoint,
    buttons_down: u32,
    mouse_data: u32,
    pointer_state: &mut PointerState,
  ) {
    let target_root = handle_to_hwnd(target_root_handle);
    if unsafe { !IsWindow(target_root).as_bool() } {
      return;
    }

    let Ok(target_client_rect) = get_client_rect(target_root) else {
      return;
    };
    let target_bounds = bounds_from_client_rect(target_client_rect);
    let (target_client_x, target_client_y) = project_to_client(normalized_point, &target_bounds);
    let root_client_point = POINT {
      x: target_client_x,
      y: target_client_y,
    };

    let (dispatch_hwnd, dispatch_point) = resolve_deepest_dispatch_hwnd(target_root, root_client_point);
    let key_state = current_mouse_modifier_state(buttons_down);

    let _ = pointer_state;

    if message != WM_MOUSEMOVE {
      dispatch_window_message(
        dispatch_hwnd,
        WM_MOUSEMOVE,
        WPARAM(key_state as usize),
        LPARAM(pack_point(dispatch_point)),
      );
    }

    if message == WM_MOUSEWHEEL || message == WM_MOUSEHWHEEL {
      let mut screen_point = root_client_point;
      if ensure_bool(unsafe { ClientToScreen(target_root, &mut screen_point) }, "ClientToScreen").is_err() {
        return;
      }

      let delta = hiword(mouse_data as usize) as i16;
      let wparam = (((delta as u16 as u32) << 16) | key_state as u32) as usize;
      dispatch_window_message(
        dispatch_hwnd,
        message,
        WPARAM(wparam),
        LPARAM(pack_point(screen_point)),
      );
      return;
    }

    dispatch_window_message(
      dispatch_hwnd,
      message,
      WPARAM(key_state as usize),
      LPARAM(pack_point(dispatch_point)),
    );
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

  let _ = started_tx.send(Ok(thread_id));

  let mut message = MSG::default();
  loop {
    if stop_flag.load(Ordering::Relaxed) {
      break;
    }

    let status = unsafe { GetMessageW(&mut message, HWND(null_mut()), 0, 0) };
    if status.0 == -1 || status.0 == 0 || message.message == WM_QUIT {
      break;
    }

    unsafe {
      let _ = TranslateMessage(&message);
      let _ = DispatchMessageW(&message);
    }
  }

  unsafe {
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

unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
  if code == HC_ACTION as i32 {
    if let Some(context) = current_hook_context() {
      let hook = &*(lparam.0 as *const MSLLHOOKSTRUCT);
      context.handle_mouse(wparam.0 as u32, hook);
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
) -> bool {
  match message {
    WM_MOUSEMOVE => {
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

fn dispatch_window_message(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) {
  if should_send_mouse_synchronously(message) {
    let mut result = 0usize;
    unsafe {
      let _ = SendMessageTimeoutW(
        hwnd,
        message,
        wparam,
        lparam,
        SMTO_ABORTIFHUNG | SMTO_BLOCK,
        60,
        Some(&mut result),
      );
    }
    return;
  }

  unsafe {
    let _ = PostMessageW(hwnd, message, wparam, lparam);
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

fn is_button_down_message(message: u32) -> bool {
  matches!(message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
}

fn is_button_up_message(message: u32) -> bool {
  matches!(message, WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP)
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

fn button_mask(button: MouseButton) -> u32 {
  match button {
    MouseButton::Left => 1,
    MouseButton::Right => 2,
    MouseButton::Middle => 4,
  }
}

#[derive(Clone, Copy)]
enum MouseButton {
  Left,
  Right,
  Middle,
}





