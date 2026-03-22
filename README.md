# Mirror Windows

Windows-first desktop control panel for arranging and synchronizing up to twenty external application windows.

## Current scope

- Tauri 2 shell with React + Vite frontend
- Real Windows window discovery via `EnumWindows`
- Real monitor discovery via `EnumDisplayMonitors`
- Real tile / stack arrangement via `SetWindowPos`
- Live simplified Windows click sync via low-level mouse hooks plus background window-message dispatch
- Session state for primary window, targets, monitor, and layout mode
- JSON profile storage
- Permission / integrity-level warnings surfaced in the UI

## Project structure

- `src/`: React control panel
- `src-tauri/src/platform/windows/`: Win32 adapter layer and sync runtime
- `src-tauri/src/layout_engine.rs`: tile / stack calculations
- `src-tauri/src/sync_session.rs`: runtime session state and validation
- `docs/architecture.md`: native architecture and boundaries
- `docs/implementation-plan.md`: phased task list for hardening and compatibility follow-up

## Run locally

1. Install frontend dependencies:

```powershell
cmd /c npm install
```

2. Start the app:

```powershell
cargo tauri dev
```

If PowerShell execution policy blocks `npm.ps1`, keep using `cmd /c npm ...` from this workspace.

## Honest status

This repo now supports the full Windows-first control flow:

- discovery
- selection
- layout
- profile persistence
- start/stop live sync
- best-effort mouse, wheel, and keyboard mirroring

The important caveat is app compatibility, not missing code. The current engine uses background window-message dispatch, so behavior can still vary by app family:

- classic Win32 apps: usually the best fit
- Chromium/Electron apps: mixed, depends on which child window actually consumes input
- games / custom renderers / elevated targets: often unreliable or blocked
- IME, dead keys, and some international keyboard paths: still best-effort only

The remaining spike work is about measuring and hardening those cases, not about bootstrapping the feature from zero.
