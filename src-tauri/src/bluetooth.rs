//! Bluetooth device discovery, pairing (with PIN/confirmation), and connect/disconnect
//! via WinRT (`Windows.Devices.Enumeration` / `Windows.Devices.Bluetooth`) and Win32 Bluetooth APIs.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use windows::{
    core::{Interface, GUID, HSTRING},
    Devices::{
        Bluetooth::{
            BluetoothDevice, BluetoothLEDevice,
            GenericAttributeProfile::{GattCommunicationStatus, GattSession},
        },
        Enumeration::{
            DeviceInformation, DeviceInformationKind, DeviceInformationUpdate,
            DevicePairingKinds, DevicePairingProtectionLevel, DevicePairingRequestedEventArgs,
            DevicePairingResultStatus, DeviceUnpairingResultStatus, DeviceWatcher,
            DeviceWatcherStatus,
        },
        Radios::{Radio, RadioKind, RadioState},
    },
    Foundation::{IPropertyValue, PropertyType, TypedEventHandler},
    Win32::{
        Devices::Bluetooth::{
            BluetoothEnumerateInstalledServices, BluetoothFindDeviceClose, BluetoothFindFirstDevice,
            BluetoothFindFirstRadio, BluetoothFindNextDevice, BluetoothFindRadioClose,
            BluetoothSetServiceState, BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS,
            BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_SERVICE_DISABLE, BLUETOOTH_SERVICE_ENABLE,
        },
        Foundation::{CloseHandle, HANDLE},
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
    },
};

const PROP_ADDRESS: &str = "System.Devices.Aep.DeviceAddress";
const PROP_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const PROP_CATEGORY: &str = "System.Devices.Aep.Category";
const PROP_CONTAINER_ID: &str = "System.Devices.Aep.ContainerId";
const PROP_CAN_PAIR: &str = "System.Devices.Aep.CanPair";
const PROP_IS_PAIRED: &str = "System.Devices.Aep.IsPaired";
const PROP_BT_BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
const PROP_DEV_CONTAINER_ID: &str = "System.Devices.ContainerId";

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> GUID {
    GUID { data1, data2, data3, data4 }
}

// Standard Bluetooth Classic profile service UUIDs.
const UUID_SERIAL_PORT: GUID = guid(0x00001101, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);
const UUID_HEADSET: GUID = guid(0x00001108, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);
const UUID_AUDIO_SINK: GUID = guid(0x0000110b, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);
const UUID_AVRCP_TARGET: GUID = guid(0x0000110c, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);
const UUID_AVRCP: GUID = guid(0x0000110e, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);
const UUID_HANDSFREE: GUID = guid(0x0000111e, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);
const UUID_HANDSFREE_AG: GUID = guid(0x0000111f, 0x0000, 0x1000, [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb]);

const CONNECTABLE_PROFILES: [GUID; 7] = [
    UUID_AUDIO_SINK,
    UUID_HANDSFREE,
    UUID_HEADSET,
    UUID_AVRCP_TARGET,
    UUID_AVRCP,
    UUID_HANDSFREE_AG,
    UUID_SERIAL_PORT,
];

#[derive(Serialize, Clone)]
pub struct BtDevice {
    pub id: String,
    pub address: u64,
    pub address_str: String,
    pub name: String,
    pub category: String,
    pub connected: bool,
    pub is_le: bool,
    pub battery: Option<u8>,
    pub can_connect: bool,
}

#[derive(Serialize, Clone)]
pub struct BtListResult {
    pub adapter: bool,
    pub radio_on: bool,
    pub devices: Vec<BtDevice>,
}

#[derive(Serialize, Clone)]
pub struct BtNearbyDevice {
    pub id: String,
    pub name: String,
    pub category: String,
    pub is_le: bool,
    pub can_pair: bool,
}

#[derive(Serialize, Clone)]
pub struct BtPairPrompt {
    pub id: String,
    pub name: String,
    /// "confirm_pin" | "display_pin" | "provide_pin"
    pub kind: String,
    pub pin: Option<String>,
}

