use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use std::collections::HashMap;
use serde::Serialize;
use sysinfo::{Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::State;

/// How often to reread disk information.
const DISK_REFRESH: Duration = Duration::from_secs(30);

#[derive(Serialize)]
pub struct Stats {
    cpu: f32,
    mem_used: u64,
    mem_total: u64,
    disk_used: u64,
    disk_total: u64,
    net_down: u64,
    net_up: u64,
}

struct Inner {
    sys: System,
    disk: (u64, u64),
    disk_at: Option<Instant>,
    nets: Option<Networks>,
    net_at: Instant,
}

pub struct StatsState(Mutex<Option<Inner>>);

impl StatsState {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

fn init_inner() -> Inner {
    let mut sys = System::new().expect("gagal menginisialisasi sysinfo");
    // The first CPU reading is always 0, so warm it up first.
    sys.refresh_cpu_usage();
    sys.refresh_memory();
    let nets = Networks::new_with_refreshed_list().ok();
    Inner {
        sys,
        disk: (0, 0),
        disk_at: None,
        nets,
        net_at: Instant::now(),
    }
}

/// (used, total) in bytes for drive C:, or the first drive if C: is unavailable.
fn read_system_disk() -> (u64, u64) {
    let Ok(disks) = Disks::new_with_refreshed_list() else {
        return (0, 0);
    };
    let pick = disks
        .iter()
        .find(|d| d.mount_point().to_string_lossy().to_uppercase().starts_with("C:"))
        .or_else(|| disks.first());
    match pick {
        Some(d) => {
            let total = d.total_space();
            (total.saturating_sub(d.available_space()), total)
        }
        None => (0, 0),
    }
}

/// Async so it does not run on the main thread.
#[tauri::command]
pub async fn get_stats(state: State<'_, StatsState>) -> Result<Stats, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    let inner = guard.get_or_insert_with(init_inner);

    inner.sys.refresh_cpu_usage();
    inner.sys.refresh_memory();

    let stale = inner.disk_at.map_or(true, |t| t.elapsed() >= DISK_REFRESH);
    if stale {
        inner.disk = read_system_disk();
        inner.disk_at = Some(Instant::now());
    }

    let net_elapsed = inner.net_at.elapsed().as_secs_f64();
    inner.net_at = Instant::now();
    let (net_down, net_up) = match inner.nets.as_mut() {
        Some(nets) => {
            nets.refresh(true);
            if (0.2..=2.5).contains(&net_elapsed) {
                let mut rx = 0u64;
                let mut tx = 0u64;
                for (name, data) in nets.iter() {
                    if name.to_string_lossy().to_ascii_lowercase().contains("loopback") {
                        continue;
                    }
                    rx = rx.saturating_add(data.received());
                    tx = tx.saturating_add(data.transmitted());
                }
                (
                    (rx as f64 / net_elapsed) as u64,
                    (tx as f64 / net_elapsed) as u64,
                )
            } else {
                (0, 0)
            }
        }
        None => (0, 0),
    };

    Ok(Stats {
        cpu: inner.sys.global_cpu_usage(),
        mem_used: inner.sys.used_memory(),
        mem_total: inner.sys.total_memory(),
        disk_used: inner.disk.0,
        disk_total: inner.disk.1,
        net_down,
        net_up,
    })
}

// ---------- Top processes ----------

#[derive(Serialize)]
pub struct ProcEntry {
    name: String,
    /// Percentage of total CPU (already divided by the number of cores).
    cpu: f32,
    /// Bytes (working set), total for all processes with this name.
    mem: u64,
    count: u32,
}

/// Process-specific system, lazily allocated while the Notch System tab is active.
pub struct ProcState(Mutex<Option<System>>);

impl ProcState {
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }
}

const TOP_PROCESSES: usize = 6;

#[tauri::command]
pub async fn get_top_processes(state: State<'_, ProcState>) -> Result<Vec<ProcEntry>, String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    let sys = guard
        .get_or_insert_with(|| System::new().expect("gagal menginisialisasi sysinfo"));

    // Use `let _` so this compiles whether the result is () or Result.
    let _ = sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1) as f32;

    // name -> (cpu, memory, process count)
    let mut grouped: HashMap<String, (f32, u64, u32)> = HashMap::new();
    for p in sys.processes().values() {
        if p.pid().as_u32() == 0 {
            continue; // System Idle Process
        }
        let raw = p.name().to_string_lossy();
        let name = raw
            .strip_suffix(".exe")
            .or_else(|| raw.strip_suffix(".EXE"))
            .unwrap_or(&raw)
            .to_string();

        let e = grouped.entry(name).or_insert((0.0, 0, 0));
        e.0 += p.cpu_usage() / cores;
        e.1 += p.memory();
        e.2 += 1;
    }

    let mut list: Vec<ProcEntry> = grouped
        .into_iter()
        .map(|(name, (cpu, mem, count))| ProcEntry { name, cpu, mem, count })
        .collect();
    list.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.mem.cmp(&a.mem))
    });
    list.truncate(TOP_PROCESSES);
    Ok(list)
}

/// Release the cached `sysinfo` process table when the Notch closes so idle memory drops.
#[tauri::command]
pub fn release_stats_cache(procs: State<'_, ProcState>) {
    if let Ok(mut guard) = procs.0.lock() {
        *guard = None;
    }
    crate::trim_working_set();
}
