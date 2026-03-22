# Architecture

## Product boundary

The frontend is a control panel, not the sync engine.

- React + Vite owns operator workflow: scanning windows, choosing a primary, choosing monitor/layout, starting or stopping a session, toggling mouse/wheel/keyboard sync, and saving profiles.
- Tauri owns the desktop shell and the Rust command bridge.
- Rust owns every system-facing operation: window enumeration, monitor enumeration, layout, session validation, privilege checks, global input capture, coordinate mapping, and best-effort replay.

## Native module map

### `window_registry`

- Enumerates visible top-level windows.
- Collects `title`, `pid`, native handle, monitor, and bounds.
- Filters out tool windows, cloaked windows, and the Tauri app itself.

### `layout_engine`

- Computes tile and stack placements.
- Resolves the destination monitor work area.
- Applies `SetWindowPos` through the platform adapter.

### `sync_session`

- Validates the operator-selected configuration.
- Enforces the Windows-first MVP limit of 5 managed windows.
- Stores primary window, targets, layout mode, coordinate mode, and running state.
- Starts and stops the live Windows sync runtime.

### `coordinate_mapper`

- Converts primary client coordinates into normalized ratios and projects them into each target client area.
- Keeps click, drag, and wheel replay aligned even when windows have different client sizes.

### `platform/windows/sync_runtime`

- Installs `WH_MOUSE_LL` and `WH_KEYBOARD_LL` hooks on a dedicated message-loop thread.
- Filters events so only the primary window drives the session.
- Resolves target child windows and dispatches `WM_*` messages for mouse and keyboard input.
- Mirrors printable text via `WM_CHAR` as a best-effort path for standard typing.

### `permissions`

- Checks current process elevation on Windows.
- Surfaces the integrity-level warning early because UIPI still matters for replay.

### `profile_store`

- Persists profiles as JSON in the application data directory.

## Platform layout

- `platform/windows/`: implemented now
- `platform/macos/`: reserved
- `platform/linux_x11/`: reserved

The Windows adapter exposes the native surface used by the rest of the app:

- `scan_windows()`
- `list_monitors()`
- `move_resize_window()`
- `permission_status()`
- `start_input_sync()`
- `stop_input_sync()`

## MVP truth

The repo now proves this operational flow on Windows:

- scan real windows
- choose up to five
- choose a primary
- choose a monitor
- tile or stack them
- start and stop live best-effort sync
- mirror mouse, wheel, and keyboard input to background targets
- save and reload the operator setup

What is still unresolved is compatibility depth, not missing architecture:

- Chromium/Electron child-window variance
- IME and dead-key behavior
- game / custom-rendered surface compatibility
- elevated target mismatch behavior

Those are the areas the next hardening spike should measure and document.