pub struct BtState {
    gatt_sessions: Mutex<HashMap<String, GattSession>>,
    watchers: Mutex<Vec<DeviceWatcher>>,
    nearby: Arc<Mutex<HashMap<String, BtNearbyDevice>>>,
    pair_tx: Mutex<Option<mpsc::SyncSender<(bool, Option<String>)>>>,
}

impl BtState {
    pub fn new() -> Self {
        Self {
            gatt_sessions: Mutex::new(HashMap::new()),
            watchers: Mutex::new(Vec::new()),
            nearby: Arc::new(Mutex::new(HashMap::new())),
            pair_tx: Mutex::new(None),
        }
    }
}

fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn radio_status() -> (bool, bool) {
    init_com();
    let Ok(op) = Radio::GetRadiosAsync() else {
        return (false, false);
    };
    let Ok(list) = op.join() else {
        return (false, false);
    };
    let mut found = false;
    let mut on = false;
    for i in 0..list.Size().unwrap_or(0) {
        if let Ok(r) = list.GetAt(i) {
            if r.Kind().ok() == Some(RadioKind::Bluetooth) {
                found = true;
                on |= r.State().ok() == Some(RadioState::On);
            }
        }
    }
    (found, on)
}

fn hstring_iterable(items: &[&str]) -> windows_collections::IIterable<HSTRING> {
    let vec: Vec<HSTRING> = items.iter().map(|s| HSTRING::from(*s)).collect();
    let view: windows_collections::IVectorView<HSTRING> = vec.into();
    view.cast().expect("IVectorView<HSTRING> implements IIterable<HSTRING>")
}

fn prop_bool(props: &windows_collections::IMapView<HSTRING, windows::core::IInspectable>, key: &str) -> Option<bool> {
    let val = props.Lookup(&HSTRING::from(key)).ok()?;
    let pv = val.cast::<IPropertyValue>().ok()?;
    pv.GetBoolean().ok()
}

fn prop_string(props: &windows_collections::IMapView<HSTRING, windows::core::IInspectable>, key: &str) -> Option<String> {
    let val = props.Lookup(&HSTRING::from(key)).ok()?;
    let pv = val.cast::<IPropertyValue>().ok()?;
    match pv.Type().ok()? {
        PropertyType::String => pv.GetString().ok().map(|s| s.to_string()),
        PropertyType::StringArray => {
            let mut arr = windows::core::Array::new();
            pv.GetStringArray(&mut arr).ok()?;
            arr.first().map(|s| s.to_string())
        }
        _ => None,
    }
}

fn prop_guid_string(props: &windows_collections::IMapView<HSTRING, windows::core::IInspectable>, key: &str) -> Option<String> {
    let val = props.Lookup(&HSTRING::from(key)).ok()?;
    let pv = val.cast::<IPropertyValue>().ok()?;
    let g = pv.GetGuid().ok()?;
    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, g.data4[0], g.data4[1], g.data4[2], g.data4[3], g.data4[4], g.data4[5], g.data4[6], g.data4[7]
    ))
}

fn prop_u8(props: &windows_collections::IMapView<HSTRING, windows::core::IInspectable>, key: &str) -> Option<u8> {
    let val = props.Lookup(&HSTRING::from(key)).ok()?;
    let pv = val.cast::<IPropertyValue>().ok()?;
    match pv.Type().ok()? {
        PropertyType::UInt8 => pv.GetUInt8().ok(),
        PropertyType::UInt16 => pv.GetUInt16().ok().map(|v| v.min(100) as u8),
        PropertyType::UInt32 => pv.GetUInt32().ok().map(|v| v.min(100) as u8),
        PropertyType::Int32 => pv.GetInt32().ok().and_then(|v| (0..=100).contains(&v).then_some(v as u8)),
        _ => None,
    }
}

fn parse_mac_u64(s: &str) -> Option<u64> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if clean.len() != 12 {
        return None;
    }
    u64::from_str_radix(&clean, 16).ok()
}

