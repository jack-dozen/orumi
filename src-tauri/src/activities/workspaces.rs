//! Workspace interface for Activities, backed by `vscroll-wm` over Named Pipe IPC (`wm_ipc`).

use serde::Serialize;

#[derive(Clone, Serialize)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub active: bool,
    /// hwnds of all windows in this workspace.
    pub windows: Vec<i64>,
}

pub trait WorkspaceProvider: Send + Sync {
    fn list(&self) -> Vec<Workspace>;
    fn switch(&self, id: &str) -> Result<(), String>;
    fn move_window(&self, hwnd: i64, id: &str) -> Result<(), String>;
}

/// Stored via `app.manage`.
pub struct WorkspaceState(pub Box<dyn WorkspaceProvider>);

impl WorkspaceState {
    pub fn new() -> Self {
        Self(Box::new(VScrollIpcProvider))
    }
}

pub struct VScrollIpcProvider;

impl VScrollIpcProvider {
    fn index_of(id: &str) -> Result<usize, String> {
        id.parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .ok_or_else(|| format!("workspace '{id}' does not exist"))
    }
}

impl WorkspaceProvider for VScrollIpcProvider {
    fn list(&self) -> Vec<Workspace> {
        crate::wm_ipc::list_workspaces()
            .into_iter()
            .map(|w| {
                let n = (w.index + 1).to_string();
                Workspace { id: n.clone(), name: n, active: w.active, windows: w.windows }
            })
            .collect()
    }

    fn switch(&self, id: &str) -> Result<(), String> {
        let index = Self::index_of(id)?;
        // Keep focus on Activities while it is open; restore_external_foreground_if_ours()
        // transfers focus to the newly active workspace when Activities closes.
        crate::wm_ipc::switch_workspace(index, !super::is_open())
    }

    fn move_window(&self, hwnd: i64, id: &str) -> Result<(), String> {
        let index = Self::index_of(id)?;
        crate::wm_ipc::move_window_to_workspace(hwnd, index)
    }
}
