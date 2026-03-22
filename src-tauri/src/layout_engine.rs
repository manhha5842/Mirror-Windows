use std::collections::HashSet;

use anyhow::{anyhow, Result};

use crate::models::{Bounds, LayoutMode, LayoutPreview, MonitorInfo, SessionConfig};
use crate::{platform, window_registry};

pub fn apply_layout(config: &SessionConfig) -> Result<Vec<LayoutPreview>> {
  let preview = preview_layout(config)?;
  for item in &preview {
    platform::move_resize_window(&item.window_id, &item.bounds)?;
  }
  Ok(preview)
}

pub fn preview_layout(config: &SessionConfig) -> Result<Vec<LayoutPreview>> {
  let monitor = resolve_monitor(config)?;
  let managed_windows = layout_window_ids(config);
  if managed_windows.is_empty() {
    return Err(anyhow!("Select at least one managed window before applying layout."));
  }

  let slots = compute_layout_slots(config.layout_mode, &monitor.work_area, managed_windows.len());
  Ok(
    managed_windows
      .into_iter()
      .zip(slots)
      .map(|(window_id, bounds)| LayoutPreview { window_id, bounds })
      .collect(),
  )
}

fn resolve_monitor(config: &SessionConfig) -> Result<MonitorInfo> {
  let monitors = platform::list_monitors()?;
  if monitors.is_empty() {
    return Err(anyhow!("No monitors were detected on this system."));
  }

  if let Some(monitor_id) = &config.monitor_id {
    if let Some(monitor) = monitors.iter().find(|monitor| monitor.id == *monitor_id) {
      return Ok(monitor.clone());
    }
  }

  let windows = window_registry::scan_windows()?;
  if let Some(primary_window) = windows
    .iter()
    .find(|window| window.id == config.primary_window_id)
  {
    if let Some(monitor_id) = &primary_window.monitor_id {
      if let Some(monitor) = monitors.iter().find(|monitor| monitor.id == *monitor_id) {
        return Ok(monitor.clone());
      }
    }
  }

  monitors
    .iter()
    .find(|monitor| monitor.is_primary)
    .cloned()
    .or_else(|| monitors.first().cloned())
    .ok_or_else(|| anyhow!("No suitable monitor was found for this session."))
}

pub fn managed_window_ids(config: &SessionConfig) -> Vec<String> {
  let mut seen = HashSet::new();
  let mut managed = Vec::new();
  for window_id in std::iter::once(&config.primary_window_id).chain(config.target_window_ids.iter()) {
    if seen.insert(window_id.clone()) {
      managed.push(window_id.clone());
    }
  }
  managed
}

fn layout_window_ids(config: &SessionConfig) -> Vec<String> {
  let managed = managed_window_ids(config);
  if config.layout_mode != LayoutMode::Stack || managed.len() <= 1 {
    return managed;
  }

  managed
    .into_iter()
    .filter(|window_id| window_id != &config.primary_window_id)
    .chain(std::iter::once(config.primary_window_id.clone()))
    .collect()
}

fn compute_layout_slots(layout_mode: LayoutMode, area: &Bounds, count: usize) -> Vec<Bounds> {
  match layout_mode {
    LayoutMode::Tile => tile_slots(area, count),
    LayoutMode::Stack => stack_slots(area, count),
  }
}

fn tile_slots(area: &Bounds, count: usize) -> Vec<Bounds> {
  if count == 0 {
    return Vec::new();
  }

  let cols = (f64::sqrt(count as f64).ceil() as usize).max(1);
  let rows = ((count + cols - 1) / cols).max(1);
  let mut slots = Vec::with_capacity(count);

  for index in 0..count {
    let row = index / cols;
    let col = index % cols;

    let x = area.x + ((col as i32 * area.width) / cols as i32);
    let next_x = area.x + (((col + 1) as i32 * area.width) / cols as i32);
    let y = area.y + ((row as i32 * area.height) / rows as i32);
    let next_y = area.y + (((row + 1) as i32 * area.height) / rows as i32);

    slots.push(Bounds {
      x,
      y,
      width: (next_x - x).max(1),
      height: (next_y - y).max(1),
    });
  }

  slots
}

fn stack_slots(area: &Bounds, count: usize) -> Vec<Bounds> {
  if count == 0 {
    return Vec::new();
  }

  if count == 1 {
    return vec![area.clone()];
  }

  let (offset_ratio_x, offset_ratio_y, min_width_ratio, min_height_ratio, min_step_x, min_step_y) =
    match count {
      2..=4 => (0.28, 0.24, 0.72, 0.74, 34, 28),
      5..=8 => (0.26, 0.22, 0.68, 0.70, 24, 20),
      9..=12 => (0.23, 0.20, 0.63, 0.66, 18, 15),
      _ => (0.18, 0.16, 0.58, 0.60, 12, 10),
    };

  let max_offset_x = ((area.width as f64) * offset_ratio_x).round() as i32;
  let max_offset_y = ((area.height as f64) * offset_ratio_y).round() as i32;
  let step_x = (max_offset_x / (count as i32 - 1)).max(min_step_x);
  let step_y = (max_offset_y / (count as i32 - 1)).max(min_step_y);

  let final_offset_x = step_x * (count as i32 - 1);
  let final_offset_y = step_y * (count as i32 - 1);
  let min_width = ((area.width as f64) * min_width_ratio).round() as i32;
  let min_height = ((area.height as f64) * min_height_ratio).round() as i32;
  let width = (area.width - final_offset_x).max(min_width).max(320);
  let height = (area.height - final_offset_y).max(min_height).max(220);

  (0..count)
    .map(|index| Bounds {
      x: area.x + step_x * index as i32,
      y: area.y + step_y * index as i32,
      width: width.min(area.width).max(1),
      height: height.min(area.height).max(1),
    })
    .collect()
}