fn format_mac(addr: u64) -> String {
    if addr == 0 {
        return String::new();
    }
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        (addr >> 40) & 0xFF,
        (addr >> 32) & 0xFF,
        (addr >> 24) & 0xFF,
        (addr >> 16) & 0xFF,
        (addr >> 8) & 0xFF,
        addr & 0xFF,
    )
}

fn classify_device(aep_cat: Option<&str>, cod: Option<u32>, name: &str) -> &'static str {
    if let Some(cat) = aep_cat {
        let lower = cat.to_lowercase();
        if lower.contains("headphone") || lower.contains("headset") || lower.contains("earbud") {
            return "headphones";
        }
        if lower.contains("speaker") {
            return "speaker";
        }
        if lower.contains("audio") {
            return "audio";
        }
        if lower.contains("keyboard") {
            return "keyboard";
        }
        if lower.contains("mouse") {
            return "mouse";
        }
        if lower.contains("gamepad") || lower.contains("joystick") || lower.contains("gaming") {
            return "gamepad";
        }
        if lower.contains("phone") {
            return "phone";
        }
    }

    if let Some(c) = cod {
        let major = (c >> 8) & 0x1F;
        let minor = (c >> 2) & 0x3F;
        match major {
            0x02 => return "phone",
            0x04 => {
                // Audio/Video major class.
                return match minor {
                    0x01 | 0x02 | 0x06 => "headphones",
                    0x05 | 0x08 | 0x09 => "speaker",
                    _ => "audio",
                };
            }
            0x05 => {
                // Peripheral major class.
                if (minor & 0x10) != 0 {
                    return "keyboard";
                }
                if (minor & 0x20) != 0 {
                    return "mouse";
                }
                if (minor & 0x03) != 0 {
                    return "gamepad";
                }
            }
            _ => {}
        }
    }

    let n = name.to_lowercase();
    if n.contains("airpods")
        || n.contains("buds")
        || n.contains("headphone")
        || n.contains("headset")
        || n.contains("wh-")
        || n.contains("wf-")
        || n.contains("tws")
    {
        return "headphones";
    }
    if n.contains("speaker") || n.contains("jbl") || n.contains("soundbar") {
        return "speaker";
    }
    if n.contains("keyboard") {
        return "keyboard";
    }
    if n.contains("mouse") {
        return "mouse";
    }
    if n.contains("controller") || n.contains("xbox") || n.contains("dualsense") || n.contains("gamepad") {
        return "gamepad";
    }
    if n.contains("phone") || n.contains("iphone") || n.contains("galaxy") || n.contains("pixel") {
        return "phone";
    }
    "other"
}

struct Win32BtInfo {
    name: String,
    cod: u32,
    connected: bool,
}

fn win32_paired_classic() -> HashMap<u64, Win32BtInfo> {
    let mut map = HashMap::new();
    unsafe {
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnAuthenticated: true.into(),
            fReturnRemembered: true.into(),
            fReturnUnknown: false.into(),
            fReturnConnected: true.into(),
            fIssueInquiry: false.into(),
            cTimeoutMultiplier: 0,
            hRadio: HANDLE::default(),
        };
        let mut info = BLUETOOTH_DEVICE_INFO {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
            ..Default::default()
        };
        let Ok(hfind) = BluetoothFindFirstDevice(&params, &mut info) else {
            return map;
        };
        loop {
            let addr = info.Address.Anonymous.ullLong;
            let end = info.szName.iter().position(|&c| c == 0).unwrap_or(info.szName.len());
            let name = String::from_utf16_lossy(&info.szName[..end]);
            map.insert(
                addr,
                Win32BtInfo {
                    name,
                    cod: info.ulClassofDevice,
                    connected: info.fConnected.as_bool(),
                },
            );
            info = BLUETOOTH_DEVICE_INFO {
                dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
                ..Default::default()
            };
            if BluetoothFindNextDevice(hfind, &mut info).is_err() {
                break;
            }
        }
        let _ = BluetoothFindDeviceClose(hfind);
    }
    map
}

