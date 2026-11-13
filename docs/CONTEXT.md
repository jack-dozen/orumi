# ORUMI — Project Context (blueprint and roadmap)

This file is the single source of context for ORUMI. It replaces the older `edgeshell-handoff-v2.md` and
`HANDOFF-activities.md`. It is written mainly for Claude in a fresh chat, but it doubles as the project blueprint
and roadmap for the owner.

- Last updated: 2026-11-13
- Code snapshot this file was checked against: `orumi-v0_1_3` archive (the code is the source of truth; if this file and the code disagree, trust the code and fix this file)
- Chat language with the owner: **Indonesian**. This file, code comments, and commit messages: **English** (see section 2).

How to use this file in a new chat: upload this file **and the latest code archive**, then ask Claude to verify the
key claims against the code before building on them (see section 8).

---

## 1. Project summary and tech stack

### 1.1 What ORUMI is

A custom desktop shell for Windows 11. Every menu is revealed from a screen edge. Visual design is inspired by
Caelestia shell (Linux); only the design is borrowed, not code. The project was previously called **edgeshell**;
the rename to ORUMI is not finished in the code (see section 4). License: GPL-3.0.

Owner environment: Windows 11 laptop, AMD GPU, Rust (MSVC toolchain), Node, VS Code, PowerShell.
Code lives on the owner's machine; Claude only sees what is uploaded.

A separate project, **vscroll-wm** (Rust, vertical scrolling tiling window manager for Windows), runs side by side
with ORUMI. Its repo stays standalone. Merging it into ORUMI (idea: git submodule) is planned for the **very end**,
not now.

### 1.2 Stack

- Tauri 2.12 (wry 0.57), React 19, TypeScript, Vite, npm
- Rust edition 2024, crate `windows` 0.62, `sysinfo` 0.40, `wmi` 0.14, `image` (jpeg/png/bmp), `rayon`, `base64`, `serde_json`
- Tauri plugins: `global-shortcut`, `single-instance`, `opener`
- Theme: `@material/material-color-utilities`; icon font: subset Material Symbols Rounded
- UI runs in WebView2

Commands:

- Dev: `npm run tauri dev` (rebuilds Rust automatically). Quit properly via tray menu → Quit.
- If `tauri` is "not recognized": `node_modules` is broken; close all related processes, then `npm install`.
- Icon font rebuild (after adding any new `Icon` name to `$icons` in the script):
  `powershell -ExecutionPolicy Bypass -File scripts\make-icon-font.ps1`

### 1.3 Windows (all transparent, frameless, always on top, not in taskbar, always existing)

Hidden means click-through plus content slid out via CSS. All windows load the same frontend;
`getCurrentWindow().label` picks the component in `App.tsx`. There are **seven** windows.

| Label | Edge | Size (design units) | Notes |
|---|---|---|---|
| bar | left | width 64, full height minus 12 inset | hover (200 ms dwell), click, or hotkey; does not take focus |
| notch | top center | 820 × 380 (trigger zone 240) | tabs: dashboard / media / system / settings (Windows quick settings); no focus except while typing |
| launcher | bottom center | 1040 × 540 | needs focus; `>` actions, `>settings` (ORUMI app/color settings), `@` windows, wallpaper carousel |
| overlay | right center | 150 × 440 | power pill, volume/brightness mixer, automatic popups |
| switcher | bottom center | 1200 × 260 | custom Alt+Tab; no focus |
| preview | free (`Edge::Free`) | computed per hover | window thumbnail preview attached to the right of the bar; no focus |
| activities | bottom panel | max 1600 × 860, at most 92% width / 84% height of screen | start-menu replacement; managed by `activities/mod.rs`, **not** in `PANELS` |

Effective size = design units × monitor DPI scale × `ui_scale` (default 0.8, applied live via `set_zoom`).
All windows must use identical `additionalBrowserArgs`
(`--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --process-per-site`) or WebView2 refuses.
`app.security.assetProtocol` is enabled; thumbnails, icons, and previews load via `convertFileSrc`, and Rust grants
per-file access with `asset_protocol_scope().allow_file`.

### 1.4 Edge engine (`edge.rs`)

