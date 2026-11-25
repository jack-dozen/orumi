//! WiFi status, network scanning, profile management, and connection via the Windows WLAN API.
//! Status & notification pattern adapted from Bloom (GPL-3.0): github.com/SehajveerSingh2005/bloom, wifi.rs.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    sync::{Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use windows::{
    core::{GUID, HSTRING, PCWSTR, PWSTR},
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, HANDLE},
        NetworkManagement::WiFi::{
            dot11_BSS_type_infrastructure, dot11_radio_state_off,
            wlan_intf_opcode_current_connection, wlan_intf_opcode_radio_state,
            wlan_notification_acm_connection_attempt_fail,
            wlan_notification_acm_connection_complete, wlan_notification_acm_disconnected,
            wlan_notification_acm_interface_arrival, wlan_notification_acm_interface_removal,
            wlan_notification_acm_scan_complete, wlan_notification_acm_scan_fail, WlanCloseHandle,
            WlanConnect, WlanDeleteProfile, WlanDisconnect, WlanEnumInterfaces, WlanFreeMemory,
            WlanGetAvailableNetworkList, WlanGetProfile, WlanGetProfileList, WlanOpenHandle,
            WlanQueryInterface, WlanReasonCodeToString, WlanRegisterNotification, WlanScan,
            WlanSetProfile, DOT11_AUTH_ALGO_80211_OPEN, DOT11_AUTH_ALGO_80211_SHARED_KEY,
            DOT11_AUTH_ALGO_RSNA, DOT11_AUTH_ALGO_RSNA_PSK, DOT11_AUTH_ALGO_WPA,
            DOT11_AUTH_ALGO_WPA3, DOT11_AUTH_ALGO_WPA3_ENT, DOT11_AUTH_ALGO_WPA3_ENT_192,
            DOT11_AUTH_ALGO_WPA3_SAE, DOT11_AUTH_ALGO_WPA_PSK, DOT11_AUTH_ALGORITHM,
            DOT11_CIPHER_ALGORITHM, DOT11_CIPHER_ALGO_CCMP, DOT11_CIPHER_ALGO_GCMP,
            DOT11_CIPHER_ALGO_GCMP_256, DOT11_CIPHER_ALGO_NONE, DOT11_CIPHER_ALGO_TKIP,
            DOT11_CIPHER_ALGO_WEP, DOT11_CIPHER_ALGO_WEP104, DOT11_CIPHER_ALGO_WEP40, DOT11_SSID,
            L2_NOTIFICATION_DATA, WLAN_AVAILABLE_NETWORK_CONNECTED,
            WLAN_AVAILABLE_NETWORK_HAS_PROFILE, WLAN_AVAILABLE_NETWORK_LIST,
            WLAN_CONNECTION_ATTRIBUTES, WLAN_CONNECTION_NOTIFICATION_DATA,
            WLAN_CONNECTION_PARAMETERS, WLAN_INTERFACE_INFO_LIST, WLAN_NOTIFICATION_SOURCE_ACM,
            WLAN_PROFILE_INFO_LIST, WLAN_RADIO_STATE, wlan_connection_mode_profile,
        },
    },
};

#[derive(Serialize)]
pub struct WifiStatus {
    /// A WiFi adapter is present on this PC.
    adapter: bool,
    connected: bool,
    ssid: Option<String>,
    /// Windows signal quality from 0 to 100 (not dBm).
    signal: u32,
    /// Windows denied access (usually because location permission for desktop apps is disabled).
    location_denied: bool,
}

#[derive(Serialize, Clone)]
pub struct WifiNetwork {
    pub ssid: String,
    pub signal: u32,
    pub connected: bool,
    pub saved: bool,
    pub profile_name: Option<String>,
    pub secured: bool,
    pub auth: String,
    pub auth_kind: String,
    pub connectable: bool,
}

#[derive(Serialize, Clone)]
pub struct WifiListResult {
    pub adapter: bool,
    pub radio_on: bool,
    pub location_denied: bool,
    pub connected_ssid: Option<String>,
    pub networks: Vec<WifiNetwork>,
}

/// WLAN client handle, closed automatically.
struct Client(HANDLE);

impl Client {
    fn open() -> Option<Self> {
        let mut version = 0u32;
        let mut handle = HANDLE::default();
        let rc = unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) };
        (rc == 0).then(|| Client(handle))
    }

    /// First WiFi adapter (like Windows quick settings, which uses only one adapter).
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