/// Query Windows PnP devices that expose DEVPKEY_Bluetooth_Battery, keyed by lowercase ContainerId.
fn battery_by_container() -> HashMap<String, u8> {
    let mut out = HashMap::new();
    let req = hstring_iterable(&[PROP_BT_BATTERY, PROP_DEV_CONTAINER_ID]);
    let aqs = HSTRING::from("System.Devices.Present:=System.StructuredQueryType.Boolean#True");
    let Ok(op) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &aqs,
        &req,
        DeviceInformationKind::Device,
    ) else {
        return out;
    };
    let Ok(list) = op.join() else {
        return out;
    };
    for i in 0..list.Size().unwrap_or(0) {
        let Ok(dev) = list.GetAt(i) else { continue };
        let Ok(props) = dev.Properties() else { continue };
        let Some(pct) = prop_u8(&props, PROP_BT_BATTERY) else { continue };
        if let Some(cid) = prop_guid_string(&props, PROP_DEV_CONTAINER_ID) {
            out.insert(cid, pct);
        }
    }
    out
}

fn read_paired_devices() -> BtListResult {
    let (adapter, radio_on) = radio_status();
    if !adapter || !radio_on {
        return BtListResult {
            adapter,
            radio_on,
            devices: Vec::new(),
        };
    }

    let classic_win32 = win32_paired_classic();
    let batteries = battery_by_container();
    let req = hstring_iterable(&[
        PROP_ADDRESS,
        PROP_CONNECTED,
        PROP_CATEGORY,
        PROP_CONTAINER_ID,
        PROP_BT_BATTERY,
    ]);

    let mut by_key: HashMap<String, BtDevice> = HashMap::new();

    let selectors = [
        (BluetoothDevice::GetDeviceSelectorFromPairingState(true).ok(), false),
        (BluetoothLEDevice::GetDeviceSelectorFromPairingState(true).ok(), true),
    ];

    for (sel_opt, is_le) in selectors {
        let Some(sel) = sel_opt else { continue };
        let Ok(op) = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
            &sel,
            &req,
            DeviceInformationKind::AssociationEndpoint,
        ) else {
            continue;
        };
        let Ok(list) = op.join() else { continue };

        for i in 0..list.Size().unwrap_or(0) {
            let Ok(dev) = list.GetAt(i) else { continue };
            let id = dev.Id().map(|s| s.to_string()).unwrap_or_default();
            let raw_name = dev.Name().map(|s| s.to_string()).unwrap_or_default();
            let props = dev.Properties().ok();

            let addr_str = props
                .as_ref()
                .and_then(|p| prop_string(p, PROP_ADDRESS))
                .unwrap_or_default();
            let addr = parse_mac_u64(&addr_str).unwrap_or(0);
            let w32 = (addr != 0).then(|| classic_win32.get(&addr)).flatten();

            let name = if !raw_name.is_empty() {
                raw_name
            } else if let Some(w) = w32 {
                w.name.clone()
            } else {
                addr_str.clone()
            };
            if name.is_empty() {
                continue;
            }

            let connected_aep = props
                .as_ref()
                .and_then(|p| prop_bool(p, PROP_CONNECTED))
                .unwrap_or(false);
            let connected = connected_aep || w32.is_some_and(|w| w.connected);

            let aep_cat = props.as_ref().and_then(|p| prop_string(p, PROP_CATEGORY));
            let category = classify_device(aep_cat.as_deref(), w32.map(|w| w.cod), &name);

            let battery = props
                .as_ref()
                .and_then(|p| prop_u8(p, PROP_BT_BATTERY))
                .or_else(|| {
                    props
                        .as_ref()
                        .and_then(|p| prop_guid_string(p, PROP_CONTAINER_ID))
                        .and_then(|cid| batteries.get(&cid).copied())
                });

            let key = if addr != 0 {
                format!("{addr:012x}")
            } else {
                id.clone()
            };
            let formatted_addr = if addr != 0 { format_mac(addr) } else { addr_str };
            let can_connect = (!is_le && addr != 0) || is_le;

            match by_key.get_mut(&key) {
                Some(existing) => {
                    existing.connected |= connected;
                    if existing.battery.is_none() {
                        existing.battery = battery;
                    }
                    // Prefer Classic over LE when dual-mode so audio profile connect works.
                    if existing.is_le && !is_le {
                        existing.id = id;
                        existing.is_le = false;
                        existing.category = category.into();
                    }
                }
                None => {
                    by_key.insert(
                        key,
                        BtDevice {
                            id,
                            address: addr,
                            address_str: formatted_addr,
                            name,
                            category: category.into(),
                            connected,
                            is_le,
                            battery,
                            can_connect,
                        },
                    );
                }
            }
        }
    }

    // Include any Win32 Classic paired device that did not surface in AEP enumeration.
    for (addr, w) in classic_win32 {
        let key = format!("{addr:012x}");
        if !by_key.contains_key(&key) && !w.name.is_empty() {
            let cat = classify_device(None, Some(w.cod), &w.name);
            by_key.insert(
                key,
                BtDevice {
                    id: format!("win32:{addr:012x}"),
                    address: addr,
                    address_str: format_mac(addr),
                    name: w.name,
                    category: cat.into(),
                    connected: w.connected,
                    is_le: false,
                    battery: None,
                    can_connect: true,
                },
            );
        }
    }

    let mut devices: Vec<BtDevice> = by_key.into_values().collect();
    devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    BtListResult {
        adapter,
        radio_on,
        devices,
    }
}

