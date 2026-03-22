#![allow(dead_code)]

use crate::coordinate_mapper::NormalizedPoint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchTarget {
  pub window_id: String,
  pub client_width: i32,
  pub client_height: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DispatchEvent {
  pub point: NormalizedPoint,
  pub wheel_delta: Option<(i32, i32)>,
}

#[derive(Debug, Default)]
pub struct DispatchPlan {
  pub targets: Vec<DispatchTarget>,
}
