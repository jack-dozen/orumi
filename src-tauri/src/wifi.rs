//! Status WiFi (SSID, sinyal) lewat WLAN API, dengan pembaruan berbasis notifikasi.
//! Diadaptasi dari Bloom (GPL-3.0): github.com/SehajveerSingh2005/bloom, wifi.rs.
#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::OnceLock;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::{
    core::{GUID, HSTRING, PCWSTR},
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, HANDLE},
        NetworkManagement::WiFi::{
            wlan_intf_opcode_current_connection, wlan_notification_acm_connection_complete,
            wlan_notification_acm_disconnected, wlan_notification_acm_interface_arrival,
            wlan_notification_acm_interface_removal, WlanCloseHandle, WlanEnumInterfaces,
            WlanFreeMemory, WlanOpenHandle, WlanQueryInterface, WlanRegisterNotification,
            DOT11_SSID, L2_NOTIFICATION_DATA, WLAN_CONNECTION_ATTRIBUTES,
            WLAN_INTERFACE_INFO_LIST, WLAN_NOTIFICATION_SOURCE_ACM,
        },
    },
};

#[derive(Serialize)]
pub struct WifiStatus {
    /// Ada adapter WiFi di PC ini.
    adapter: bool,
    connected: bool,
    ssid: Option<String>,
    /// Kualitas sinyal 0-100 versi Windows (bukan dBm).
    signal: u32,
    /// Windows menolak akses (biasanya izin lokasi untuk app desktop mati).
    location_denied: bool,
}

/// Handle klien WLAN, ditutup otomatis.
struct Client(HANDLE);

impl Client {
    fn open() -> Option<Self> {
        let mut version = 0u32;
        let mut handle = HANDLE::default();
        let rc = unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) };
        (rc == 0).then(|| Client(handle))
    }

    /// Adapter WiFi pertama (seperti quick settings Windows, satu adapter saja).
    fn interface(&self) -> Option<GUID> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let rc = unsafe { WlanEnumInterfaces(self.0, None, &mut list) };
        if rc != 0 {
            return None;
        }
        unsafe {
            let guid = list
                .as_ref()
                .filter(|l| l.dwNumberOfItems > 0)
                .map(|l| l.InterfaceInfo[0].InterfaceGuid);
            WlanFreeMemory(list as *const _);
            guid
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.0, None);
        }
    }
}

fn ssid_bytes(ssid: &DOT11_SSID) -> &[u8] {
    &ssid.ucSSID[..(ssid.uSSIDLength as usize).min(ssid.ucSSID.len())]
}

fn read_status() -> WifiStatus {
    let mut st = WifiStatus {
        adapter: false,
        connected: false,
        ssid: None,
        signal: 0,
        location_denied: false,
    };
    let Some(client) = Client::open() else { return st };
    let Some(iface) = client.interface() else { return st };
    st.adapter = true;

    let mut size = 0u32;
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    unsafe {
        let rc = WlanQueryInterface(
            client.0,
            &iface,
            wlan_intf_opcode_current_connection,
            None,
            &mut size,
            &mut data,
            None,
        );
        if rc == 0 {
            if let Some(attrs) = (data as *const WLAN_CONNECTION_ATTRIBUTES).as_ref() {
                let assoc = &attrs.wlanAssociationAttributes;
                st.connected = true;
                st.signal = assoc.wlanSignalQuality.min(100);
                let name = String::from_utf8_lossy(ssid_bytes(&assoc.dot11Ssid)).into_owned();
                if !name.is_empty() {
                    st.ssid = Some(name);
                }
            }
        } else if rc == ERROR_ACCESS_DENIED.0 {
            st.location_denied = true;
        }
        // Gagal dengan ERROR_INVALID_STATE kalau tidak tersambung: itu normal.
        if !data.is_null() {
            WlanFreeMemory(data);
        }
    }
    st
}

#[tauri::command]
pub async fn get_wifi() -> Result<WifiStatus, String> {
    tauri::async_runtime::spawn_blocking(read_status)
        .await
        .map_err(|e| e.to_string())
}

// ---------- Notifikasi ----------

static APP: OnceLock<AppHandle> = OnceLock::new();

unsafe extern "system" fn on_notification(
    data: *mut L2_NOTIFICATION_DATA,
    _: *mut std::ffi::c_void,
) {
    let Some(data) = data.as_ref() else { return };
    if data.NotificationSource != WLAN_NOTIFICATION_SOURCE_ACM {
        return;
    }
    let code = data.NotificationCode as i32;
    let relevant = [
        wlan_notification_acm_connection_complete.0,
        wlan_notification_acm_disconnected.0,
        wlan_notification_acm_interface_arrival.0,
        wlan_notification_acm_interface_removal.0,
    ];
    if relevant.contains(&code) {
        if let Some(app) = APP.get() {
            let _ = app.emit("wifi-changed", ());
        }
    }
}

/// Mendaftar notifikasi WLAN supaya tampilan langsung berubah saat tersambung atau putus.
pub fn start_notifications(app: AppHandle) {
    let _ = APP.set(app);
    std::thread::spawn(|| {
        let Some(client) = Client::open() else { return };
        let rc = unsafe {
            WlanRegisterNotification(
                client.0,
                WLAN_NOTIFICATION_SOURCE_ACM,
                true,
                Some(on_notification),
                None,
                None,
                None,
            )
        };
        if rc == 0 {
            // Handle harus tetap terbuka selama app hidup: menutupnya mencabut callback.
            std::mem::forget(client);
        }
    });
}

// ---------- Halaman Pengaturan Windows ----------

/// page: "wifi" atau "location".
#[tauri::command]
pub fn open_settings_page(page: String) -> Result<(), String> {
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let uri = match page.as_str() {
        "wifi" => "ms-settings:network-wifi",
        "location" => "ms-settings:privacy-location",
        _ => return Err(format!("halaman tidak dikenal: {page}")),
    };
    unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(uri),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
    Ok(())
}