#[tauri::command]
pub async fn bt_list() -> Result<BtListResult, String> {
    tauri::async_runtime::spawn_blocking(read_paired_devices)
        .await
        .map_err(|e| e.to_string())
}

fn first_radio_handle() -> Option<HANDLE> {
    unsafe {
        let params = BLUETOOTH_FIND_RADIO_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
        };
        let mut h_radio = HANDLE::default();
        let Ok(h_find) = BluetoothFindFirstRadio(&params, &mut h_radio) else {
            return None;
        };
        let _ = BluetoothFindRadioClose(h_find);
        (!h_radio.is_invalid()).then_some(h_radio)
    }
}

fn device_target_services(h_radio: HANDLE, info: &BLUETOOTH_DEVICE_INFO, category: &str) -> Vec<GUID> {
    let mut count = 32u32;
    let mut guids = vec![GUID::default(); count as usize];
    let rc = unsafe {
        BluetoothEnumerateInstalledServices(Some(h_radio), info, &mut count, Some(guids.as_mut_ptr()))
    };
    let mut out = Vec::new();
    if rc == 0 {
        guids.truncate(count as usize);
        for g in &guids {
            if CONNECTABLE_PROFILES.contains(g) {
                out.push(*g);
            }
        }
    }
    if out.is_empty() && matches!(category, "headphones" | "speaker" | "audio") {
        out.push(UUID_AUDIO_SINK);
        out.push(UUID_HANDSFREE);
    }
    out
}

fn classic_set_connected(address: u64, category: &str, connect: bool) -> Result<(), String> {
    let h_radio = first_radio_handle().ok_or_else(|| "Bluetooth radio not found".to_string())?;
    let mut info = BLUETOOTH_DEVICE_INFO {
        dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32,
        ..Default::default()
    };
    info.Address.Anonymous.ullLong = address;

    let services = device_target_services(h_radio, &info, category);
    if services.is_empty() {
        unsafe {
            let _ = CloseHandle(h_radio);
        }
        return Err("No connectable Bluetooth profiles found for this device; try Windows Settings".into());
    }

    let flag = if connect {
        BLUETOOTH_SERVICE_ENABLE
    } else {
        BLUETOOTH_SERVICE_DISABLE
    };
    let mut any_ok = false;
    let mut last_err = 0u32;

    unsafe {
        for guid in &services {
            if connect {
                // Toggle off briefly if Windows left a stale service handle.
                let _ = BluetoothSetServiceState(Some(h_radio), &info, guid, BLUETOOTH_SERVICE_DISABLE);
            }
            let rc = BluetoothSetServiceState(Some(h_radio), &info, guid, flag);
            if rc == 0 {
                any_ok = true;
            } else {
                last_err = rc;
            }
        }
        let _ = CloseHandle(h_radio);
    }

    if any_ok {
        Ok(())
    } else {
        Err(format!(
            "Could not {} device (error {last_err})",
            if connect { "connect" } else { "disconnect" }
        ))
    }
}

