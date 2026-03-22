mod sync_runtime;

use std::ffi::{c_void, OsStr};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use png::{BitDepth, ColorType, Encoder};
use windows::core::{Error as WindowsError, PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
  CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
  GetMonitorInfoW, GetObjectW, MonitorFromWindow, SelectObject, BITMAP, BITMAPINFO,
  BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HDC, HGDIOBJ, HMONITOR,
  MONITOR_DEFAULTTONEAREST, MONITORINFOEXW,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::System::Threading::{
  GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_NAME_FORMAT,
  PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::HiDpi::{
  GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
  MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON};
use windows::Win32::UI::WindowsAndMessaging::{
  DestroyIcon, DrawIconEx, EnumWindows, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
  GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
  SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST, SWP_NOMOVE,
  SWP_NOSIZE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_SHOWWINDOW, SW_RESTORE, WS_EX_TOOLWINDOW,
};

use crate::models::{Bounds, MonitorInfo, PermissionStatus, WindowInfo};

pub use sync_runtime::{start_input_sync, stop_input_sync};

pub fn initialize() -> Result<()> {
  unsafe {
    let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
  }
  Ok(())
}

pub fn scan_windows() -> Result<Vec<WindowInfo>> {
  let mut windows = Vec::new();
  unsafe {
    EnumWindows(
      Some(enum_windows_proc),
      LPARAM((&mut windows as *mut Vec<WindowInfo>) as isize),
    )?;
  }
  windows.sort_by(|left: &WindowInfo, right: &WindowInfo| {
    left.title.to_lowercase().cmp(&right.title.to_lowercase())
  });
  Ok(windows)
}

pub fn list_monitors() -> Result<Vec<MonitorInfo>> {
  let mut monitors = Vec::new();
  unsafe {
    ensure_bool(
      EnumDisplayMonitors(
        HDC(null_mut()),
        None,
        Some(enum_monitors_proc),
        LPARAM((&mut monitors as *mut Vec<MonitorInfo>) as isize),
      ),
      "EnumDisplayMonitors",
    )?;
  }
  monitors.sort_by(|left: &MonitorInfo, right: &MonitorInfo| {
    right
      .is_primary
      .cmp(&left.is_primary)
      .then_with(|| left.name.cmp(&right.name))
  });
  Ok(monitors)
}

pub fn move_resize_window(window_id: &str, bounds: &Bounds) -> Result<()> {
  let hwnd = parse_hwnd(window_id)?;

  if unsafe { IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() } {
    unsafe {
      let _ = ShowWindow(hwnd, SW_RESTORE);
    }
  }

  unsafe {
    SetWindowPos(
      hwnd,
      HWND_TOPMOST,
      bounds.x,
      bounds.y,
      bounds.width.max(1),
      bounds.height.max(1),
      SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    )?;

    SetWindowPos(
      hwnd,
      HWND_NOTOPMOST,
      bounds.x,
      bounds.y,
      bounds.width.max(1),
      bounds.height.max(1),
      SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    )?;

    SetWindowPos(
      hwnd,
      HWND_TOP,
      bounds.x,
      bounds.y,
      bounds.width.max(1),
      bounds.height.max(1),
      SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    )?;
  }

  Ok(())
}

pub fn bring_window_to_front(window_id: &str) -> Result<()> {
  let hwnd = parse_hwnd(window_id)?;

  if unsafe { IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() } {
    unsafe {
      let _ = ShowWindow(hwnd, SW_RESTORE);
    }
  }

  unsafe {
    SetWindowPos(
      hwnd,
      HWND_TOPMOST,
      0,
      0,
      0,
      0,
      SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    )?;

    SetWindowPos(
      hwnd,
      HWND_TOP,
      0,
      0,
      0,
      0,
      SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    )?;

    SetWindowPos(
      hwnd,
      HWND_NOTOPMOST,
      0,
      0,
      0,
      0,
      SWP_SHOWWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
    )?;
  }

  Ok(())
}

#[allow(dead_code)]
pub fn window_exists(window_id: &str) -> bool {
  parse_hwnd(window_id)
    .map(|hwnd| unsafe { IsWindow(hwnd).as_bool() })
    .unwrap_or(false)
}

pub fn permission_status() -> Result<PermissionStatus> {
  let elevated = is_current_process_elevated()?;
  let mut warnings = vec![
    "Window arrangement and best-effort background input mirroring are enabled on Windows.".to_string(),
    "Mouse click, drag, wheel, and keyboard sync now use low-level hooks plus window-message dispatch; compatibility still varies across Chromium/Electron apps and custom renderers.".to_string(),
    "IME, dead keys, elevated app mismatches, and some anti-cheat or game surfaces can still reject mirrored keyboard input.".to_string(),
  ];

  if elevated {
    warnings.push(
      "This control panel is currently elevated. Background mirroring will only reach elevated target apps unless the app is restarted without elevation.".to_string(),
    );
  }

  Ok(PermissionStatus {
    platform: "windows".to_string(),
    window_management_supported: true,
    input_capture_supported: true,
    input_dispatch_supported: true,
    is_process_elevated: elevated,
    warnings,
  })
}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
  let windows = &mut *(lparam.0 as *mut Vec<WindowInfo>);
  if let Some(window) = describe_window(hwnd) {
    windows.push(window);
  }
  BOOL(1)
}

unsafe extern "system" fn enum_monitors_proc(
  monitor: HMONITOR,
  _hdc: HDC,
  _clip_rect: *mut RECT,
  lparam: LPARAM,
) -> BOOL {
  let monitors = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
  if let Ok(info) = describe_monitor(monitor) {
    monitors.push(info);
  }
  BOOL(1)
}

fn describe_window(hwnd: HWND) -> Option<WindowInfo> {
  if !is_candidate_window(hwnd) {
    return None;
  }

  let title = read_window_text(hwnd)?;
  let pid = window_process_id(hwnd);
  let rect = read_window_rect(hwnd).ok()?;
  let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
  let process_path = read_process_path(pid).ok();
  let process_name = process_path.as_deref().and_then(file_name_from_path);
  if should_exclude_window(&title, process_name.as_deref()) {
    return None;
  }
  let icon_data_url = process_path
    .as_deref()
    .and_then(read_file_icon_data_url);

  Some(WindowInfo {
    id: format_handle(hwnd.0),
    native_handle: hwnd.0 as usize as i64,
    title,
    process_name,
    icon_data_url,
    pid,
    monitor_id: (!monitor.0.is_null()).then(|| format_handle(monitor.0)),
    rect,
    is_visible: unsafe { IsWindowVisible(hwnd).as_bool() },
    is_minimized: unsafe { IsIconic(hwnd).as_bool() },
  })
}

fn describe_monitor(monitor: HMONITOR) -> Result<MonitorInfo> {
  let mut info = MONITORINFOEXW::default();
  info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;

  unsafe {
    ensure_bool(
      GetMonitorInfoW(monitor, &mut info as *mut MONITORINFOEXW as *mut _),
      "GetMonitorInfoW",
    )?;
  }

  Ok(MonitorInfo {
    id: format_handle(monitor.0),
    name: utf16_buffer_to_string(&info.szDevice),
    rect: bounds_from_rect(&info.monitorInfo.rcMonitor),
    work_area: bounds_from_rect(&info.monitorInfo.rcWork),
    is_primary: info.monitorInfo.dwFlags != 0,
    scale_factor: monitor_scale_factor(monitor).unwrap_or(1.0),
  })
}

fn is_candidate_window(hwnd: HWND) -> bool {
  if !unsafe { IsWindow(hwnd).as_bool() } || !unsafe { IsWindowVisible(hwnd).as_bool() } {
    return false;
  }

  if unsafe { GetWindowTextLengthW(hwnd) } <= 0 {
    return false;
  }

  if window_process_id(hwnd) == unsafe { GetCurrentProcessId() } {
    return false;
  }

  let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
  if ex_style & WS_EX_TOOLWINDOW.0 != 0 {
    return false;
  }

  !is_cloaked(hwnd)
}

fn is_cloaked(hwnd: HWND) -> bool {
  let mut cloaked = 0u32;
  unsafe {
    let _ = DwmGetWindowAttribute(
      hwnd,
      DWMWA_CLOAKED,
      &mut cloaked as *mut u32 as *mut _,
      size_of::<u32>() as u32,
    );
  }
  cloaked != 0
}

fn should_exclude_window(title: &str, process_name: Option<&str>) -> bool {
  let normalized_title = title.trim().to_ascii_lowercase();
  if normalized_title == "mirror windows" || normalized_title.starts_with("mirror windows ") {
    return true;
  }

  let Some(process_name) = process_name else {
    return false;
  };

  matches!(
    process_name.trim().to_ascii_lowercase().as_str(),
    "windowsterminal.exe"
      | "powershell.exe"
      | "pwsh.exe"
      | "cmd.exe"
      | "conhost.exe"
      | "wezterm.exe"
      | "alacritty.exe"
      | "mintty.exe"
  )
}

fn read_window_text(hwnd: HWND) -> Option<String> {
  let length = unsafe { GetWindowTextLengthW(hwnd) };
  if length <= 0 {
    return None;
  }

  let mut buffer = vec![0u16; length as usize + 1];
  let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
  if copied <= 0 {
    return None;
  }

  let value = String::from_utf16_lossy(&buffer[..copied as usize]);
  let trimmed = value.trim();
  if trimmed.is_empty() {
    None
  } else {
    Some(trimmed.to_string())
  }
}

fn read_window_rect(hwnd: HWND) -> Result<Bounds> {
  let mut rect = RECT::default();
  unsafe {
    GetWindowRect(hwnd, &mut rect)?;
  }
  Ok(bounds_from_rect(&rect))
}

fn read_process_path(pid: u32) -> Result<String> {
  let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
    .with_context(|| format!("Failed to open process {pid}"))?;
  let result = read_process_path_from_handle(handle);
  unsafe {
    let _ = CloseHandle(handle);
  }
  result
}

fn read_process_path_from_handle(handle: HANDLE) -> Result<String> {
  let mut buffer = vec![0u16; 32768];
  let mut buffer_len = buffer.len() as u32;

  unsafe {
    QueryFullProcessImageNameW(
      handle,
      PROCESS_NAME_FORMAT(0),
      PWSTR(buffer.as_mut_ptr()),
      &mut buffer_len,
    )?;
  }

  Ok(String::from_utf16_lossy(&buffer[..buffer_len as usize]))
}

fn file_name_from_path(raw_path: &str) -> Option<String> {
  Path::new(raw_path.trim())
    .file_name()
    .and_then(|name| name.to_str())
    .map(|name| name.to_string())
}

fn read_file_icon_data_url(raw_path: &str) -> Option<String> {
  let wide_path = wide_null(raw_path.trim());
  let mut file_info = SHFILEINFOW::default();
  let result = unsafe {
    SHGetFileInfoW(
      PCWSTR(wide_path.as_ptr()),
      FILE_FLAGS_AND_ATTRIBUTES(0),
      Some(&mut file_info),
      size_of::<SHFILEINFOW>() as u32,
      SHGFI_ICON | SHGFI_SMALLICON,
    )
  };

  if result == 0 || file_info.hIcon.0.is_null() {
    return None;
  }

  let icon = file_info.hIcon;
  let encoded = icon_to_data_url(icon);
  unsafe {
    let _ = DestroyIcon(icon);
  }
  encoded
}

fn icon_to_data_url(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<String> {
  let mut icon_info = windows::Win32::UI::WindowsAndMessaging::ICONINFO::default();
  unsafe { windows::Win32::UI::WindowsAndMessaging::GetIconInfo(icon, &mut icon_info).ok()?; }

  let source_bitmap = if !icon_info.hbmColor.0.is_null() {
    icon_info.hbmColor
  } else {
    icon_info.hbmMask
  };

  let mut bitmap = BITMAP::default();
  let get_object_result = unsafe {
    GetObjectW(
      source_bitmap,
      size_of::<BITMAP>() as i32,
      Some(&mut bitmap as *mut BITMAP as *mut c_void),
    )
  };
  if get_object_result == 0 {
    unsafe {
      let _ = DeleteObject(icon_info.hbmColor);
      let _ = DeleteObject(icon_info.hbmMask);
    }
    return None;
  }

  let width = bitmap.bmWidth.max(1);
  let mut height = bitmap.bmHeight.max(1);
  if icon_info.hbmColor.0.is_null() {
    height = (height / 2).max(1);
  }

  let screen_dc = unsafe { windows::Win32::Graphics::Gdi::GetDC(HWND(null_mut())) };
  if screen_dc.0.is_null() {
    unsafe {
      let _ = DeleteObject(icon_info.hbmColor);
      let _ = DeleteObject(icon_info.hbmMask);
    }
    return None;
  }

  let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
  if memory_dc.0.is_null() {
    unsafe {
      let _ = windows::Win32::Graphics::Gdi::ReleaseDC(HWND(null_mut()), screen_dc);
      let _ = DeleteObject(icon_info.hbmColor);
      let _ = DeleteObject(icon_info.hbmMask);
    }
    return None;
  }
  let mut bmi = BITMAPINFO::default();
  bmi.bmiHeader = BITMAPINFOHEADER {
    biSize: size_of::<BITMAPINFOHEADER>() as u32,
    biWidth: width,
    biHeight: -height,
    biPlanes: 1,
    biBitCount: 32,
    biCompression: BI_RGB.0,
    ..Default::default()
  };

  let mut bits = null_mut();
  let dib = unsafe { CreateDIBSection(screen_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) }.ok()?;
  let old_bitmap = unsafe { SelectObject(memory_dc, HGDIOBJ(dib.0)) };
  let draw_result = unsafe { DrawIconEx(memory_dc, 0, 0, icon, width, height, 0, None, windows::Win32::UI::WindowsAndMessaging::DI_NORMAL) };

  let encoded = if draw_result.is_ok() && !bits.is_null() {
    let pixel_count = width as usize * height as usize;
    let bgra = unsafe { std::slice::from_raw_parts(bits as *const u8, pixel_count * 4) };
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for chunk in bgra.chunks_exact(4) {
      rgba.push(chunk[2]);
      rgba.push(chunk[1]);
      rgba.push(chunk[0]);
      rgba.push(chunk[3]);
    }

    let mut png_bytes = Vec::new();
    let mut encoder = Encoder::new(&mut png_bytes, width as u32, height as u32);
    encoder.set_color(ColorType::Rgba);
    encoder.set_depth(BitDepth::Eight);
    {
      let mut writer = encoder.write_header().ok()?;
      writer.write_image_data(&rgba).ok()?;
    }
    Some(format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png_bytes)))
  } else {
    None
  };

  unsafe {
    let _ = SelectObject(memory_dc, old_bitmap);
    let _ = DeleteObject(dib);
    let _ = DeleteDC(memory_dc);
    let _ = windows::Win32::Graphics::Gdi::ReleaseDC(HWND(null_mut()), screen_dc);
    let _ = DeleteObject(icon_info.hbmColor);
    let _ = DeleteObject(icon_info.hbmMask);
  }

  encoded
}

