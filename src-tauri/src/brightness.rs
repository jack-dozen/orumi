//! Brightness layar laptop: membaca lewat event WMI (tanpa polling, tanpa proses tambahan),
//! menulis lewat WMI COM langsung. Diadaptasi dari Bloom (GPL-3.0): services.rs.
//! Monitor eksternal (DDC/CI) belum didukung.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        mpsc::{channel, Receiver, Sender},
        OnceLock,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use windows::Win32::System::{
    Com::{
        CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
    },
    Variant::{VariantClear, VARENUM, VARIANT},
    Wmi::{IWbemClassObject, IWbemLocator, WbemLocator, WBEM_GENERIC_FLAG_TYPE},
};
use wmi::{COMLibrary, WMIConnection};

#[derive(Deserialize, Debug)]
#[serde(rename = "WmiMonitorBrightness")]
#[serde(rename_all = "PascalCase")]
struct WmiMonitorBrightness {
    current_brightness: u8,
}

/// Dipicu Windows setiap kali brightness berubah, oleh apa pun.
#[derive(Deserialize, Debug)]
#[serde(rename = "WmiMonitorBrightnessEvent")]
#[serde(rename_all = "PascalCase")]
struct WmiMonitorBrightnessEvent {
    brightness: u8,
}

/// -1 = belum diketahui atau layar tidak mendukung.
static CURRENT: AtomicI32 = AtomicI32::new(-1);
static SUPPORTED: AtomicBool = AtomicBool::new(true);
/// Kapan terakhir kita sendiri mengubah brightness (untuk mengabaikan event gema-nya).
static LAST_OWN_CHANGE_MS: AtomicU64 = AtomicU64::new(0);
static SET_TX: OnceLock<Sender<u32>> = OnceLock::new();

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// None = layar ini tidak mendukung kontrol brightness lewat WMI.
#[tauri::command]
pub fn get_brightness() -> Option<u8> {
    let v = CURRENT.load(Ordering::Relaxed);
    (v >= 0).then_some(v as u8)
}

#[tauri::command]
pub fn set_brightness(percent: u8) {
    if !SUPPORTED.load(Ordering::Relaxed) {
        return;
    }
    let p = percent.min(100);
    LAST_OWN_CHANGE_MS.store(now_ms(), Ordering::Relaxed);
    CURRENT.store(p as i32, Ordering::Relaxed);
    if let Some(tx) = SET_TX.get() {
        let _ = tx.send(p as u32);
    }
}

/// Memulai thread pembaca (event) dan thread penulis.
pub fn start_watcher(app: AppHandle) {
    let (tx, rx) = channel::<u32>();
    let _ = SET_TX.set(tx);
    thread::spawn(move || unsafe { set_worker(rx) });

    thread::spawn(move || {
        let Ok(com) = COMLibrary::new() else {
            SUPPORTED.store(false, Ordering::Relaxed);
            return;
        };
        let Ok(con) = WMIConnection::with_namespace_path("root\\WMI", com) else {
            SUPPORTED.store(false, Ordering::Relaxed);
            return;
        };

        let mut last: i32 = match con.query::<WmiMonitorBrightness>() {
            Ok(rows) => match rows.first() {
                Some(b) => b.current_brightness as i32,
                None => {
                    SUPPORTED.store(false, Ordering::Relaxed);
                    return; // tidak ada panel yang bisa diatur (PC desktop)
                }
            },
            Err(_) => {
                SUPPORTED.store(false, Ordering::Relaxed);
                return;
            }
        };
        CURRENT.store(last, Ordering::Relaxed);

        let mut on_change = |b: i32| {
            if b == last {
                return;
            }
            last = b;
            CURRENT.store(b, Ordering::Relaxed);
            // Gema dari slider kita sendiri tidak perlu memunculkan popup.
            if now_ms().saturating_sub(LAST_OWN_CHANGE_MS.load(Ordering::Relaxed)) < 2000 {
                return;
            }
            let _ = app.emit("brightness-changed", b as u8);
            crate::overlay::show_popup(&app, "brightness");
        };

        // Tombol brightness ditangani Windows/firmware; ikuti perubahannya lewat event.
        if let Ok(events) = con.raw_notification::<WmiMonitorBrightnessEvent>(
            "SELECT * FROM WmiMonitorBrightnessEvent",
        ) {
            for event in events {
                let Ok(event) = event else { break };
                on_change(event.brightness as i32);
            }
        }

        // Cadangan kalau langganan event tidak tersedia atau berhenti.
        loop {
            thread::sleep(Duration::from_millis(1500));
            if let Ok(rows) = con.query::<WmiMonitorBrightness>() {
                if let Some(b) = rows.first() {
                    on_change(b.current_brightness as i32);
                }
            }
        }
    });
}