#[tauri::command]
pub async fn bt_connect(
    id: String,
    address: u64,
    is_le: bool,
    category: String,
    state: State<'_, BtState>,
    app: AppHandle,
) -> Result<(), String> {
    if !is_le && address != 0 {
        let cat = category.clone();
        let res = tauri::async_runtime::spawn_blocking(move || classic_set_connected(address, &cat, true))
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit("bt-changed", ());
        return res;
    }

    // BLE connection via GattSession + uncached GATT query.
    let key = id.clone();
    let session_res = tauri::async_runtime::spawn_blocking(move || -> Result<GattSession, String> {
        init_com();
        let dev = BluetoothLEDevice::FromIdAsync(&HSTRING::from(id.as_str()))
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let dev_id = dev.BluetoothDeviceId().map_err(|e| e.to_string())?;
        let session = GattSession::FromDeviceIdAsync(&dev_id)
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let _ = session.SetMaintainConnection(true);
        let gatt = dev
            .GetGattServicesAsync()
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        if gatt.Status().ok() != Some(GattCommunicationStatus::Success) {
            return Err("BLE device did not respond (make sure it is awake and in range)".into());
        }
        Ok(session)
    })
    .await
    .map_err(|e| e.to_string())?;

    if let Ok(session) = session_res {
        if let Ok(mut map) = state.gatt_sessions.lock() {
            map.insert(key, session);
        }
        let _ = app.emit("bt-changed", ());
        Ok(())
    } else {
        Err(session_res.err().unwrap_or_else(|| "Failed to connect BLE device".into()))
    }
}

#[tauri::command]
pub async fn bt_disconnect(
    id: String,
    address: u64,
    is_le: bool,
    category: String,
    state: State<'_, BtState>,
    app: AppHandle,
) -> Result<(), String> {
    if let Ok(mut map) = state.gatt_sessions.lock() {
        if let Some(session) = map.remove(&id) {
            let _ = session.SetMaintainConnection(false);
        }
    }

    if !is_le && address != 0 {
        let res = tauri::async_runtime::spawn_blocking(move || classic_set_connected(address, &category, false))
            .await
            .map_err(|e| e.to_string())?;
        let _ = app.emit("bt-changed", ());
        return res;
    }

    let _ = app.emit("bt-changed", ());
    Err("Windows does not allow disconnecting HID/BLE devices via public API; use device switch or Windows Settings".into())
}

#[tauri::command]
pub async fn bt_unpair(id: String, state: State<'_, BtState>, app: AppHandle) -> Result<(), String> {
    if let Ok(mut map) = state.gatt_sessions.lock() {
        if let Some(session) = map.remove(&id) {
            let _ = session.SetMaintainConnection(false);
        }
    }
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        init_com();
        let dev = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id.as_str()))
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let pairing = dev.Pairing().map_err(|e| e.to_string())?;
        let result = pairing
            .UnpairAsync()
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let st = result.Status().map_err(|e| e.to_string())?;
        if st == DeviceUnpairingResultStatus::Unpaired
            || st == DeviceUnpairingResultStatus::AlreadyUnpaired
        {
            Ok(())
        } else {
            Err(format!("Failed to remove device ({st:?})"))
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit("bt-changed", ());
    res
}

// ---------- Discovery (nearby unpaired devices) ----------

fn stop_watchers(state: &BtState) {
    if let Ok(mut list) = state.watchers.lock() {
        for w in list.drain(..) {
            if let Ok(st) = w.Status() {
                if matches!(st, DeviceWatcherStatus::Started | DeviceWatcherStatus::EnumerationCompleted) {
                    let _ = w.Stop();
                }
            }
        }
    }
}

fn emit_nearby(app: &AppHandle, nearby: &Arc<Mutex<HashMap<String, BtNearbyDevice>>>) {
    if let Ok(map) = nearby.lock() {
        let mut list: Vec<BtNearbyDevice> = map.values().cloned().collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let _ = app.emit("bt-nearby-changed", list);
    }
}