fn window_process_id(hwnd: HWND) -> u32 {
  unsafe { GetWindowThreadProcessId(hwnd, None) }
}

fn monitor_scale_factor(monitor: HMONITOR) -> Result<f64> {
  let mut dpi_x = 96u32;
  let mut dpi_y = 96u32;
  unsafe {
    GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y)?;
  }
  Ok(dpi_x as f64 / 96.0)
}

fn is_current_process_elevated() -> Result<bool> {
  let mut token = HANDLE::default();
  unsafe {
    OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
  }

  let mut elevation = TOKEN_ELEVATION::default();
  let mut bytes_returned = 0u32;
  unsafe {
    GetTokenInformation(
      token,
      TokenElevation,
      Some(&mut elevation as *mut TOKEN_ELEVATION as *mut _),
      size_of::<TOKEN_ELEVATION>() as u32,
      &mut bytes_returned,
    )?;
    let _ = CloseHandle(token);
  }

  Ok(elevation.TokenIsElevated != 0)
}

fn bounds_from_rect(rect: &RECT) -> Bounds {
  Bounds {
    x: rect.left,
    y: rect.top,
    width: rect.right - rect.left,
    height: rect.bottom - rect.top,
  }
}

fn utf16_buffer_to_string(buffer: &[u16]) -> String {
  let len = buffer.iter().position(|value| *value == 0).unwrap_or(buffer.len());
  String::from_utf16_lossy(&buffer[..len]).trim().to_string()
}

fn format_handle(handle: *mut c_void) -> String {
  format!("0x{:X}", handle as usize)
}

fn wide_null(value: &str) -> Vec<u16> {
  OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

pub(crate) fn parse_hwnd(window_id: &str) -> Result<HWND> {
  let trimmed = window_id.trim();
  let hex = trimmed
    .strip_prefix("0x")
    .or_else(|| trimmed.strip_prefix("0X"))
    .unwrap_or(trimmed);
  let raw = usize::from_str_radix(hex, 16)
    .map_err(|error| anyhow!("Invalid native window handle '{window_id}': {error}"))?;
  Ok(HWND(raw as *mut c_void))
}

pub(crate) fn ensure_bool(value: BOOL, context: &str) -> Result<()> {
  if value.as_bool() {
    Ok(())
  } else {
    Err(anyhow!("{context} failed: {}", WindowsError::from_win32()))
  }
}