/// Thread penulis: hanya nilai terbaru yang dipakai, karena tiap tulis lambat.
unsafe fn set_worker(rx: Receiver<u32>) {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

    let locator: IWbemLocator = match CoCreateInstance(&WbemLocator, None, CLSCTX_ALL) {
        Ok(l) => l,
        Err(_) => {
            let _ = CoUninitialize();
            return;
        }
    };
    let ns = windows::core::BSTR::from("root\\WMI");
    let empty = windows::core::BSTR::new();
    let services = match locator.ConnectServer(&ns, &empty, &empty, &empty, 0, &empty, None) {
        Ok(s) => s,
        Err(_) => {
            let _ = CoUninitialize();
            return;
        }
    };

    // Tanpa impersonation di proxy, WMI menolak dengan WBEM_E_ACCESS_DENIED.
    // 10 = RPC_C_AUTHN_WINNT, 0 = RPC_C_AUTHZ_NONE.
    let _ = CoSetProxyBlanket(
        &services,
        10,
        0,
        windows::core::PCWSTR::null(),
        RPC_C_AUTHN_LEVEL_CALL,
        RPC_C_IMP_LEVEL_IMPERSONATE,
        None,
        windows::Win32::System::Com::EOAC_NONE,
    );
    let _ = EOAC_NONE;

    // GetMethod hanya bekerja pada definisi kelas, bukan pada instance.
    let params_class = {
        let mut class_def: Option<IWbemClassObject> = None;
        let mut in_cls: Option<IWbemClassObject> = None;
        if services
            .GetObject(
                &windows::core::BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None,
                Some(&mut class_def),
                None,
            )
            .is_ok()
        {
            if let Some(ref class_def) = class_def {
                let _ = class_def.GetMethod(
                    windows::core::w!("WmiSetBrightness"),
                    0i32,
                    &mut in_cls,
                    std::ptr::null_mut(),
                );
            }
        }
        in_cls
    };

    while let Ok(mut value) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            value = newer;
        }
        let value = value.min(100);

        let wql = windows::core::BSTR::from("WQL");
        let q = windows::core::BSTR::from("SELECT * FROM WmiMonitorBrightnessMethods");
        let Ok(rows) = services.ExecQuery(&wql, &q, WBEM_GENERIC_FLAG_TYPE(0), None) else {
            continue;
        };

        let mut row = [None::<IWbemClassObject>; 1];
        let mut returned = 0u32;
        while rows.Next(-1i32, &mut row, &mut returned).is_ok() && returned > 0 {
            let Some(obj) = row[0].take() else { continue };

            let mut var = VARIANT::default();
            if obj
                .Get(windows::core::w!("__RELPATH"), 0i32, &mut var, None, None)
                .is_err()
            {
                continue;
            }
            let relpath = var.Anonymous.Anonymous.Anonymous.bstrVal.to_string();
            let _ = VariantClear(&mut var);
            if relpath.is_empty() {
                continue;
            }
            let Some(ref in_cls) = params_class else { continue };
            let Ok(in_params) = in_cls.SpawnInstance(0i32) else { continue };

            let mut b_var = VARIANT::default();
            let b = &mut b_var.Anonymous.Anonymous;
            b.vt = VARENUM(17); // VT_UI1
            b.Anonymous.bVal = value as u8;
            let _ = in_params.Put(windows::core::w!("Brightness"), 0i32, &b_var, 0);

            let mut t_var = VARIANT::default();
            let t = &mut t_var.Anonymous.Anonymous;
            t.vt = VARENUM(3); // VT_I4
            t.Anonymous.lVal = 0i32;
            let _ = in_params.Put(windows::core::w!("Timeout"), 0i32, &t_var, 0);

            let _ = services.ExecMethod(
                &windows::core::BSTR::from(relpath.as_str()),
                &windows::core::BSTR::from("WmiSetBrightness"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None,
                Some(&in_params),
                None,
                None,
            );
        }
    }
    let _ = CoUninitialize();
}