One thread watches the cursor (adaptive interval: 30 ms near an edge or with an open panel, 150 ms idle).
Hover panels open after the cursor rests 200 ms in the trigger zone and close 400 ms after it leaves.
`Panels` stores a per-panel "forced open" flag. `TRANSIENT` = opened by a UI click (closes itself after the cursor
enters then leaves, or after 6 s if never entered). Hotkey/tray opens are persistent. `HELD` = in use for typing
(do not close). `SIZE_OVERRIDE` = temporary size (launcher during carousel). `FREE_RECT` + `set_free_rect` =
position/size of the free window (preview). While the cursor is over a visible preview window, the bar stays open
(`over_preview`).

All window changes go through `run_on_main_thread` from a single thread. Calling window APIs from several
high-frequency threads once caused heap corruption (0xc0000374).

### 1.5 Rust modules (`src-tauri/src`)

| File | Purpose |
|---|---|
| `lib.rs` | `PANELS`, `apply_layout`, `place_panel`, setup, tray menu, `generate_handler!`, `--guard` mode at start of `run()`, plugin `orumi-state` that registers all managed state |
| `edge.rs` | edge engine (1.4), `launcher_view`, `notch_set_height`, `panel_activation`, `set_activatable` (WS_EX_NOACTIVATE), `Edge::Free` |
| `config.rs` | `Config` + `config.json`, hotkeys (parse + re-register), `set_config`, `remember_wallpaper`, `toggle_alt_tab`, calls `taskbar::apply` on change |
| `stats.rs`, `gpu.rs` | CPU/RAM/disk (sysinfo) with top processes; GPU usage via PDH "GPU Engine" |
| `wifi.rs`, `battery.rs`, `media.rs` | WLAN API + notifications; `GetSystemPowerStatus`; SMTC (title, cover, controls, seek) |
| `apps.rs`, `actions.rs` | app list via `Get-StartApps` + `launch_app`; lock/sleep/hibernate/shutdown/restart/logout/display off |
| `winlist.rs` | window list (strict filter), `collect_windows`, `activate_window`, WinEvent hook → `windows-changed`; `WindowEntry` has `hwnd`, `title`, `exe`, `minimized`, `focused`, `aumid`, `path` |
| `icons.rs` | app icons via `IShellItemImageFactory` → PNG cache, dedicated COM STA thread; `app_icon(target)` with `aumid:<AppID>` or `path:<exe>`; `ICON_SIZE = 128`, `ICON_VERSION = 2` |
| `previews.rs` | bar hover thumbnails: `PrintWindow` → JPEG in `app_cache_dir/previews/<hwnd>_<seq>.jpg`; `preview_open/keep/close_soon/close` with generation counter `GEN` |
| `overlay.rs`, `brightness.rs`, `osd.rs` | volume (Core Audio) and popups; brightness (WMI events to read, COM worker to write, laptop panel only); hides the Windows 11 volume/brightness flyout |
| `altswitch.rs` | custom Alt+Tab: `WH_KEYBOARD_LL` hook + worker + watchdog; installs after the Activities hook so it runs first in the hook chain; re-installs after launcher close and rescues a missed Alt+Tab by dismissing the native switcher and starting the custom one |
| `wallpaper.rs`, `theme.rs` | wallpaper list, thumbnail cache (rayon, background `warm`), `set_wallpaper`; theme from wallpaper (`wallpaper_pixels` downsized to 128 px in Rust), persist/broadcast theme |
| `tray_flyout.rs` | `>` button in the bar opens the Windows hidden-icons flyout via UI Automation (moved next to the bar, best effort) |
| `quick.rs` | notch quick settings: Wi-Fi / Bluetooth / airplane mode via `Windows.Devices.Radios` (Bloom pattern), Do Not Disturb via `HKCU\...\PushNotifications\ToastEnabled` (Astraeus pattern); commands `quick_get`, `quick_set` |
| `taskbar.rs` | taskbar auto-hide, full hide (stage 2), tray reveal helpers, `--guard` process (see 2.3) |
| `activities/mod.rs` | panel window: `show/hide/dismiss/toggle`, placement via `edge::place`, events `activities-open` / `activities-close`, commands `activities_toggle`, `activities_close`, `ws_list`, `ws_switch`, `ws_move_window` |
| `activities/winkey.rs` | Win key trigger (poller + hook), Start menu suppression |
| `activities/thumbs.rs` | window snapshots + thumbnails (`PrintWindow`, rayon, per-hwnd cache) |
| `activities/workspaces.rs` | `WorkspaceProvider` trait + fake `StaticProvider` (4 in-memory workspaces) |

### 1.6 Frontend (`src`)

