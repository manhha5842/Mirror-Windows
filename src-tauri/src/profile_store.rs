use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use tauri::{AppHandle, Manager};

use crate::models::{ProfileDraft, ProfileRecord};
use crate::state::unix_time_ms;

pub fn load_profiles(app: &AppHandle) -> Result<Vec<ProfileRecord>> {
  let path = profiles_path(app)?;
  if !path.exists() {
    return Ok(Vec::new());
  }

  let raw = fs::read_to_string(&path).with_context(|| format!("Failed to read {}", path.display()))?;
  if raw.trim().is_empty() {
    return Ok(Vec::new());
  }

  let mut profiles = serde_json::from_str::<Vec<ProfileRecord>>(&raw)
    .with_context(|| format!("Failed to parse {}", path.display()))?;
  profiles.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
  Ok(profiles)
}

pub fn save_profile(app: &AppHandle, draft: ProfileDraft) -> Result<Vec<ProfileRecord>> {
  let name = draft.name.trim();
  if name.is_empty() {
    return Err(anyhow!("Profile name cannot be empty."));
  }

  let path = profiles_path(app)?;
  let mut profiles = load_profiles(app)?;
  let now = unix_time_ms();

  match draft.id.as_deref() {
    Some(id) => {
      if let Some(profile) = profiles.iter_mut().find(|profile| profile.id == id) {
        profile.name = name.to_string();
        profile.config = draft.config;
        profile.updated_at_ms = now;
      } else {
        profiles.push(ProfileRecord {
          id: id.to_string(),
          name: name.to_string(),
          config: draft.config,
          created_at_ms: now,
          updated_at_ms: now,
        });
      }
    }
    None => {
      profiles.push(ProfileRecord {
        id: format!("profile-{now}"),
        name: name.to_string(),
        config: draft.config,
        created_at_ms: now,
        updated_at_ms: now,
      });
    }
  }

  profiles.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
  fs::write(&path, serde_json::to_string_pretty(&profiles)?)
    .with_context(|| format!("Failed to write {}", path.display()))?;

  Ok(profiles)
}

fn profiles_path(app: &AppHandle) -> Result<PathBuf> {
  let mut dir = app
    .path()
    .app_data_dir()
    .map_err(|error| anyhow!("Failed to resolve app data directory: {error}"))?;
  fs::create_dir_all(&dir)
    .with_context(|| format!("Failed to create application data directory {}", dir.display()))?;
  dir.push("profiles.json");
  Ok(dir)
}
