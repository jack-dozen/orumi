//! Battery status through GetSystemPowerStatus.
//! Flag interpretation follows Bloom (GPL-3.0): battery.rs.

use serde::Serialize;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

const UNKNOWN_FLAG: u8 = 255;
const NO_SYSTEM_BATTERY: u8 = 128;
const CHARGING_FLAG: u8 = 8;

#[derive(Serialize)]
pub struct BatteryStatus {
    percent: u8,
    charging: bool,
    /// Connected to power (not necessarily charging, e.g. when fully charged).
    plugged: bool,
}

/// None = this PC has no battery (or its status cannot be read).
#[tauri::command]
pub fn get_battery() -> Option<BatteryStatus> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;

    let present = status.BatteryFlag != UNKNOWN_FLAG && status.BatteryFlag & NO_SYSTEM_BATTERY == 0;
    if !present || status.BatteryLifePercent > 100 {
        return None;
    }
    Some(BatteryStatus {
        percent: status.BatteryLifePercent,
        charging: status.BatteryFlag & CHARGING_FLAG != 0,
        plugged: status.ACLineStatus == 1,
    })
}