`App.tsx` (routing by label + `useTheme`), `App.css` (color tokens, icon `@font-face`, all styles), `theme.ts`
(four base colors → CSS tokens), `config.ts`.
Hooks: `useEdgeVisible`, `useNow`, `useStats`, `useMedia`, `useProcesses`, `useGpu`, `useWifi`, `useBattery`,
`useWindows`, `useWeather`, `useConfig`, `useQuick`.
Components: `Bar`, `Notch`, `Launcher`, `Overlay`, `Switcher`, `Preview`, `Media`, `Weather`, `Gauge`, `Icon`,
`AppIcon` (real app icon with letter fallback), `Settings` (+ `ColorSettings`), `LauncherSettings`, `VolumeBrightness`,
`WallpaperSettings`, `WallpaperPicker`.
Activities lives in `src/activities/` (`Activities.tsx`, `useActivities.ts`, `activities.css`, all classes prefixed `.act`).

Theme: four colors **bg0** (panel), **bg1** (card), **fg** (text), **accent**, each either "wallpaper" (auto) or
"custom", plus a wallpaper color intensity slider; everything else derives from these tokens.

Rust → UI events: `edge-visible:<label>` (bool), `overlay-mode`, `volume-changed`, `brightness-changed`, `notch-tab`,
`launcher-view`, `windows-changed`, `wifi-changed`, `theme-changed`, `theme-refresh`, `config-changed`, `switcher-items`,
`switcher-select`, `preview-data`, `activities-open`, `activities-close`.
`useEdgeVisible` waits two real frames (plus a 500 ms safety) before starting the animation so panels do not "jump".

### 1.7 Config (`%APPDATA%\com.example.edgeshell\config.json`)

`ui_scale`, `weather_city`, `bg_tint`, `colors{bg0,bg1,fg,accent:{mode:"auto"|"custom",value}}`, `wallpaper_dir`,
`wallpaper_switcher` ("builtin"|"external"), `wallpaper_command`, `last_wallpaper` (**written by backend only**),
`alt_tab` (bool), `taskbar_autohide` (bool, default true), `taskbar_hidden` (bool, default false; full hide, takes precedence over autohide), `bar_app_style` ("icons"|"dots"), `hotkeys{bar,notch,launcher}`.
Default hotkeys: Ctrl+Alt+B (bar), Ctrl+Alt+N (notch), Ctrl+Alt+J (launcher). Combinations used by hook-based apps
(e.g. Google Drive) are not detected as conflicts; Windows combinations (Win+…, Ctrl+Alt+Del) cannot be used.
Tray menu: bar, notch, launcher, Settings, custom Alt+Tab on/off, Quit.

### 1.8 Memory profile (last measurement, before the preview and activities windows existed)

About 200–250 MB total (gpu-process 127, renderer 73 shared by all windows, browser 32, utility 9+3, crashpad 2),
down from ~500 MB peak / 300–350 idle. Wallpaper-change spike (~1 GB) is gone. Impact of the 6th and 7th windows is
**not measured**. Measurement method: take `msedgewebview2.exe` processes whose command line contains the app name
(`Win32_Process`), read `Win32_PerfFormattedData_PerfProc_Process.WorkingSetPrivate`, group by `--type=`.
Untried options: `--enable-low-end-device-mode`, hiding real windows when unused (risk of "jump"), `TrySuspend`.
A pure-Rust UI port (Slint) was explored in a separate chat (brief in `edgeshell-slint-spike.md`); no results yet.

### 1.9 Credits and licensing

Code adapted from **Bloom** (GPL-3.0): `osd.rs`, `brightness.rs`, `winlist.rs` filter, `wifi.rs`, battery flag, tray
chevron pattern, icon pattern (`IShellItemImageFactory`), `PrintWindow`, taskbar guard pattern, `Windows.Devices.Radios`
pattern (`commands.rs`, adapted in `quick.rs`). Bloom pins `windows` 0.61 while ORUMI uses 0.62, so adapted snippets may
need API fixes. **Astraeus** (GPL-3.0): feature inspiration and `ToastEnabled` Do Not Disturb switch (`sys_notifs.cpp`,
adapted in `quick.rs`).
**Caelestia**: design only. The **carousel** project has no LICENSE file, so its code must not be copied (only techniques
rewritten). Bloom's source is uploaded as a zip because GitHub blocks automated folder fetching.

---

## 2. Rules and conventions

### 2.1 Communication and workflow

