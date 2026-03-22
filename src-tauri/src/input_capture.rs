#![allow(dead_code)]

use crate::coordinate_mapper::NormalizedPoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
  Left,
  Right,
  Middle,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CapturedMouseEvent {
  ButtonDown {
    button: MouseButton,
    point: NormalizedPoint,
  },
  ButtonUp {
    button: MouseButton,
    point: NormalizedPoint,
  },
  Move {
    point: NormalizedPoint,
  },
  Wheel {
    delta_x: i32,
    delta_y: i32,
    point: NormalizedPoint,
  },
}

#[derive(Debug, Default)]
pub struct CaptureState {
  pub is_running: bool,
  pub active_button: Option<MouseButton>,
}
