use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use windows::{
    Media::Control::{
        GlobalSystemMediaTransportControlsSession,
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionMediaProperties,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    },
    Storage::Streams::DataReader,
    Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED},
};

#[derive(Serialize)]
pub struct MediaInfo {
    title: String,
    artist: String,
    album: String,
    source: String,
    playing: bool,
    position_ms: u64,
    duration_ms: u64,
    /// Penanda lagu. Berubah = cover perlu diambil ulang.
    art_key: String,
    /// Cover (data URL). Hanya terisi saat art_key berubah dari yang sudah diketahui React.
    art: Option<String>,
}

/// Sesi media yang sedang aktif menurut Windows, kalau ada.
fn current_session() -> Option<GlobalSystemMediaTransportControlsSession> {
    // Thread pekerja belum tentu sudah menyalakan WinRT.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .ok()?
        .join()
        .ok()?;
    manager.GetCurrentSession().ok()
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Waktu WinRT (100 ns sejak 1601) ke milidetik sejak 1970. 0 kalau tidak valid.
fn winrt_to_unix_ms(ticks: i64) -> u64 {
    const EPOCH_DIFF: i64 = 116_444_736_000_000_000;
    if ticks < EPOCH_DIFF {
        0
    } else {
        ((ticks - EPOCH_DIFF) / 10_000) as u64
    }
}

fn read_art(props: &GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<String> {
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 8_000_000 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;

    let mime = if buf.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else if buf.starts_with(b"BM") {
        "image/bmp"
    } else {
        "image/png"
    };
    Some(format!("data:{mime};base64,{}", STANDARD.encode(buf)))
}

/// "Spotify.exe" -> "Spotify", "Microsoft.ZuneMusic_xxx!App" -> "Microsoft.ZuneMusic_xxx"
fn pretty_source(raw: &str) -> String {
    let base = raw.split('!').next().unwrap_or(raw);
    base.trim_end_matches(".exe").to_string()
}

fn read_media(known_art_key: Option<String>) -> Option<MediaInfo> {
    let session = current_session()?;
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;

    let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
    let source = session
        .SourceAppUserModelId()
        .map(|s| pretty_source(&s.to_string()))
        .unwrap_or_default();

    let playing = session
        .GetPlaybackInfo()
        .and_then(|i| i.PlaybackStatus())
        .map(|s| s == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false);

    let (position_ms, duration_ms) = match session.GetTimelineProperties() {
        Ok(t) => {
            let ticks = |r: windows::core::Result<windows::Foundation::TimeSpan>| {
                r.map(|d| d.Duration).unwrap_or(0)
            };
            let start = ticks(t.StartTime());
            let end = ticks(t.EndTime());
            let pos = ticks(t.Position());
            let duration = ((end - start).max(0) / 10_000) as u64;
            let mut position = ((pos - start).max(0) / 10_000) as u64;

            // Posisi dari Windows adalah potret saat terakhir diperbarui;
            // kalau sedang diputar, tambahkan waktu yang sudah berlalu.
            if playing {
                if let Ok(updated) = t.LastUpdatedTime() {
                    let updated_ms = winrt_to_unix_ms(updated.UniversalTime);
                    if updated_ms > 0 {
                        position += unix_now_ms().saturating_sub(updated_ms);
                    }
                }
            }
            if duration > 0 {
                position = position.min(duration);
            }
            (position, duration)
        }
        Err(_) => (0, 0),
    };

    let base_key = format!("{title}|{artist}|{album}");
    let (art_key, art) = if known_art_key.as_deref() == Some(base_key.as_str()) {
        (base_key, None) // cover sudah dimiliki React
    } else {
        match read_art(&props) {
            Some(a) => (base_key, Some(a)),
            // Cover kadang telat muncul setelah lagu ganti: coba lagi di polling berikutnya.
            None => (format!("{base_key}#noart"), None),
        }
    };

    Some(MediaInfo {
        title,
        artist,
        album,
        source,
        playing,
        position_ms,
        duration_ms,
        art_key,
        art,
    })
}

#[tauri::command]
pub async fn get_media(known_art_key: Option<String>) -> Result<Option<MediaInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || read_media(known_art_key))
        .await
        .map_err(|e| e.to_string())
}

/// action: "toggle" | "next" | "prev"
#[tauri::command]
pub async fn media_control(action: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(session) = current_session() else { return };
        match action.as_str() {
            "toggle" => {
                let _ = session.TryTogglePlayPauseAsync().and_then(|op| op.join());
            }
            "next" => {
                let _ = session.TrySkipNextAsync().and_then(|op| op.join());
            }
            "prev" => {
                let _ = session.TrySkipPreviousAsync().and_then(|op| op.join());
            }
            _ => {}
        }
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn media_seek(position_ms: u64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(session) = current_session() else { return };
        let ticks = (position_ms as i64).saturating_mul(10_000);
        let _ = session
            .TryChangePlaybackPositionAsync(ticks)
            .and_then(|op| op.join());
    })
    .await
    .map_err(|e| e.to_string())
}