- Chat with the owner in **Indonesian**.
- **Code comments, `context.md`, and commit messages are written in English.** Existing comments are still Indonesian;
  the owner will convert them later. Do not translate existing comments unless asked, but every comment Claude adds
  or rewrites must be English.
- Claude writes the code; the owner builds, tests, and reports (including raw compile errors).
- Claude cannot run the app or compile Rust for Windows. Never claim something "works" before the owner tested it.
  Separate verified facts, assumptions, and untested items. State limitations and risks up front, not at the end.
- One step per turn. At most one question per turn.
- Delivering code: **describe the change as a diff or edit steps instead of repackaging whole files.** New files are
  given in full with full path. No zips.
- When a change touches an existing file outside the feature's own files, list those touch points explicitly (old
  files may change completely later).
- The owner sometimes fixes compile errors with AI in VS Code, so local code can drift from what Claude last saw.
  Work from the latest uploaded archive; ask for the current file only if there is reason to think it changed.
- Fix compile errors with the smallest possible change.
- For fast-moving APIs (`windows` crate, `sysinfo`, Tauri, UI Automation, Slint) check current docs via web search
  before writing code.
- The owner often applies several steps at once. If an experiment must be tested one at a time, say so clearly.

### 2.2 Technical rules (learned the hard way)

- `windows` 0.62: `BOOL` is in `windows::core`; `HWND(ptr)`; many functions return `Result`; edition 2024 needs
  `unsafe {}` inside `unsafe fn` (Win32 modules use `#![allow(unsafe_op_in_unsafe_fn)]`); `extern` blocks must be
  `unsafe extern`. Tauri depends on its own `windows` version, so convert `HWND` via the pointer value.
  `sysinfo` 0.40: `System::new()` and `Disks::new_with_refreshed_list()` return `Result`.
- Low-level keyboard hooks must be extremely fast (only post a message). Windows removes slow hooks and drops them on
  desktop switch (lock/unlock). Never call `SendInput` inside a hook callback.
- Window API calls from one thread via `run_on_main_thread` (see 1.4).
- Prefer WMI events + direct COM over PowerShell for brightness; never keep a long-lived PowerShell process.
- Large images (4K wallpapers) must not be decoded in the WebView: downsize in Rust first. Thumbnails, icons, and
  previews go through files on disk + asset protocol (not base64 over IPC). Rename the file on every capture to
  bypass image caching.
- Windows sharing one renderer (`--process-per-site`) share one JS thread; one busy window can stall the others briefly.
- Dev builds: `Cargo.toml` uses `[profile.dev.package."*"] opt-level = 3` and `[profile.dev] opt-level = 1` so `image`
  stays fast in `tauri dev`.
- **Any new managed state used by commands must be registered in the `orumi-state` plugin, not in `setup`.** A startup
  race (state registered after windows are created) caused random "state() called before manage()" crashes.
- New window checklist: `tauri.conf.json` entry (identical browser args, `visible: false` where applicable),
  `capabilities/default.json`, `App.tsx` routing, and registration in `PANELS` (unless self-managed like `activities`).
  Every new command goes into `generate_handler!`.
- Adding an `Icon` name requires updating the icon font subset (1.2). A missing glyph renders as the icon's name text.
- `Win32_Storage_Xps` is enabled in `Cargo.toml` for `PrintWindow`; `previews.rs` still declares it manually (see section 4).

### 2.3 Taskbar rules

