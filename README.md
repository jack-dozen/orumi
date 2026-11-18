# ORUMI

<!-- Logo placeholder -->
<!-- <p align="center"><img src="assets/logo.png" alt="ORUMI logo" width="180"></p> -->

[![Status](https://img.shields.io/badge/status-development%20%2F%20testing-orange)](https://github.com/jack-dozen/orumi)
[![Platform](https://img.shields.io/badge/platform-Windows%2011-blue)](https://github.com/jack-dozen/orumi)
[![Stack](https://img.shields.io/badge/built%20with-Tauri%202%20%2B%20Rust%20%2B%20React-informational)](https://github.com/jack-dozen/orumi)
[![License](https://img.shields.io/badge/license-GPL--3.0-green)](LICENSE)

> A custom edge-revealed desktop shell and tiling workspace environment for Windows 11, heavily inspired by **Caelestia**.

---

## Overview

**ORUMI** replaces the traditional Windows desktop clutter with sleek, edge-revealed overlays built on **Tauri 2**, **Rust**, and **React**. Every panel stays hidden at the edge of the screen until summoned via hover, click, or hotkey—dynamically themed around your current wallpaper using Material You color extraction.

Paired out-of-the-box with [**vscroll-wm**](vscroll-wm/) (a vertical scrolling tiling window manager for Windows) over Named Pipe IPC, ORUMI brings a cohesive, Linux-inspired rice workflow to Windows 11 without sacrificing native system controls.

<!-- Demo video / GIF placeholder -->
<!-- ![ORUMI Demo](assets/demo.gif) -->
<!-- Or embed a YouTube / showcase link here once recorded -->

---

## Motivation

Desktop environments like **Caelestia** on Linux demonstrate how clean, responsive, and cohesive an edge-driven shell can feel—combining a minimal dock, a dynamic top notch, an integrated launcher, and Material You theming into one unified experience.

**ORUMI** aims to bring that same visual language and workflow ergonomics to **Windows 11** natively—using Win32/WinRT APIs for system integration (audio, brightness, Wi-Fi, Bluetooth, notifications, SMTC media, and taskbar management) rather than attempting to port Linux code.

---

## Features

### Edge-Revealed Panels
- **Bar (Left Edge):** App launcher trigger, active-workspace window list (icons or dots) with hover thumbnail previews, workspace dot switcher with scroll support, clock, native system tray (`>`) flyout trigger, and quick status indicators (Wi-Fi, volume, battery, power).
- **Notch (Top Center):** Multi-tab control center featuring:
  - **Dashboard:** Clock, weather, calendar, CPU/RAM/disk/GPU gauges, and mini media player.
  - **Media:** Full Windows SMTC media controls, album art, and seek bar.
  - **Notifications:** Custom Windows 11 notification center (`wpndatabase.db` reader) with closed-notch toast popups and optional native banner suppression.
  - **System:** Live CPU, RAM, disk, GPU, Wi-Fi, and top-process monitor.
  - **Quick Settings:** Wi-Fi & Bluetooth panels (connect, disconnect, scan, pair/unpair), Airplane Mode, Do Not Disturb, and volume/brightness sliders.
  - **Workspace OSD:** Compact slide-down indicator when switching workspaces.
- **Launcher (Bottom Center):** Fast app search, `>` command actions (`>settings`, `>switch`, `>WM shortcuts`), an endless wallpaper carousel with live Material You theme generation, and built-in **ORUMI** & **Window Manager** settings.
- **Overlay (Right Center):** Power actions menu, volume/brightness mixer, and automatic OSD popups (hiding the native Windows 11 flyout).
- **Switcher (Bottom Center):** Custom `Alt+Tab` window switcher with real application icons.
- **Activities (`Win` Key):** Start-menu replacement with categorized app grid, 27-dot letter rail, and multi-workspace window overview.
- **WM Overview (Left Edge):** Proportional tiled workspace strip powered by `vscroll-wm` with live/cached thumbnails and drag-and-drop window/workspace reordering.

### Integrated Tiling Window Manager (`vscroll-wm`)
- Runs as a paired out-of-process helper (`vscroll-wm.exe --no-tray`) over Windows Named Pipes (`\\.\pipe\vscroll-wm`) to keep low-level keyboard hooks isolated.
- Includes **Snap Layouts** & **Snap Assist** synced to ORUMI's live Material You color palette.
- Configurable directly inside ORUMI's **Window Manager** settings tab (`gap`, `anim_ms`, `focus_border`, `bind.*` shortcuts, and `vscroll-wm.ignore` rules).
- Optional **Admin Windows** support via a Windows Scheduled Task (`scripts/setup-elevated-wm.ps1`) so elevated windows can be tiled and switched without running the WebView2 UI as administrator.

---

## Getting Started

### Prerequisites (Building from Source)
- **Windows 11** (with Microsoft Edge WebView2 Runtime)
- **Node.js** (npm)
- **Rust** (MSVC toolchain)

### Installation & Development

1. Clone the repository:
   ```powershell
   git clone https://github.com/jack-dozen/orumi.git
   cd orumi
   ```

2. Install frontend dependencies:
   ```powershell
   npm install
   ```

3. Run in development mode (automatically prepares `vscroll-wm` and launches Tauri):
   ```powershell
   npm run tauri dev
   ```

4. Build a release installer:
   ```powershell
   npm run tauri build
   ```

> **Tip:** Always quit ORUMI cleanly via the system tray menu (**Quit**) so its taskbar and notification guard can restore Windows' default taskbar and toast settings. A detached `--guard` process also restores them automatically if ORUMI exits unexpectedly.

---

## Configuration & Shortcuts

ORUMI stores its configuration in:
```text
%APPDATA%\com.example.edgeshell\config.json
```
*(The internal app identifier remains `com.example.edgeshell` from the project's earlier `edgeshell` name.)*

Most visual, behavior, wallpaper, and window manager options can be adjusted live from **Settings** (`>settings` in Launcher or via the tray menu).

### Default Shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl` + `Alt` + `B` | Toggle **Bar** |
| `Ctrl` + `Alt` + `N` | Toggle **Notch** |
| `Ctrl` + `Alt` + `J` | Toggle **Launcher** |
| `Win` *(tap)* | Toggle **Activities** |
| `Alt` + `Tab` | Custom window **Switcher** (when enabled) |
| `Win` + `Tab` / `Win` + `Alt` + `Space` | Toggle **WM Overview** |
| `Win` + `/` | Open **WM Keyboard Shortcuts** cheatsheet |

---

## Project Status & Known Limitations

ORUMI is currently in active development. Core shell panels, Material You theming, taskbar management, Activities, and `vscroll-wm` IPC integration are functional, while newer additions (Wi-Fi/Bluetooth quick panels, custom notification center, Snap Layouts integration, and elevated `vscroll-wm` support) are undergoing runtime testing.

- **Windows 11 only:** Built directly on Win32 and WinRT APIs.
- **Window thumbnails:** Captured via `PrintWindow`; certain hardware-accelerated or protected windows may return blank frames.
- **Elevated windows:** Standard-privilege hooks cannot intercept keys when an administrator window has focus unless the optional elevated `vscroll-wm` Scheduled Task helper is enabled.

---

## Credits & Acknowledgments

ORUMI stands on the shoulders of several open-source projects:

- **Caelestia** — Primary visual and UX inspiration for the edge-revealed shell, notch, launcher, and dynamic theming philosophy (design inspiration only; no code shared).
- **Bloom** *(GPL-3.0)* — Adapted Windows system integration patterns, including OSD flyout hiding, WMI/COM brightness control, window filtering, Wi-Fi/radio toggles, tray chevron reveal, `IShellItemImageFactory` icon extraction, `PrintWindow` capture, and the crash-safe taskbar guard pattern.
- **Astraeus** *(GPL-3.0)* — Feature inspiration and the registry-based `ToastEnabled` Do Not Disturb toggle pattern.
- [**vscroll-wm**](vscroll-wm/) — Integrated vertical scrolling tiling window manager powering ORUMI's workspaces, Snap Layouts, and Overview strip.

---

## License

This project is licensed under the **GNU General Public License v3.0** — see the [LICENSE](LICENSE) file for details.