#[tauri::command]
pub fn bt_scan_start(state: State<'_, BtState>, app: AppHandle) -> Result<(), String> {
    init_com();
    stop_watchers(&state);
    if let Ok(mut n) = state.nearby.lock() {
        n.clear();
    }
    let _ = app.emit("bt-nearby-changed", Vec::<BtNearbyDevice>::new());

    let req = hstring_iterable(&[PROP_CATEGORY, PROP_CAN_PAIR, PROP_IS_PAIRED]);
    let selectors = [
        (BluetoothDevice::GetDeviceSelectorFromPairingState(false).ok(), false),
        (BluetoothLEDevice::GetDeviceSelectorFromPairingState(false).ok(), true),
    ];

    let mut started = Vec::new();
    for (sel_opt, is_le) in selectors {
        let Some(sel) = sel_opt else { continue };
        let Ok(watcher) = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
            &sel,
            &req,
            DeviceInformationKind::AssociationEndpoint,
        ) else {
            continue;
        };

        let nearby_add = state.nearby.clone();
        let app_add = app.clone();
        let _ = watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
            move |_, dev| {
                let Some(dev) = dev.as_ref() else { return Ok(()) };
                let id = dev.Id().map(|s| s.to_string()).unwrap_or_default();
                let name = dev.Name().map(|s| s.to_string()).unwrap_or_default();
                if id.is_empty() || name.is_empty() {
                    return Ok(());
                }
                let props = dev.Properties().ok();
                let can_pair = props
                    .as_ref()
                    .and_then(|p| prop_bool(p, PROP_CAN_PAIR))
                    .unwrap_or(true);
                let is_paired = props
                    .as_ref()
                    .and_then(|p| prop_bool(p, PROP_IS_PAIRED))
                    .unwrap_or(false);
                if is_paired {
                    return Ok(());
                }
                let cat = classify_device(
                    props.as_ref().and_then(|p| prop_string(p, PROP_CATEGORY)).as_deref(),
                    None,
                    &name,
                );
                if let Ok(mut map) = nearby_add.lock() {
                    map.insert(
                        id.clone(),
                        BtNearbyDevice {
                            id,
                            name,
                            category: cat.into(),
                            is_le,
                            can_pair,
                        },
                    );
                }
                emit_nearby(&app_add, &nearby_add);
                Ok(())
            },
        ));

        let nearby_upd = state.nearby.clone();
        let app_upd = app.clone();
        let _ = watcher.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
            move |_, upd| {
                let Some(upd) = upd.as_ref() else { return Ok(()) };
                let id = upd.Id().map(|s| s.to_string()).unwrap_or_default();
                if let Ok(props) = upd.Properties() {
                    if prop_bool(&props, PROP_IS_PAIRED) == Some(true) {
                        if let Ok(mut map) = nearby_upd.lock() {
                            if map.remove(&id).is_some() {
                                drop(map);
                                emit_nearby(&app_upd, &nearby_upd);
                            }
                        }
                    }
                }
                Ok(())
            },
        ));

        let nearby_rem = state.nearby.clone();
        let app_rem = app.clone();
        let _ = watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(
            move |_, upd| {
                let Some(upd) = upd.as_ref() else { return Ok(()) };
                let id = upd.Id().map(|s| s.to_string()).unwrap_or_default();
                if let Ok(mut map) = nearby_rem.lock() {
                    if map.remove(&id).is_some() {
                        drop(map);
                        emit_nearby(&app_rem, &nearby_rem);
                    }
                }
                Ok(())
            },
        ));

        if watcher.Start().is_ok() {
            started.push(watcher);
        }
    }

    if let Ok(mut list) = state.watchers.lock() {
        *list = started;
    }
    Ok(())
}

#[tauri::command]
pub fn bt_scan_stop(state: State<'_, BtState>) {
    stop_watchers(&state);
}

// ---------- Custom Pairing (Confirm / PIN / Passkey) ----------

#[tauri::command]
pub fn bt_pair_respond(accept: bool, pin: Option<String>, state: State<'_, BtState>) {
    if let Ok(mut slot) = state.pair_tx.lock() {
        if let Some(tx) = slot.take() {
            let _ = tx.send((accept, pin));
        }
    }
}