- Hiding the taskbar must always come with a restore path that does not depend on a clean exit.
- Mechanism (modes: Auto-hide / Sembunyi (hidden) / Biarkan): `SHAppBarMessage(ABM_SETSTATE, ABS_AUTOHIDE)` on `Shell_TrayWnd`.
  Hidden mode (stage 2, following Bloom's `set_taskbar_visibility`) additionally does `SW_HIDE` + `SetWindowPos(-10000,-10000)` on
  `Shell_TrayWnd` and every `Shell_SecondaryTrayWnd`, and a 500 ms poller re-hides it if Explorer shows it again. Original state
  **and original window positions** are saved to `taskbar-restore.json` (`original_state` + `rects`) in the config dir before changing. Three restore layers: normal exit (`RunEvent::Exit`), a
  detached guard process (`<exe> --guard <pid> <marker>`, no Tauri/WebView, ~1–2 MB) that waits for the parent to die
  and then restores taskbar **and** flyout, and the marker re-read at next launch.
- A hidden (`SW_HIDE`/parked) taskbar has no UI Automation tree, so the tray `>` button cannot find the chevron. Solution
  (Bloom's `open_system_tray` pattern): `tray_toggle` calls `taskbar::reveal_for_tray` (window made alpha 0 + `WS_EX_TRANSPARENT`,
  moved back to its saved position, `SW_SHOW`), invokes the chevron, and `taskbar::conceal_after_close` hides it again once the
  flyout is closed. Restore order: show windows at saved positions first, then `ABM_SETSTATE`; if still parked, toggle the state to force a relayout.
- The Windows volume/brightness flyout is hidden by making Explorer's `XamlExplorerHostIslandWindow` fully transparent
  and click-through (`osd.rs`); restored on normal exit and by the guard. `scripts\check-flyout.ps1` lists changed
  windows (`-Pulihkan` restores) if symptoms return.

### 2.4 Activities-specific rules

- Activities was built in parallel without touching old files beyond a short list of touch points; the folder in the
  main repo is now the source of truth. Old zips and patches are obsolete.
- Win key design (history matters, do not repeat failed approaches):
  1. Hook only: unreliable (hook goes "blind" for hundreds of ms after Activities opens/closes, Start menu leaks).
  2. Hold Win-down and replay the combination: breaks Win+Arrow in the window manager (injected events are ignored by the WM).
  3. Swallow Win-up and re-inject: unreliable while Activities holds focus.
  4. **Current:** a high-priority poller reads `GetAsyncKeyState` every 8 ms. When Win goes down it injects one neutral
     key (VK 0xE8) while Win is held so Windows does not open Start on release. The real Win-up is never swallowed. Win
     released with no other key or click = tap = toggle. If Activities is open when Win is pressed, it dismisses. The
     hook is still installed but only as a supplement (instant masking, catching very short taps); it never swallows events.
- `dismiss` uses the same downward animation as `hide`. If the Start menu leaks again on rapid presses, revert `dismiss`
  to immediate hide (hint in the function's comment).
- Snapshot first, show later: thumbnails are captured before the window is shown so Activities is not captured. This
  causes a short delay before it appears. Minimized or failed windows use the last cached thumbnail, or the app icon.
- UI talks to workspaces only through `WorkspaceProvider`; swapping in an IPC provider means changing one line in
  `WorkspaceState::new()`.
- Letter rail: each letter starts a new row so jumps and the active letter are exact. While searching, the rail is hidden
  and results are flat, ordered by score; arrows and Enter only work in search results. Icons load lazily via
  `IntersectionObserver`.
- Dependencies on old code (adjust if these change): `winlist::collect_windows` and `WindowEntry` fields, commands
  `list_apps`, `launch_app`, `focus_window`, `app_icon`, component `AppIcon`, `edge::{place, ui_scale, Edge, PanelSpec}`.

### 2.5 Naming

The project is called ORUMI but the code still says edgeshell (see section 4). Do not rename `identifier`, config dir,
or the guard mode without a migration plan.

---

## 3. Current status

### 3.1 Working and tested by the owner

- Bar: launcher button, open-app list as real icons or dots, hover icon → thumbnail preview growing from the bar edge,
  clock → dashboard, tray `>` button, WiFi, volume, battery, power.
- Notch: dashboard (clock, weather, calendar, gauges, mini media), media, system (CPU/RAM/disk/GPU/WiFi/top processes), settings.
- Launcher: app search with icons (most relevant result at the bottom, near the search box), `>` actions + settings +
  wallpaper, `@` window switch, height follows content.
- Wallpaper carousel (endless, starts at the current wallpaper, asset-protocol thumbnails, width follows image count).
- Overlay: power, mixer, automatic volume/brightness popups. Custom Alt+Tab with app icons. Theme from wallpaper/custom
  (four colors). Settings. Tray flyout (native Windows). Taskbar auto-hide (normal exit, force kill, and settings toggle all tested).
- Activities (confirmed by the owner): Win key alone opens/closes it, rapid presses stable, Win+Arrow and other combos
  still reach the WM; bottom panel with the same appearance motion as the launcher; follows `ui_scale`; sharp icons
  after `ICON_SIZE = 128`; app list with a 27-dot letter rail (`#` then A–Z; active letter stretched and `--primary`;
  hover shows the letter); startup crash "state() called before manage() for Panels" fixed by the `orumi-state` plugin
  (still to be watched).

### 3.2 Implemented but not yet reported as working (do not assume proven)

- Activities: window thumbnails, dragging a window card onto a workspace box, touchpad scroll gesture between views.
- Impact of the preview and activities windows on memory.

- Taskbar full hide (`taskbar_hidden`, Settings -> Taskbar Windows -> "Sembunyi"), written 2026-11-13, not compiled by Claude
  (`cargo check` in the agent sandbox only fails on the unrelated `OUT_DIR` / build-script error). Test checklist:
  hide appears instantly with no work-area gap; switching Sembunyi / Auto-hide / Biarkan restores the taskbar at the right place;
  force-kill (guard restores it); restart with a leftover marker; tray `>` button still opens the flyout and the taskbar hides again
  after closing it; Win key / Explorer restart does not leave the taskbar visible; multi-monitor (secondary taskbars, untested).
  Risks: the 500 ms poller can flicker the taskbar if Explorer re-shows it; the HWND of a restarted Explorer is new, so saved
  positions fall back to `ABM_GETTASKBARPOS`; bottom-edge panels no longer conflict with the taskbar when hidden.
- Alt+Tab pure slide animation (`.swpanel` in `App.css`, fade removed; `translateY(100%)` → `0`, 200 ms).
- Notch quick settings (`quick.rs`, `useQuick.ts`, `VolumeBrightness.tsx`, `Notch.tsx` `settings` tab: Wi-Fi, Bluetooth,
  Airplane mode, Do Not Disturb tiles + horizontal volume/brightness sliders; dashboard `volume · brightness` placeholder
  replaced with vertical sliders) and ORUMI app/color settings moved into the launcher (`LauncherSettings.tsx`, opened via
  `>settings` or tray menu "Pengaturan") with dynamic panel width/height (`Aplikasi` 780 / `Warna` 720) and tightened card spacing.
  Notch height dynamically follows content (`ResizeObserver` + `notch_set_height` hit-area clamp in `edge.rs`), and changing
  volume/mute/brightness from UI sliders suppresses the automatic right-edge OSD popup (`LAST_OWN_CHANGE_MS` in `overlay.rs`
  and `brightness.rs`). Note: `scripts\make-icon-font.ps1` has 4 new icons (`bluetooth`,
  `bluetooth_disabled`, `airplanemode_active`, `do_not_disturb_on`) and must be re-run so those glyphs render properly.

### 3.3 Verified in the v0.1.3 code snapshot

- Activities touch points are present: `mod activities;`, five commands in `generate_handler!`, `activities::init(...)`
  in `setup`, plugin `orumi-state`, the `activities` window in `tauri.conf.json`, `Win32_Graphics_Gdi` and
  `Win32_Storage_Xps` in `Cargo.toml`, `ICON_SIZE = 128` / `ICON_VERSION = 2` in `icons.rs`. (`App.tsx` routing was not inspected.)
- There is no `CHANGES.md` in the archive.
- `winkey.rs` still has `const DEBUG: bool = true`.

### 3.4 Known limitations by design

- Window thumbnails are snapshots, not live; some apps return black/empty images with `PrintWindow`.
- Hooks and the Win key poller cannot see keys while an elevated (administrator) window has focus, unless ORUMI also runs elevated.
- Apps running as administrator are unreachable by the Alt+Tab replacement for the same reason.
- Windows virtual desktops are deliberately not used; real workspaces will come from vscroll-wm via IPC. Until then
  `StaticProvider` is a bridge and does not hide real windows.

---

## 4. Active issues, bugs, and tech debt

Priority is rough; "check" means suspected from reading code, not confirmed.

**Preview (bar hover thumbnails)**
- Moving between two app icons while a preview is open teleports the window (new position/size via `set_free_rect` +
  `place_panel`) and swaps content instantly; the CSS animation only covers open and close.
- Delay before the animation: after the 260 ms hover intent, Rust captures up to 4 windows sequentially (`PrintWindow`,
  JPEG encode, file write) before anything is shown. Duration not measured. `PrintWindow` can hang on non-responding apps.
- Flicker: the thumbnail file name changes per capture, so `<img>` may be blank briefly before decode.
- Waste: every hover re-captures, re-encodes, rewrites, and deletes old files even if the last capture was seconds ago.
- The "morph" effect is only a same-colored card with a flat left edge and a `scale` animation, not a truly merged shape.
- Native `title` tooltips on bar icons may collide with the preview (consider removing them).

**Activities**
- `DEBUG` in `winkey.rs` is `true`: noisy terminal, and it logs key codes pressed while Win is held. Turn off before calling it done.
- `set_win_key_enabled` exists (`#[allow(dead_code)]`) but is not wired to config or settings.
- The poller runs continuously (8 ms). Cheap, but on the optimization list.
- A very short tap can be missed by the poller (the hook catches it only when alive).
- Each letter in the app list starts a new row, so the last row of each letter is often not full.
- Fallback letter-box color uses `--primary-container` (from `AppIcon`), not `--secondary-container`.

**Integration and code health**
- Two `PrintWindow` capture paths: `previews.rs` (manual `extern` declaration, JPEG files) and `activities/thumbs.rs`
  (crate feature `Win32_Storage_Xps`, per-hwnd cache, rayon). Not yet compared in detail; should become one module.
- Check: `ConfigState` appears to be managed both in the `orumi-state` plugin and again in `setup` (`lib.rs`). Could be
  a leftover; verify the context before changing.
- Naming drift: `productName`, `identifier` (`com.example.edgeshell`), `package.json` name, Cargo lib name
  (`edgeshell_lib`), and the `--guard` exe name still say edgeshell; versions in `tauri.conf.json`, `package.json`, and
  `Cargo.toml` are still 0.1.0 while the archive is v0.1.3. Renaming the identifier changes the config dir, which
  orphans `taskbar-restore.json` and `config.json`; a migration is required.
- Open question: UI strings are partly Indonesian (e.g. the "diperkecil" badge). Decide the UI language together with
  the comment-language change.
- Unused command `wallpaper_image` can be removed.

**Input and system**
- Alt+Tab hook recovery: launcher close requests a reinstall; watchdog polls for missed Alt+Tab presses and, while the chord is still held, dismisses the native switcher and starts the custom one. Desktop-switch recovery (lock/unlock) still needs verification.
- Transparent areas of our windows capture clicks; a proper hit region (`SetWindowRgn`, see Bloom's `window_region.rs`) is not done.
- Polling → events where possible (mouse hook for screen edges, `IAudioEndpointVolumeCallback`).
- Security/stability todo (shared with vscroll-wm): re-install hooks after sleep and startup (chosen for the todo list),
  review of the Activities keyboard hook, poller thread, and key injection.
- Past laptop symptom (Alt+Tab dead / taskbar unclickable when the shell was not running) disappeared by itself; cause never proven.

---

## 5. Next steps (roadmap)

### 5.1 Current chat (in this order)

1. `context.md` (this file). Update it at the end of the chat with a new session-log entry.
2. Preview: tidy the animation and optimize. Ideas under evaluation: show the cached thumbnail first and refresh in the
   background; capture TTL; parallel capture; preload images before showing; smoother icon-to-icon transitions
   (larger window with CSS-moved card vs. moving the window with a FLIP-style delta; both untested and the latter risks flicker).
3. Unify the two capture paths (`previews.rs` and `activities/thumbs.rs`) into one module with one cache; do this as a
   separate step from step 2 to keep compile errors easy to trace.
4. Other optimization: turn off `DEBUG`, review the 8 ms poller, measure memory with Activities open and closed (7 windows)
   before deciding anything about a Slint port.
5. Taskbar hiding stage 2 is **implemented but untested** (see 3.2). Next: owner tests it (checklist in 3.2), then fix issues.
6. Finish merging Activities into the main code (checklist below), then new features if context remains.

Activities merge checklist (build the combined version and test):
- Starts without a crash, several times in a row.
- Rapid Win taps from the desktop and while Activities is open: Start menu must never appear.
- Win+Arrow, Win+E, Win+D, Win+Shift+S, Win+click, Win+scroll behave normally and do not toggle Activities.
- Window thumbnails appear; minimized windows use cache or icon.
- Dragging a window card to a workspace box moves it (fake provider).
- Letter rail: clicking a dot jumps, active letter follows scrolling, hover shows the letter.
- Icons sharp at high DPI; changing `ui_scale` then reopening resizes Activities.
- Click outside the panel, staged Esc, and Win all close Activities.

### 5.2 Later (not this chat unless stated)

- Remaining quick-settings / notification items: Night Light (skipped for now due to no safe public API in Bloom/Astraeus);
  Windows notifications restyled to match the shell (owner asked whether this is possible; needs research).
- Wire `set_win_key_enabled` to config; optional extra Activities triggers (bar button, hot corner); arrow-key
  navigation in the normal app list.
- Tray stage 2 (own tray icon panel via `NotifyIconSettings` registry + `Shell_NotifyIconGetRect` + UIA): fragile,
  only if the owner wants it.
- Carousel extras (dominant-color filter, other layouts): owner said "not now".
- Decide on pure-Rust UI (Slint) after the memory measurement.

### 5.3 vscroll-wm integration (end of the project)

- Merge into the ORUMI repo (git submodule idea) while the WM repo stays standalone; both are still early and run as
  separate programs for now.
- Replace `StaticProvider` with an IPC provider; the IPC format (list, switch, move_window, change events) must be
  specified first. The WM repo is not available to Claude unless uploaded.
- After merging, the WM overview should follow the shell style (appears from the bottom). The overview shows only the
  current workspace, and workspace-switching UI lives only in Activities. The standalone WM must still be able to
  switch workspaces from its own overview. **Undecided:** whether Activities and the WM overview become one view or two.

---

## 6. Decision log

| Decision | Why |
|---|---|
| Activities window is not in `PANELS` | keeps `edge.rs` untouched; spec is rebuilt on every open |
| Win key via poller + neutral key injection | hook-only and swallow/replay approaches failed (see 2.4) |
| Snapshot before showing Activities | otherwise Activities captures itself; costs a short delay |
| Workspaces behind a trait | WM IPC provider can replace the fake one without UI changes |
| All `manage()` calls in the `orumi-state` plugin | removes the startup race |
| Preview/thumbnails as files + asset protocol | avoids base64 over IPC and large images in the WebView |
| `--process-per-site` + font subset + adaptive polling | memory from ~500 MB peak to ~200–250 MB |
| Taskbar: auto-hide with three restore layers | safe baseline |
| Taskbar full hide = auto-hide + SW_HIDE + park off-screen; tray reveals it invisibly | Bloom's proven approach; replaces the earlier "never SW_HIDE" rule, tray button kept working via `reveal_for_tray` |
| Notch `settings` tab = Windows quick settings; ORUMI settings moved to launcher (`>settings`) | separates OS quick controls from shell configuration without adding another tab or window |
| Do Not Disturb via `HKCU\...\PushNotifications\ToastEnabled` | Astraeus pattern; avoids undocumented `WNF_SHEL_QUIETHOURS_*` binary blobs that can wedge Windows quick settings; Night Light skipped for the same reason |
| No Windows virtual desktops | API is undocumented and changes between builds; WM will own workspaces |

---

## 7. Session log

- 2026-11-11 (chat 3): read both handoffs and the v0.1.3 code; verified Activities touch points (3.3); created this
  `context.md`; agreed the plan in 5.1; agreed the English comments rule (2.1).
- 2026-11-13 (chat 4): read Bloom's `utils.rs` / `commands.rs` taskbar code; implemented taskbar full hide (`taskbar.rs` rewrite,
  `taskbar_hidden` config + Settings button, tray reveal in `tray_flyout.rs`); removed Alt+Tab fade (slide-only `.swpanel` in
  `App.css`); implemented notch quick settings (`quick.rs`, `useQuick.ts`, `VolumeBrightness.tsx`) + dashboard
  volume/brightness sliders, moved ORUMI app/color settings into launcher (`LauncherSettings.tsx`, tray menu), suppressed
  automatic OSD popups for own UI slider/mute changes, and added dynamic height/width & tighter card spacing for notch and
  launcher settings; updated 1.3, 1.5, 1.6, 1.7, 1.9, 2.3, 3.2, 5.1, 5.2, 6.
- Earlier: edgeshell built from bar/notch/launcher up to custom Alt+Tab, taskbar auto-hide, tray flyout, previews;
  Activities developed in a separate chat and merged into the main repo; code renamed to ORUMI v0.1.x (rename incomplete).

---

## 8. Chat handover protocol

When the owner says they are switching chats, Claude regenerates this file:

1. Update "Last updated", the code snapshot name, sections 3 to 5 (move finished items, add new bugs, reorder next steps),
   the decision log, and add a session-log entry.
2. Keep everything that is still true; do not shorten it into a summary. Context must carry over completely.
3. Keep it in English and keep the structure stable so diffs stay readable.

In a new chat: read this file first, then verify its claims against the uploaded code before relying on them
(file existence, touch points, constants). Treat this file as context, not as ground truth over the code.
