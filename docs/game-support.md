# Hỗ trợ Game

## Tổng quan

Project hiện đã hỗ trợ đồng bộ input với game thông qua **SendInput API**. Đây là giải pháp mô phỏng input ở tầng driver, được game chấp nhận như input thật từ hardware.

## Cách hoạt động

### 2 phương pháp dispatch

1. **WindowMessage Mode** (mặc định cho browser/ứng dụng thông thường)
   - Sử dụng `PostMessage` và `SendMessageTimeoutW`
   - Hoạt động tốt với: Chrome, Firefox, Edge, Electron apps, Notepad, Explorer
   - Không hoạt động với game vì game đọc input trực tiếp từ hardware

2. **SendInput Mode** (cho game)
   - Sử dụng `SendInput` API - mô phỏng input ở tầng driver
   - Hoạt động tốt với: Unity games, Unreal Engine games, DirectX/OpenGL apps
   - Yêu cầu window phải được focus trước khi gửi input

### Auto Detection

Hệ thống tự động phát hiện loại ứng dụng dựa trên:
- **Class name** của window
- **Process name** của ứng dụng

**Game indicators** (sẽ dùng SendInput):
- Unity games: `unityplayer`
- Unreal Engine: `ue4`, `ue5`, `unreal`
- Game launchers: `steam`, `epic`, `origin`, `uplay`, `battlenet`
- DirectX/OpenGL/Vulkan windows

**Browser indicators** (sẽ dùng WindowMessage):
- Browsers: `chrome`, `firefox`, `edge`, `brave`, `opera`
- Apps: `electron`, `notepad`, `explorer`, `vscode`

## Cấu hình

### Trong SessionConfig

```typescript
{
  dispatch_mode: "auto" | "window_message" | "send_input"
}
```

- `"auto"` (mặc định): Tự động phát hiện
- `"window_message"`: Bắt buộc dùng PostMessage/SendMessage
- `"send_input"`: Bắt buộc dùng SendInput API

### Trong App.tsx

Mặc định đã được cấu hình:

```typescript
function buildConfig(): SessionConfig {
  return {
    // ...
    dispatch_mode: "auto",
    sync_mouse_move: true,
    sync_wheel: true,
    sync_keyboard: false,
  };
}
```

## Lưu ý quan trọng

### Với Game

1. **Window phải được focus**: SendInput yêu cầu target window phải ở foreground
2. **Anti-cheat**: Một số game có anti-cheat mạnh có thể vẫn chặn SendInput
3. **Exclusive fullscreen**: Game ở chế độ exclusive fullscreen có thể không nhận input
4. **Giải pháp**: Chạy game ở chế độ windowed hoặc borderless windowed

### Với Browser

1. Browser vẫn dùng WindowMessage mode (hiệu quả hơn)
2. Không cần focus window
3. Hoạt động tốt ở background

## Kiểm tra

### Test với game

1. Mở một game (ví dụ: Unity game, Steam game)
2. Chọn làm target window trong Mirror Windows
3. Click vào primary window
4. Kiểm tra xem game có nhận được click không

### Debug

Nếu game không nhận input:
1. Kiểm tra game có đang ở windowed mode không
2. Thử set `dispatch_mode: "send_input"` thủ công
3. Kiểm tra log để xem dispatch mode nào được chọn
4. Một số game cần quyền admin - chạy Mirror Windows với quyền admin

## Kỹ thuật

### SendInput API

```rust
pub fn dispatch_mouse(
    target_hwnd: HWND,
    normalized_point: NormalizedPoint,
    target_bounds: &Bounds,
    message: u32,
    mouse_data: u32,
) -> Result<()>
```

**Quy trình:**
1. Chuyển đổi normalized coordinates → client coordinates
2. Chuyển đổi client coordinates → screen coordinates
3. Chuyển đổi screen coordinates → absolute coordinates (0-65535)
4. Focus target window với `SetForegroundWindow`
5. Gửi mouse move event
6. Gửi button/wheel event
7. Sử dụng `SendInput` để inject input

### Coordinate Conversion

```
Normalized (0.0-1.0) 
  → Client (pixel trong window)
  → Screen (pixel trên màn hình)
  → Absolute (0-65535 cho SendInput)
```

## Tương lai

Các cải tiến có thể:
- Hỗ trợ keyboard input cho game
- Tối ưu focus management
- Hỗ trợ Raw Input API cho game hardcore
- Bypass một số anti-cheat systems (nếu hợp pháp)
