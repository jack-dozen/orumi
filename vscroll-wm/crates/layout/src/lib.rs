//! Layout engine: strip vertikal dengan dua lajur. Murni logika, tanpa Win32.
//!
//! Tiap window punya lebar (`Span`: penuh, lajur kiri, lajur kanan) dan tinggi
//! sendiri (1/3, 1/2, penuh). Window disusun berurutan seperti aliran teks:
//!  - window `Full` ditaruh di bawah kedua lajur,
//!  - window `Left` / `Right` ditumpuk di lajur masing-masing.
//! Tinggi tiap window independen. Viewport meng-scroll mengikuti fokus.
//!
//! Aksi: snap (tinggi / sisi), maximize, focus, dan move_window (tukar posisi
//! dengan tetangga di arah tertentu, apa pun ukurannya).

mod flow;
mod ops;
#[cfg(test)]
mod tests;
mod types;
mod workspaces;

pub use types::*;
pub use workspaces::{SavedLayout, SavedStrip, Workspaces};