fn wide_slice_to_string(slice: &[u16]) -> String {
    let end = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..end])
}

fn reason_code_string(reason: u32) -> String {
    if reason == 0 {
        return "Success".into();
    }
    let mut buf = vec![0u16; 512];
    let slice = unsafe { std::slice::from_raw_parts(buf.as_mut_ptr(), buf.len()) };
    let rc = unsafe { WlanReasonCodeToString(reason, slice, None) };
    if rc == 0 {
        let msg = wide_slice_to_string(&buf).trim().to_string();
        if !msg.is_empty() {
            return msg;
        }
    }
    format!("WLAN error 0x{reason:08X}")
}

fn is_radio_on(client: &Client, iface: &GUID) -> bool {
    let mut size = 0u32;
    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
    let rc = unsafe {
        WlanQueryInterface(
            client.0,
            iface,
            wlan_intf_opcode_radio_state,
            None,
            &mut size,
            &mut data,
            None,
        )
    };
    if rc != 0 || data.is_null() {
        return true;
    }
    let on = unsafe {
        let st = &*(data as *const WLAN_RADIO_STATE);
        let n = (st.dwNumberOfPhys as usize).min(st.PhyRadioState.len());
        if n == 0 {
            true
        } else {
            st.PhyRadioState[..n].iter().any(|p| {
                p.dot11SoftwareRadioState != dot11_radio_state_off
                    && p.dot11HardwareRadioState != dot11_radio_state_off
            })
        }
    };
    unsafe {
        WlanFreeMemory(data);
    }
    on
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
        // ERROR_INVALID_STATE when disconnected is expected.
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

fn classify_auth(
    secured: bool,
    auth: DOT11_AUTH_ALGORITHM,
    cipher: DOT11_CIPHER_ALGORITHM,
) -> (&'static str, &'static str) {
    if !secured || (auth == DOT11_AUTH_ALGO_80211_OPEN && cipher == DOT11_CIPHER_ALGO_NONE) {
        return ("Open", "open");
    }
    if auth == DOT11_AUTH_ALGO_WPA3_SAE || auth == DOT11_AUTH_ALGO_WPA3 {
        return ("WPA3-Personal", "wpa3sae");
    }
    if auth == DOT11_AUTH_ALGO_RSNA_PSK {
        return ("WPA2-Personal", "wpa2psk");
    }
    if auth == DOT11_AUTH_ALGO_WPA_PSK {
        if cipher == DOT11_CIPHER_ALGO_CCMP {
            return ("WPA-Personal", "wpapsk_aes");
        }
        return ("WPA-Personal", "wpapsk");
    }
    if auth == DOT11_AUTH_ALGO_RSNA
        || auth == DOT11_AUTH_ALGO_WPA
        || auth == DOT11_AUTH_ALGO_WPA3_ENT
        || auth == DOT11_AUTH_ALGO_WPA3_ENT_192
    {
        return ("Enterprise", "enterprise");
    }
    if auth == DOT11_AUTH_ALGO_80211_SHARED_KEY
        || cipher == DOT11_CIPHER_ALGO_WEP
        || cipher == DOT11_CIPHER_ALGO_WEP40
        || cipher == DOT11_CIPHER_ALGO_WEP104
    {
        return ("WEP", "wep");
    }
    if cipher == DOT11_CIPHER_ALGO_GCMP || cipher == DOT11_CIPHER_ALGO_GCMP_256 {
        return ("WPA3-Personal", "wpa3sae");
    }
    if cipher == DOT11_CIPHER_ALGO_CCMP {
        return ("WPA2-Personal", "wpa2psk");
    }
    if cipher == DOT11_CIPHER_ALGO_TKIP {
        return ("WPA-Personal", "wpapsk");
    }
    ("Secured", "other")
}

/// Extract `<SSID>...<name>...</name>...</SSID>` from a WLAN profile XML if present.
fn extract_profile_ssid(xml: &str) -> Option<String> {
    let ssid_start = xml.find("<SSID>")?;
    let ssid_end = xml[ssid_start..].find("</SSID>")? + ssid_start;
    let block = &xml[ssid_start..ssid_end];
    let name_start = block.find("<name>")? + "<name>".len();
    let name_end = block[name_start..].find("</name>")? + name_start;
    Some(xml_unescape(&block[name_start..name_end]))
}

/// Map SSID -> profile name for all saved profiles on the interface.
fn saved_profiles_map(client: &Client, iface: &GUID) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut list: *mut WLAN_PROFILE_INFO_LIST = std::ptr::null_mut();
    let rc = unsafe { WlanGetProfileList(client.0, iface, None, &mut list) };
    if rc != 0 || list.is_null() {
        return map;
    }
    unsafe {
        let count = (*list).dwNumberOfItems as usize;
        let ptr = (*list).ProfileInfo.as_ptr();
        for i in 0..count {
            let info = &*ptr.add(i);
            let prof_name = wide_slice_to_string(&info.strProfileName);
            if prof_name.is_empty() {
                continue;
            }
            let mut xml_ptr = PWSTR::null();
            let mut flags = 0u32;
            let mut access = 0u32;
            let prc = WlanGetProfile(
                client.0,
                iface,
                &HSTRING::from(prof_name.as_str()),
                None,
                &mut xml_ptr,
                Some(&mut flags),
                Some(&mut access),
            );
            if prc == 0 && !xml_ptr.is_null() {
                let xml = xml_ptr.to_string().unwrap_or_default();
                WlanFreeMemory(xml_ptr.0 as *const _);
                if let Some(ssid) = extract_profile_ssid(&xml) {
                    map.entry(ssid).or_insert_with(|| prof_name.clone());
                }
            }
            map.entry(prof_name.clone()).or_insert(prof_name);
        }
        WlanFreeMemory(list as *const _);
    }
    map
}