#[tauri::command]
pub async fn bt_pair(id: String, name: String, state: State<'_, BtState>, app: AppHandle) -> Result<(), String> {
    // Cancel any leftover pairing prompt responder.
    if let Ok(mut slot) = state.pair_tx.lock() {
        if let Some(tx) = slot.take() {
            let _ = tx.send((false, None));
        }
    }

    let app_for_task = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        init_com();
        let dev = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id.as_str()))
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        let custom = dev
            .Pairing()
            .and_then(|p| p.Custom())
            .map_err(|e| e.to_string())?;

        let app_ev = app_for_task.clone();
        let dev_id = id.clone();
        let dev_name = name.clone();
        let token = custom
            .PairingRequested(&TypedEventHandler::<_, DevicePairingRequestedEventArgs>::new(
                move |_, args| {
                    let Some(args) = args.as_ref() else { return Ok(()) };
                    let kind = args.PairingKind()?;
                    if kind == DevicePairingKinds::ConfirmOnly {
                        let _ = args.Accept();
                    } else if kind == DevicePairingKinds::DisplayPin {
                        let pin = args.Pin().map(|s| s.to_string()).ok();
                        let _ = app_ev.emit(
                            "bt-pair-prompt",
                            BtPairPrompt {
                                id: dev_id.clone(),
                                name: dev_name.clone(),
                                kind: "display_pin".into(),
                                pin,
                            },
                        );
                        let _ = args.Accept();
                    } else if kind == DevicePairingKinds::ConfirmPinMatch || kind == DevicePairingKinds::ProvidePin {
                        let deferral = args.GetDeferral()?;
                        let pin = if kind == DevicePairingKinds::ConfirmPinMatch {
                            args.Pin().map(|s| s.to_string()).ok()
                        } else {
                            None
                        };
                        let (tx, rx) = mpsc::sync_channel::<(bool, Option<String>)>(1);
                        if let Some(st) = tauri::Manager::try_state::<BtState>(&app_ev) {
                            if let Ok(mut slot) = st.pair_tx.lock() {
                                *slot = Some(tx);
                            }
                        }
                        let _ = app_ev.emit(
                            "bt-pair-prompt",
                            BtPairPrompt {
                                id: dev_id.clone(),
                                name: dev_name.clone(),
                                kind: if kind == DevicePairingKinds::ConfirmPinMatch {
                                    "confirm_pin".into()
                                } else {
                                    "provide_pin".into()
                                },
                                pin,
                            },
                        );
                        let args_clone = args.clone();
                        std::thread::spawn(move || {
                            if let Ok((accept, user_pin)) = rx.recv_timeout(Duration::from_secs(60)) {
                                if accept {
                                    if let Some(p) = user_pin {
                                        let _ = args_clone.AcceptWithPin(&HSTRING::from(p.as_str()));
                                    } else {
                                        let _ = args_clone.Accept();
                                    }
                                }
                            }
                            let _ = deferral.Complete();
                        });
                    }
                    Ok(())
                },
            ))
            .map_err(|e| e.to_string())?;

        let kinds = DevicePairingKinds::ConfirmOnly
            | DevicePairingKinds::DisplayPin
            | DevicePairingKinds::ProvidePin
            | DevicePairingKinds::ConfirmPinMatch;
        let result = custom
            .PairWithProtectionLevelAsync(kinds, DevicePairingProtectionLevel::Default)
            .and_then(|op| op.join())
            .map_err(|e| e.to_string());
        let _ = custom.RemovePairingRequested(token);

        let pair_res = result?;
        let st = pair_res.Status().map_err(|e| e.to_string())?;
        if st == DevicePairingResultStatus::Paired || st == DevicePairingResultStatus::AlreadyPaired {
            Ok(())
        } else {
            Err(format!("Pairing failed ({st:?})"))
        }
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit("bt-pair-prompt", Option::<BtPairPrompt>::None);
    let _ = app.emit("bt-changed", ());
    res
}
