use anyhow::Result;

use crate::models::PermissionStatus;
use crate::platform;

pub fn initialize_platform() -> Result<()> {
  platform::initialize()
}

pub fn current_permission_status() -> Result<PermissionStatus> {
  platform::permission_status()
}