fn read_network_list() -> WifiListResult {
    let mut out = WifiListResult {
        adapter: false,
        radio_on: false,
        location_denied: false,
        connected_ssid: None,
        networks: Vec::new(),
    };
    let Some(client) = Client::open() else { return out };
    let Some(iface) = client.interface() else { return out };
    out.adapter = true;
    out.radio_on = is_radio_on(&client, &iface);
    if !out.radio_on {
        return out;
    }

    let status = read_status();
    out.location_denied = status.location_denied;
    out.connected_ssid = status.ssid.clone();

    let profiles = saved_profiles_map(&client, &iface);

    let mut net_list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
    let rc = unsafe { WlanGetAvailableNetworkList(client.0, &iface, 0, None, &mut net_list) };
    if rc == ERROR_ACCESS_DENIED.0 {
        out.location_denied = true;
        return out;
    }
    if rc != 0 || net_list.is_null() {
        return out;
    }

    let mut merged: HashMap<String, WifiNetwork> = HashMap::new();
    unsafe {
        let count = (*net_list).dwNumberOfItems as usize;
        let ptr = (*net_list).Network.as_ptr();
        for i in 0..count {
            let item = &*ptr.add(i);
            let ssid = String::from_utf8_lossy(ssid_bytes(&item.dot11Ssid)).into_owned();
            if ssid.is_empty() {
                continue;
            }
            let connected = (item.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED) != 0
                || out.connected_ssid.as_deref() == Some(ssid.as_str());
            let has_profile_flag = (item.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE) != 0;
            let raw_prof = wide_slice_to_string(&item.strProfileName);
            let profile_name = if !raw_prof.is_empty() {
                Some(raw_prof)
            } else {
                profiles.get(&ssid).cloned()
            };
            let saved = has_profile_flag || profile_name.is_some();
            let secured = item.bSecurityEnabled.as_bool();
            let (auth_label, auth_kind) = classify_auth(
                secured,
                item.dot11DefaultAuthAlgorithm,
                item.dot11DefaultCipherAlgorithm,
            );
            let signal = item.wlanSignalQuality.min(100);
            let connectable = item.bNetworkConnectable.as_bool();

            match merged.get_mut(&ssid) {
                Some(existing) => {
                    existing.connected |= connected;
                    existing.saved |= saved;
                    if existing.profile_name.is_none() {
                        existing.profile_name = profile_name;
                    }
                    if signal > existing.signal {
                        existing.signal = signal;
                        existing.secured = secured;
                        existing.auth = auth_label.into();
                        existing.auth_kind = auth_kind.into();
                    }
                    existing.connectable |= connectable;
                }
                None => {
                    merged.insert(
                        ssid.clone(),
                        WifiNetwork {
                            ssid,
                            signal,
                            connected,
                            saved,
                            profile_name,
                            secured,
                            auth: auth_label.into(),
                            auth_kind: auth_kind.into(),
                            connectable,
                        },
                    );
                }
            }
        }
        WlanFreeMemory(net_list as *const _);
    }

    let mut list: Vec<WifiNetwork> = merged.into_values().collect();
    list.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.saved.cmp(&a.saved))
            .then(b.signal.cmp(&a.signal))
            .then_with(|| a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase()))
    });
    out.networks = list;
    out
}

