#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use tauri::State;
use windows::{
    core::w,
    Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
        PDH_MORE_DATA,
    },
};

const PDH_SUCCESS: u32 = 0;

struct Probe {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    /// Buffer dalam satuan u64 supaya alignment-nya cukup untuk struct PDH.
    buffer: Vec<u64>,
    last_collect: Option<Instant>,
}

// Handle PDH berupa pointer mentah sehingga tidak otomatis Send. Aman di sini karena
// Probe hanya diakses lewat Mutex, jadi tidak pernah dipakai dua thread sekaligus.
unsafe impl Send for Probe {}

impl Probe {
    unsafe fn new() -> Option<Self> {
        let mut query: PDH_HQUERY = std::mem::zeroed();
        if PdhOpenQueryW(None, 0, &mut query) != PDH_SUCCESS {
            return None;
        }
        let mut counter: PDH_HCOUNTER = std::mem::zeroed();
        if PdhAddEnglishCounterW(
            query,
            w!("\\GPU Engine(*)\\Utilization Percentage"),
            0,
            &mut counter,
        ) != PDH_SUCCESS
        {
            let _ = PdhCloseQuery(query);
            return None;
        }
        Some(Self {
            query,
            counter,
            buffer: Vec::new(),
            last_collect: None,
        })
    }

    /// Pemakaian GPU (0-100) dari adapter yang paling sibuk.
    unsafe fn read(&mut self) -> Option<f64> {
        // Persen butuh dua sampel. Kalau pembacaan terakhir sudah lama,
        // ambil sampel baru dulu supaya selisihnya tidak mencakup jeda yang panjang.
        let stale = self
            .last_collect
            .map_or(true, |t| t.elapsed() > Duration::from_secs(3));

        if PdhCollectQueryData(self.query) != PDH_SUCCESS {
            return None;
        }
        if stale {
            std::thread::sleep(Duration::from_millis(400));
            if PdhCollectQueryData(self.query) != PDH_SUCCESS {
                return None;
            }
        }
        self.last_collect = Some(Instant::now());

        // Panggilan pertama hanya untuk mengetahui ukuran buffer.
        let mut bytes: u32 = 0;
        let mut count: u32 = 0;
        let sized = PdhGetFormattedCounterArrayW(
            self.counter,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            None,
        );
        if sized != PDH_MORE_DATA as u32 || bytes == 0 {
            return None;
        }

        let words = (bytes as usize).div_ceil(size_of::<u64>());
        self.buffer.resize(words, 0);

        let status = PdhGetFormattedCounterArrayW(
            self.counter,
            PDH_FMT_DOUBLE,
            &mut bytes,
            &mut count,
            Some(self.buffer.as_mut_ptr().cast()),
        );
        if status != PDH_SUCCESS {
            return None;
        }

        let items = std::slice::from_raw_parts(
            self.buffer.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
            count as usize,
        );

        // (adapter, jenis engine) -> jumlah pemakaian seluruh proses
        let mut sums: HashMap<(String, String), f64> = HashMap::new();
        for item in items {
            let status = item.FmtValue.CStatus;
            if status != 0 && status != 1 {
                continue; // 0 = data valid, 1 = data baru
            }
            let Ok(name) = item.szName.to_string() else { continue };

            // Contoh nama: pid_1234_luid_0x00000000_0x0000ABCD_phys_0_eng_0_engtype_3D
            let luid = name
                .split("_luid_")
                .nth(1)
                .and_then(|s| s.split("_phys_").next())
                .unwrap_or("")
                .to_string();
            let engine = name.rsplit("engtype_").next().unwrap_or("").to_string();

            *sums.entry((luid, engine)).or_insert(0.0) += item.FmtValue.Anonymous.doubleValue;
        }

        sums.values().copied().fold(None, |best: Option<f64>, v| {
            Some(best.map_or(v, |b| b.max(v)))
        })
        .map(|v| v.clamp(0.0, 100.0))
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

pub struct GpuState(Arc<Mutex<Option<Probe>>>);

impl GpuState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

/// None = counter GPU tidak tersedia di PC ini.
#[tauri::command]
pub async fn get_gpu(state: State<'_, GpuState>) -> Result<Option<f64>, String> {
    let shared = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut guard = shared.lock().ok()?;
        if guard.is_none() {
            *guard = unsafe { Probe::new() };
        }
        guard.as_mut().and_then(|p| unsafe { p.read() })
    })
    .await
    .map_err(|e| e.to_string())
}
