use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, SetCursorPos, SetForegroundWindow,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::coordinate_mapper::{project_to_client, NormalizedPoint};
use crate::models::Bounds;

pub struct GameInputDispatcher;

const DEFAULT_FOCUS_SETTLE_MS: u64 = 65;
const DEFAULT_CLICK_HOLD_MS: u64 = 28;
const GAME_MODE_FOCUS_SETTLE_MS: u64 = 120;
const GAME_MODE_CLICK_HOLD_MS: u64 = 52;
const GAME_MODE_MOVE_SETTLE_MS: u64 = 22;
const GAME_MODE_POST_CLICK_SETTLE_MS: u64 = 36;

impl GameInputDispatcher {
    pub fn dispatch_gesture(
        target_hwnd: HWND,
        points: &[NormalizedPoint],
        target_bounds: &Bounds,
        down_message: u32,
        up_message: u32,
        game_mode: bool,
    ) -> Result<()> {
        let _cursor_guard = CursorRestoreGuard::capture();
        let Some(first_point) = points.first().copied() else {
            return Err(anyhow!("Buffered gesture had no points"));
        };

        focus_target(target_hwnd, game_mode)?;
        dispatch_move(target_hwnd, first_point, target_bounds)?;
        thread::sleep(Duration::from_millis(GAME_MODE_MOVE_SETTLE_MS));
        dispatch_button(target_hwnd, first_point, target_bounds, down_message)?;
        thread::sleep(Duration::from_millis(click_hold_ms(game_mode)));

        for point in points.iter().copied().skip(1).take(points.len().saturating_sub(2)) {
            dispatch_move(target_hwnd, point, target_bounds)?;
            thread::sleep(Duration::from_millis(GAME_MODE_MOVE_SETTLE_MS));
        }

        let last_point = points.last().copied().unwrap_or(first_point);
        dispatch_move(target_hwnd, last_point, target_bounds)?;
        thread::sleep(Duration::from_millis(GAME_MODE_MOVE_SETTLE_MS));
        dispatch_button(target_hwnd, last_point, target_bounds, up_message)?;
        thread::sleep(Duration::from_millis(GAME_MODE_POST_CLICK_SETTLE_MS));

        Ok(())
    }

    pub fn dispatch_mouse(
        target_hwnd: HWND,
        normalized_point: NormalizedPoint,
        target_bounds: &Bounds,
        message: u32,
        mouse_data: u32,
        game_mode: bool,
    ) -> Result<()> {
        let _cursor_guard = CursorRestoreGuard::capture();
        focus_target(target_hwnd, game_mode)?;

        let (target_x, target_y) = project_to_client(normalized_point, target_bounds);
        let absolute_coords = client_to_absolute(target_hwnd, POINT { x: target_x, y: target_y })?;

        let mut inputs = Vec::new();
        inputs.push(create_mouse_move_input(absolute_coords.0, absolute_coords.1));

        match message {
            0x0201 => inputs.push(create_mouse_button_input(MOUSEEVENTF_LEFTDOWN.0, absolute_coords.0, absolute_coords.1)),
            0x0202 => inputs.push(create_mouse_button_input(MOUSEEVENTF_LEFTUP.0, absolute_coords.0, absolute_coords.1)),
            0x0204 => inputs.push(create_mouse_button_input(MOUSEEVENTF_RIGHTDOWN.0, absolute_coords.0, absolute_coords.1)),
            0x0205 => inputs.push(create_mouse_button_input(MOUSEEVENTF_RIGHTUP.0, absolute_coords.0, absolute_coords.1)),
            0x0207 => inputs.push(create_mouse_button_input(MOUSEEVENTF_MIDDLEDOWN.0, absolute_coords.0, absolute_coords.1)),
            0x0208 => inputs.push(create_mouse_button_input(MOUSEEVENTF_MIDDLEUP.0, absolute_coords.0, absolute_coords.1)),
            0x020A => {
                let delta = hiword(mouse_data as usize) as i16;
                inputs.push(create_mouse_wheel_input(delta as i32, absolute_coords.0, absolute_coords.1, false));
            }
            0x020E => {
                let delta = hiword(mouse_data as usize) as i16;
                inputs.push(create_mouse_wheel_input(delta as i32, absolute_coords.0, absolute_coords.1, true));
            }
            0x0200 => {}
            _ => {}
        }

        if !inputs.is_empty() {
            send_inputs(&inputs)?;
        }

        if is_button_down_message(message) {
            thread::sleep(Duration::from_millis(click_hold_ms(game_mode)));
        }

        Ok(())
    }
}

struct CursorRestoreGuard {
    original_cursor_pos: Option<POINT>,
}

impl CursorRestoreGuard {
    fn capture() -> Self {
        Self {
            original_cursor_pos: current_cursor_pos(),
        }
    }
}

