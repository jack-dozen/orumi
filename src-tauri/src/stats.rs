use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use std::collections::HashMap;
use serde::Serialize;
use sysinfo::{Disks, ProcessRefreshKind, ProcessesToUpdate, System};
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
}

struct Inner {
    sys: System,
    disk: (u64, u64),
    disk_at: Option<Instant>,
}

pub struct StatsState(Mutex<Inner>);

impl StatsState {
    pub fn new() -> Self {
        let mut sys = System::new().expect("gagal menginisialisasi sysinfo");
        // The first CPU reading is always 0, so warm it up first.
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self(Mutex::new(Inner {
            sys,
            disk: (0, 0),
            disk_at: None,
        }))
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
    let mut inner = state.0.lock().map_err(|e| e.to_string())?;

    inner.sys.refresh_cpu_usage();
    inner.sys.refresh_memory();

    let stale = inner.disk_at.map_or(true, |t| t.elapsed() >= DISK_REFRESH);
    if stale {
        inner.disk = read_system_disk();
        inner.disk_at = Some(Instant::now());
    }

    Ok(Stats {
        cpu: inner.sys.global_cpu_usage(),
        mem_used: inner.sys.used_memory(),
        mem_total: inner.sys.total_memory(),
        disk_used: inner.disk.0,
        disk_total: inner.disk.1,
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

/// Process-specific system, separate so dashboard statistics do not also scan processes.
pub struct ProcState(Mutex<System>);

impl ProcState {
    pub fn new() -> Self {
        Self(Mutex::new(System::new().expect("gagal menginisialisasi sysinfo")))
    }
}

const TOP_PROCESSES: usize = 6;

#[tauri::command]
pub async fn get_top_processes(state: State<'_, ProcState>) -> Result<Vec<ProcEntry>, String> {
    let mut sys = state.0.lock().map_err(|e| e.to_string())?;

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