#[tauri::command]
pub async fn wifi_list() -> Result<WifiListResult, String> {
    tauri::async_runtime::spawn_blocking(read_network_list)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn wifi_scan() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let client = Client::open().ok_or_else(|| "Unable to open WLAN service".to_string())?;
        let iface = client
            .interface()
            .ok_or_else(|| "No Wi-Fi adapter found".to_string())?;
        let rc = unsafe { WlanScan(client.0, &iface, None, None, None) };
        if rc == 0 {
            Ok(())
        } else if rc == ERROR_ACCESS_DENIED.0 {
            Err("Location permission is required to scan Wi-Fi networks".into())
        } else {
            Err(format!("Wi-Fi scan failed (error {rc})"))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0F) as usize] as char);
    }
    s
}

fn build_profile_xml(
    ssid: &str,
    auth_kind: &str,
    password: Option<&str>,
    auto_connect: bool,
) -> Result<String, String> {
    let esc_name = xml_escape(ssid);
    let hex_ssid = hex_encode(ssid.as_bytes());
    let mode = if auto_connect { "auto" } else { "manual" };

    let security_xml = match auth_kind {
        "open" => "\
            <authEncryption>\n\
                <authentication>open</authentication>\n\
                <encryption>none</encryption>\n\
                <useOneX>false</useOneX>\n\
            </authEncryption>"
            .to_string(),
        "wpa3sae" | "wpa2psk" | "wpapsk" | "wpapsk_aes" => {
            let pw = password.filter(|p| !p.is_empty()).ok_or("Password is required")?;
            let esc_pw = xml_escape(pw);
            let (auth, enc) = match auth_kind {
                "wpa3sae" => ("WPA3SAE", "AES"),
                "wpapsk" => ("WPAPSK", "TKIP"),
                "wpapsk_aes" => ("WPAPSK", "AES"),
                _ => ("WPA2PSK", "AES"),
            };
            format!(
                "\
            <authEncryption>\n\
                <authentication>{auth}</authentication>\n\
                <encryption>{enc}</encryption>\n\
                <useOneX>false</useOneX>\n\
            </authEncryption>\n\
            <sharedKey>\n\
                <keyType>passPhrase</keyType>\n\
                <protected>false</protected>\n\
                <keyMaterial>{esc_pw}</keyMaterial>\n\
            </sharedKey>"
            )
        }
        "wep" => {
            let pw = password.filter(|p| !p.is_empty()).ok_or("Password is required")?;
            let esc_pw = xml_escape(pw);
            format!(
                "\
            <authEncryption>\n\
                <authentication>open</authentication>\n\
                <encryption>WEP</encryption>\n\
                <useOneX>false</useOneX>\n\
            </authEncryption>\n\
            <sharedKey>\n\
                <keyType>networkKey</keyType>\n\
                <protected>false</protected>\n\
                <keyMaterial>{esc_pw}</keyMaterial>\n\
            </sharedKey>"
            )
        }
        "enterprise" => {
            return Err("Enterprise networks must be connected via Windows Settings".into());
        }
        _ => {
            return Err("Unsupported network security type; use Windows Settings".into());
        }
    };

    Ok(format!(
        "<?xml version=\"1.0\"?>\n\
<WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">\n\
    <name>{esc_name}</name>\n\
    <SSIDConfig>\n\
        <SSID>\n\
            <hex>{hex_ssid}</hex>\n\
            <name>{esc_name}</name>\n\
        </SSID>\n\
    </SSIDConfig>\n\
    <connectionType>ESS</connectionType>\n\
    <connectionMode>{mode}</connectionMode>\n\
    <MSM>\n\
        <security>\n\
{security_xml}\n\
        </security>\n\
    </MSM>\n\
</WLANProfile>"
    ))
}

