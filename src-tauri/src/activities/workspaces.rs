//! Antarmuka workspace untuk Activities.
//!
//! UI hanya bicara lewat `WorkspaceProvider`. Sekarang isinya `StaticProvider` (palsu:
//! workspace disimpan di memori dan TIDAK menyembunyikan window sungguhan). Saat window
//! manager sudah dipilih, cukup tulis provider baru yang bicara lewat IPC-nya dan ganti
//! satu baris di `WorkspaceState::new()`. UI dan command tidak perlu diubah.

use std::{collections::HashMap, sync::Mutex};

use serde::Serialize;

use crate::winlist;

#[derive(Clone, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub active: bool,
    /// hwnd semua window di workspace ini.
    pub windows: Vec<i64>,
}

pub trait WorkspaceProvider: Send + Sync {
    fn list(&self) -> Vec<Workspace>;
    fn switch(&self, id: &str) -> Result<(), String>;
    fn move_window(&self, hwnd: i64, id: &str) -> Result<(), String>;
}

/// Dititipkan lewat `app.manage`.
pub struct WorkspaceState(pub Box<dyn WorkspaceProvider>);

impl WorkspaceState {
    pub fn new() -> Self {
        // Ganti di sini dengan provider IPC window manager nanti.
        Self(Box::new(StaticProvider::new(4)))
    }
}

// ---------- Provider palsu ----------

struct Inner {
    /// Indeks workspace aktif.
    active: usize,
    /// hwnd -> indeks workspace. Window baru masuk ke workspace yang aktif saat pertama terlihat.
    assigned: HashMap<i64, usize>,
}

pub struct StaticProvider {
    count: usize,
    inner: Mutex<Inner>,
}

impl StaticProvider {
    pub fn new(count: usize) -> Self {
        Self {
            count: count.max(1),
            inner: Mutex::new(Inner { active: 0, assigned: HashMap::new() }),
        }
    }

    fn index_of(&self, id: &str) -> Result<usize, String> {
        id.parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|i| *i < self.count)
            .ok_or_else(|| format!("workspace '{id}' tidak ada"))
    }
}

impl WorkspaceProvider for StaticProvider {
    fn list(&self) -> Vec<Workspace> {
        let live: Vec<i64> = winlist::collect_windows().into_iter().map(|w| w.hwnd).collect();
        let Ok(mut st) = self.inner.lock() else { return Vec::new() };

        st.assigned.retain(|hwnd, _| live.contains(hwnd));
        let active = st.active;
        for hwnd in &live {
            st.assigned.entry(*hwnd).or_insert(active);
        }

        (0..self.count)
            .map(|i| Workspace {
                id: (i + 1).to_string(),
                name: (i + 1).to_string(),
                active: i == st.active,
                windows: live
                    .iter()
                    .copied()
                    .filter(|h| st.assigned.get(h) == Some(&i))
                    .collect(),
            })
            .collect()
    }

    fn switch(&self, id: &str) -> Result<(), String> {
        let i = self.index_of(id)?;
        self.inner.lock().map_err(|e| e.to_string())?.active = i;
        Ok(())
    }

    fn move_window(&self, hwnd: i64, id: &str) -> Result<(), String> {
        let i = self.index_of(id)?;
        self.inner.lock().map_err(|e| e.to_string())?.assigned.insert(hwnd, i);
        Ok(())
    }
}
