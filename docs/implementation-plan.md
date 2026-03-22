# Implementation Plan

## Completed baseline

Implemented in this repo today:

- Windows-only MVP boundary
- max 5 managed windows
- normalized-client coordinate mode in the data model
- local JSON profiles
- live Windows hook runtime for mouse and keyboard capture
- best-effort background dispatch for click, drag, wheel, keydown/up, and standard text input

## Phase 1: compatibility spike

This is still the highest-value next step.

### Goals

- measure how the current runtime behaves against at least:
  - Notepad or another classic Win32 app
  - Chrome or Edge
  - one Electron app
- document which actions are reliable in foreground vs background mode
- record keyboard caveats for shortcuts, text entry, and IME paths

### Candidate tasks

1. Build a small repeatable test matrix and log each target app class.
2. Verify click, drag, wheel, shortcut keys, and plain text input.
3. Note whether child-window targeting needs app-specific tuning.
4. Capture failures by privilege level and DPI scaling.
5. Decide whether any app classes need an alternate replay path beyond `WM_*` dispatch.

### Deliverable

- a short matrix with app type, foreground/background behavior, mouse reliability, keyboard reliability, and caveats

## Phase 2: hardening

Recommended tasks:

- session restore on launch
- profile delete / rename
- richer runtime events from Rust back into the UI
- better lost-window handling when targets close or minimize
- keyboard-path improvements for IME and dead keys where possible

## Phase 3: quality and ergonomics

- global shortcut start / stop
- target health indicators in the UI
- target compatibility notes per profile
- richer filtering for large window lists

## Phase 4: packaging

- NSIS installer polish
- signed build pipeline
- update story if needed