struct PendingConnect {
    ssid: String,
    done: bool,
    reason_code: u32,
}

static PENDING_CONNECT: OnceLock<(Mutex<Option<PendingConnect>>, Condvar)> = OnceLock::new();

fn pending_sync() -> &'static (Mutex<Option<PendingConnect>>, Condvar) {
    PENDING_CONNECT.get_or_init(|| (Mutex::new(None), Condvar::new()))
}

#[tauri::command]
pub async fn wifi_connect(
    ssid: String,
    password: Option<String>,
    auto_connect: bool,
    auth_kind: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let client = Client::open().ok_or_else(|| "Unable to open WLAN service".to_string())?;
        let iface = client
            .interface()
            .ok_or_else(|| "No Wi-Fi adapter found".to_string())?;

        let profiles = saved_profiles_map(&client, &iface);
        let existing_profile = profiles.get(&ssid).cloned();
        let use_existing = password.as_deref().map_or(true, |p| p.is_empty()) && existing_profile.is_some();

        let (profile_name, created_new_profile) = if use_existing {
            (existing_profile.unwrap(), false)
        } else {
            let kind = auth_kind.as_deref().unwrap_or_else(|| {
                if password.as_deref().is_some_and(|p| !p.is_empty()) {
                    "wpa2psk"
                } else {
                    "open"
                }
            });
            let xml = build_profile_xml(&ssid, kind, password.as_deref(), auto_connect)?;
            let xml_h = HSTRING::from(xml.as_str());
            let mut reason = 0u32;
            let rc = unsafe {
                WlanSetProfile(
                    client.0,
                    &iface,
                    0,
                    PCWSTR(xml_h.as_ptr()),
                    None,
                    true,
                    None,
                    &mut reason,
                )
            };
            if rc != 0 {
                let detail = if reason != 0 {
                    reason_code_string(reason)
                } else {
                    format!("error {rc}")
                };
                return Err(format!("Failed to save Wi-Fi profile: {detail}"));
            }
            (ssid.clone(), existing_profile.is_none())
        };

        // Arm pending completion waiter before issuing WlanConnect.
        {
            let (lock, _) = pending_sync();
            let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(PendingConnect {
                ssid: ssid.clone(),
                done: false,
                reason_code: 0,
            });
        }

        let prof_h = HSTRING::from(profile_name.as_str());
        let params = WLAN_CONNECTION_PARAMETERS {
            wlanConnectionMode: wlan_connection_mode_profile,
            strProfile: PCWSTR(prof_h.as_ptr()),
            pDot11Ssid: std::ptr::null_mut(),
            pDesiredBssidList: std::ptr::null_mut(),
            dot11BssType: dot11_BSS_type_infrastructure,
            dwFlags: 0,
        };
        let rc = unsafe { WlanConnect(client.0, &iface, &params, None) };
        if rc != 0 {
            let (lock, _) = pending_sync();
            *lock.lock().unwrap_or_else(|e| e.into_inner()) = None;
            if created_new_profile {
                let _ = unsafe { WlanDeleteProfile(client.0, &iface, &prof_h, None) };
            }
            return Err(format!("Unable to start connection (error {rc})"));
        }

        // Wait up to 14s for Windows ACM connection_complete notification.
        let (lock, cv) = pending_sync();
        let deadline = Instant::now() + Duration::from_secs(14);
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut result_reason: Option<u32> = None;
        while let Some(p) = guard.as_ref() {
            if p.ssid != ssid {
                break;
            }
            if p.done {
                result_reason = Some(p.reason_code);
                break;
            }
            let rem = deadline.saturating_duration_since(Instant::now());
            if rem.is_zero() {
                break;
            }
            let (g, _) = cv.wait_timeout(guard, rem).unwrap_or_else(|e| e.into_inner());
            guard = g;
        }
        *guard = None;
        drop(guard);

        if let Some(reason) = result_reason {
            if reason != 0 {
                if created_new_profile {
                    let _ = unsafe { WlanDeleteProfile(client.0, &iface, &prof_h, None) };
                }
                return Err(reason_code_string(reason));
            }
            return Ok(());
        }

        // Timed out waiting for notification: check actual current status.
        let st = read_status();
        if st.connected && st.ssid.as_deref() == Some(ssid.as_str()) {
            Ok(())
        } else {
            Err("Connection timed out or failed".into())
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit("wifi-changed", ());
    res
}

#[tauri::command]
pub async fn wifi_disconnect(app: AppHandle) -> Result<(), String> {
    let res = tauri::async_runtime::spawn_blocking(|| {
        let client = Client::open().ok_or_else(|| "Unable to open WLAN service".to_string())?;
        let iface = client
            .interface()
            .ok_or_else(|| "No Wi-Fi adapter found".to_string())?;
        let rc = unsafe { WlanDisconnect(client.0, &iface, None) };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("Failed to disconnect (error {rc})"))
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit("wifi-changed", ());
    res
}

#[tauri::command]
pub async fn wifi_forget(profile_name: String, app: AppHandle) -> Result<(), String> {
    let res = tauri::async_runtime::spawn_blocking(move || {
        let client = Client::open().ok_or_else(|| "Unable to open WLAN service".to_string())?;
        let iface = client
            .interface()
            .ok_or_else(|| "No Wi-Fi adapter found".to_string())?;
        let prof_h = HSTRING::from(profile_name.as_str());
        let rc = unsafe { WlanDeleteProfile(client.0, &iface, &prof_h, None) };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("Failed to forget network (error {rc})"))
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit("wifi-changed", ());
    res
}

// ---------- Notifications ----------

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

    if code == wlan_notification_acm_connection_complete.0 && !data.pData.is_null() {
        let info = &*(data.pData as *const WLAN_CONNECTION_NOTIFICATION_DATA);
        let ssid = String::from_utf8_lossy(ssid_bytes(&info.dot11Ssid)).into_owned();
        let (lock, cv) = pending_sync();
        if let Ok(mut guard) = lock.lock() {
            if let Some(p) = guard.as_mut() {
                if p.ssid == ssid || ssid.is_empty() {
                    p.done = true;
                    p.reason_code = info.wlanReasonCode;
                    cv.notify_all();
                }
            }
        }
    } else if code == wlan_notification_acm_connection_attempt_fail.0 && !data.pData.is_null() {
        let info = &*(data.pData as *const WLAN_CONNECTION_NOTIFICATION_DATA);
        let ssid = String::from_utf8_lossy(ssid_bytes(&info.dot11Ssid)).into_owned();
        if info.wlanReasonCode != 0 {
            let (lock, cv) = pending_sync();
            if let Ok(mut guard) = lock.lock() {
                if let Some(p) = guard.as_mut() {
                    if p.ssid == ssid {
                        p.done = true;
                        p.reason_code = info.wlanReasonCode;
                        cv.notify_all();
                    }
                }
            }
        }
    }

    let relevant = [
        wlan_notification_acm_connection_complete.0,
        wlan_notification_acm_disconnected.0,
        wlan_notification_acm_interface_arrival.0,
        wlan_notification_acm_interface_removal.0,
        wlan_notification_acm_scan_complete.0,
        wlan_notification_acm_scan_fail.0,
    ];
    if relevant.contains(&code) {
        if let Some(app) = APP.get() {
            let _ = app.emit("wifi-changed", ());
        }
    }
}

/// Register WLAN notifications so the display updates immediately when connecting, disconnecting, or scanning.
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
            // Keep the handle open while the app runs: closing it removes the callback.
            std::mem::forget(client);
        }
    });
}

// ---------- Windows Settings pages ----------

/// Open a Windows Settings page or system URI (right-click on quick tiles, etc.).
#[tauri::command]
pub fn open_settings_page(page: String) -> Result<(), String> {
    use windows::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

    let uri = match page.as_str() {
        "wifi" => "ms-settings:network-wifi",
        "bluetooth" => "ms-settings:bluetooth",
        "airplane" => "ms-settings:network-airplanemode",
        "hotspot" => "ms-settings:network-mobilehotspot",
        "dnd" | "notifications" => "ms-settings:notifications",
        "sound" | "mic" => "ms-settings:sound",
        "power" => "ms-settings:powersleep",
        "colors" | "theme" => "ms-settings:colors",
        "display" => "ms-settings:display",
        "location" => "ms-settings:privacy-location",
        "touchpad" => "ms-settings:devices-touchpad",
        "snip" => "ms-screenclip:",
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
