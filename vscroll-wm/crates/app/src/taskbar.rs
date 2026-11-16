//! Sembunyikan / tampilkan tombol taskbar window milik proses lain lewat ITaskbarList.
//!
//! Mengikuti LeopardWM (crates/platform_win32/src/taskbar.rs): DWM cloak ditolak
//! (E_ACCESSDENIED) untuk window proses lain dan memarkir di luar layar saja tidak
//! menghapus tombolnya, jadi DeleteTab / AddTab satu-satunya cara yang jalan.
//! ITaskbarList berjenis apartment-threaded, jadi satu thread STA khusus
//! memilikinya dan menerima perintah lewat kanal. Best-effort: kalau gagal, semua
//! fungsi di sini tidak berbuat apa-apa (window tetap bisa dipakai, hanya tombolnya
//! tetap tampil).
//!
//! TEBAKAN (belum diverifikasi di sini): perilaku DeleteTab untuk window proses lain,
//! dan apakah Explorer menambahkan tombolnya kembali saat window berubah state.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};

enum Cmd {
    /// Hapus tombol (hanya sekali per window sampai ditampilkan lagi).
    Hide(isize),
    /// Kembalikan tombol, hanya kalau kita yang menyembunyikannya.
    Show(isize),
    /// Kembalikan tombol tanpa syarat (sisa dari proses sebelumnya yang mati paksa).
    Restore(isize),
    /// Window sudah ditutup: lupakan tanpa AddTab (HWND bisa didaur ulang).
    Forget(isize),
    /// Kembalikan semua tombol yang kita sembunyikan (jeda).
    ShowAll,
}

struct Worker {
    tx: Sender<Cmd>,
    thread: JoinHandle<()>,
}

static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

/// Mulai thread COM. Panggil sekali saat start.
pub fn init() {
    let (tx, rx) = mpsc::channel();
    match std::thread::Builder::new()
        .name("taskbar".into())
        .spawn(move || run(rx))
    {
        Ok(thread) => {
            if let Ok(mut g) = WORKER.lock() {
                *g = Some(Worker { tx, thread });
            }
        }
        Err(e) => eprintln!("gagal membuat thread taskbar: {e}"),
    }
}

fn send(cmd: Cmd) {
    if let Ok(g) = WORKER.lock() {
        if let Some(w) = g.as_ref() {
            let _ = w.tx.send(cmd);
        }
    }
}

pub fn hide(h: HWND) {
    send(Cmd::Hide(h.0 as isize));
}

pub fn show(h: HWND) {
    send(Cmd::Show(h.0 as isize));
}

pub fn restore(h: HWND) {
    send(Cmd::Restore(h.0 as isize));
}

pub fn forget(h: HWND) {
    send(Cmd::Forget(h.0 as isize));
}

pub fn show_all() {
    send(Cmd::ShowAll);
}

/// Hentikan thread; semua tombol yang kita sembunyikan dikembalikan dulu. Menunggu
/// maksimal ~500 ms. Aman dipanggil dari panic hook (try_lock) dan berulang kali.
pub fn shutdown() {
    let worker = WORKER.try_lock().ok().and_then(|mut g| g.take());
    if let Some(w) = worker {
        drop(w.tx); // kanal tertutup -> run() mengembalikan tombol lalu keluar
        for _ in 0..50 {
            if w.thread.is_finished() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn hwnd(key: isize) -> HWND {
    HWND(key as *mut c_void)
}

fn run(rx: Receiver<Cmd>) {
    unsafe {
        // STA: ITaskbarList apartment-threaded
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let list: ITaskbarList = match CoCreateInstance(&TaskbarList, None, CLSCTX_ALL) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("CoCreateInstance(TaskbarList) gagal: {e}");
                CoUninitialize();
                return;
            }
        };
        if let Err(e) = list.HrInit() {
            eprintln!("ITaskbarList::HrInit gagal: {e}");
            CoUninitialize();
            return;
        }
        let mut hidden: HashSet<isize> = HashSet::new();
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Hide(k) => {
                    if hidden.insert(k) {
                        let _ = list.DeleteTab(hwnd(k));
                    }
                }
                Cmd::Show(k) => {
                    if hidden.remove(&k) {
                        let _ = list.AddTab(hwnd(k));
                    }
                }
                Cmd::Restore(k) => {
                    hidden.remove(&k);
                    let _ = list.AddTab(hwnd(k));
                }
                Cmd::Forget(k) => {
                    hidden.remove(&k);
                }
                Cmd::ShowAll => {
                    for k in hidden.drain() {
                        let _ = list.AddTab(hwnd(k));
                    }
                }
            }
        }
        // kanal ditutup: kembalikan semua tombol yang masih kita sembunyikan
        for k in hidden.drain() {
            let _ = list.AddTab(hwnd(k));
        }
        drop(list);
        CoUninitialize();
    }
}