impl Drop for CursorRestoreGuard {
    fn drop(&mut self) {
        restore_cursor_pos(self.original_cursor_pos);
    }
}

fn focus_target(target_hwnd: HWND, game_mode: bool) -> Result<()> {
    let settle_ms = if game_mode {
        GAME_MODE_FOCUS_SETTLE_MS
    } else {
        DEFAULT_FOCUS_SETTLE_MS
    };

    for attempt in 0..2 {
        unsafe {
            let _ = SetForegroundWindow(target_hwnd);
        }
        thread::sleep(Duration::from_millis(settle_ms));

        if unsafe { GetForegroundWindow() } == target_hwnd {
            return Ok(());
        }

        if attempt == 0 {
            thread::sleep(Duration::from_millis(24));
        }
    }

    Err(anyhow!(
        "Failed to focus target window before SendInput replay"
    ))
}

fn click_hold_ms(game_mode: bool) -> u64 {
    if game_mode {
        GAME_MODE_CLICK_HOLD_MS
    } else {
        DEFAULT_CLICK_HOLD_MS
    }
}

fn client_to_absolute(target_hwnd: HWND, client_point: POINT) -> Result<(i32, i32)> {
    let mut screen_point = client_point;
    unsafe {
        if !ClientToScreen(target_hwnd, &mut screen_point).as_bool() {
            return Err(anyhow!("Failed to convert client to screen coordinates"));
        }
    }
    screen_to_absolute(screen_point)
}

fn dispatch_move(target_hwnd: HWND, normalized_point: NormalizedPoint, target_bounds: &Bounds) -> Result<()> {
    let (target_x, target_y) = project_to_client(normalized_point, target_bounds);
    let absolute_coords = client_to_absolute(target_hwnd, POINT { x: target_x, y: target_y })?;
    send_inputs(&[create_mouse_move_input(absolute_coords.0, absolute_coords.1)])
}

fn dispatch_button(
    target_hwnd: HWND,
    normalized_point: NormalizedPoint,
    target_bounds: &Bounds,
    message: u32,
) -> Result<()> {
    let (target_x, target_y) = project_to_client(normalized_point, target_bounds);
    let absolute_coords = client_to_absolute(target_hwnd, POINT { x: target_x, y: target_y })?;
    let flag = match message {
        0x0201 => MOUSEEVENTF_LEFTDOWN.0,
        0x0202 => MOUSEEVENTF_LEFTUP.0,
        0x0204 => MOUSEEVENTF_RIGHTDOWN.0,
        0x0205 => MOUSEEVENTF_RIGHTUP.0,
        0x0207 => MOUSEEVENTF_MIDDLEDOWN.0,
        0x0208 => MOUSEEVENTF_MIDDLEUP.0,
        _ => return Err(anyhow!("Unsupported button message for gesture replay: {message}")),
    };

    send_inputs(&[create_mouse_button_input(flag, absolute_coords.0, absolute_coords.1)])
}

fn send_inputs(inputs: &[INPUT]) -> Result<()> {
    unsafe {
        let sent = SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
        if sent != inputs.len() as u32 {
            return Err(anyhow!("Failed to send all input events: sent {}/{}", sent, inputs.len()));
        }
    }
    Ok(())
}

fn screen_to_absolute(screen_point: POINT) -> Result<(i32, i32)> {
    unsafe {
        let virtual_screen_left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let virtual_screen_top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let virtual_screen_width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let virtual_screen_height = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        if virtual_screen_width == 0 || virtual_screen_height == 0 {
            return Err(anyhow!("Invalid virtual screen dimensions"));
        }

        let absolute_x = ((screen_point.x - virtual_screen_left) * 65536) / virtual_screen_width;
        let absolute_y = ((screen_point.y - virtual_screen_top) * 65536) / virtual_screen_height;

        Ok((absolute_x, absolute_y))
    }
}

fn create_mouse_move_input(absolute_x: i32, absolute_y: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: absolute_x,
                dy: absolute_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn create_mouse_button_input(button_flag: u32, absolute_x: i32, absolute_y: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: absolute_x,
                dy: absolute_y,
                mouseData: 0,
                dwFlags: MOUSE_EVENT_FLAGS(button_flag | MOUSEEVENTF_ABSOLUTE.0 | MOUSEEVENTF_VIRTUALDESK.0),
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn create_mouse_wheel_input(delta: i32, absolute_x: i32, absolute_y: i32, horizontal: bool) -> INPUT {
    let wheel_flag = if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: absolute_x,
                dy: absolute_y,
                mouseData: delta as u32,
                dwFlags: wheel_flag | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn hiword(value: usize) -> u16 {
    ((value >> 16) & 0xffff) as u16
}

fn is_button_down_message(message: u32) -> bool {
    matches!(message, 0x0201 | 0x0204 | 0x0207)
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
