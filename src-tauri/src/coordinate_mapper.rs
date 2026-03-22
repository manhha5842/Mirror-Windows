#![allow(dead_code)]

use crate::models::Bounds;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NormalizedPoint {
  pub rx: f64,
  pub ry: f64,
}

pub fn normalize_client_point(x: i32, y: i32, client_bounds: &Bounds) -> Option<NormalizedPoint> {
  if client_bounds.width <= 0 || client_bounds.height <= 0 {
    return None;
  }

  Some(NormalizedPoint {
    rx: x as f64 / client_bounds.width as f64,
    ry: y as f64 / client_bounds.height as f64,
  })
}

pub fn project_to_client(point: NormalizedPoint, client_bounds: &Bounds) -> (i32, i32) {
  let x = (point.rx.clamp(0.0, 1.0) * client_bounds.width as f64).round() as i32;
  let y = (point.ry.clamp(0.0, 1.0) * client_bounds.height as f64).round() as i32;
  (x, y)
}
