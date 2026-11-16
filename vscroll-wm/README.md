# V[ertical]SCROLL WM

<!-- logo placeholder -->
<!-- <img src="assets/logo.png" alt="VScroll WM logo" width="180"> -->

[![Status](https://img.shields.io/badge/status-development%20%2F%20testing-orange)](https://github.com/jack-dozen/vscroll-wm)
[![Platform](https://img.shields.io/badge/platform-Windows-blue)](https://github.com/jack-dozen/vscroll-wm)
[![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

> A vertical scrolling tiling window manager for Windows, inspired by Windows Snap and niri.

---

## What

**vscroll-wm** is a tiling window manager that combines the familiar snapping behavior of Windows with the scrolling workspace idea from [niri](https://github.com/YaLTeR/niri).

- Many shortcuts feel the same as Windows Snap (Win + Arrow, etc.)
- Unlike niri (which scrolls **horizontally**), this one lives up to its name: the workspace scrolls **vertically**

<!-- Demo video placeholder -->
<!-- ![Demo](assets/demo.gif) -->
<!-- or embed a YouTube / Loom link later -->

---

## Why

I really like niri’s tiling + scrolling concept, but I was too lazy to relearn a completely new set of muscle memory after years of using Windows Snap.

So I asked Claude to help build a window manager that keeps the Windows-style shortcuts while giving the vertical scrolling workspace feel.

---

## How

### Installation

1. Download the latest `.exe` from the [Releases](https://github.com/jack-dozen/vscroll-wm/releases) page (when available).
2. Put `vscroll-wm.exe` wherever you want.
3. (Optional) Copy `vscroll-wm.conf.example` → `vscroll-wm.conf` next to the exe and edit it.
4. Run the exe. It will sit in the system tray.

You can also point to a custom config with the environment variable:

```
VSCROLL_CONFIG=C:\path\to\your\vscroll-wm.conf
```

### Configuration

Config file: `vscroll-wm.conf` (next to the exe, or via `VSCROLL_CONFIG`).

Reload config anytime from the tray menu → **Reload configuration**.

| Key | Default | Description |
|-----|---------|-------------|
| `gap` | `16` | Gap between windows and screen edges (pixels, 0–200) |
| `anim_ms` | `160` | Animation duration in ms. `0` = off. Auto-disabled if Windows animation effects are turned off |
| `focus_border` | *(none)* | Focus border color on Windows 11 (`#RRGGBB` or `off`) |
| `overview_key` | `tab` | Key used with Win for overview (`tab`, `space`, `grave`, single letter/number, or `off`). `Win+Alt+Space` is always available |
| `overview_size` | `70` | Overview panel size as % of work area (30–100) |
| `mouse_drag` | `on` | Drag a window onto another to swap; drag the edge next to the column divider to move it; in the overview, drag a window onto another workspace to move it |
| `remember_layout` | `on` | Save the layout automatically and restore it when vscroll-wm is restarted while the windows are still open (not after a reboot) |
| `workspace_osd` | `on` | Small indicator near the bottom of the screen for ~1 second whenever the workspace changes |
| `overview_wrap` | `on` | Arrow keys in the overview wrap around at the ends (Up on the top row → bottom row, Left/Right → first/last workspace) |
| `resize_step` | `80` | Step (px) of the column divider shortcuts `Win` + `]` / `[` |
| `height_mode` / `height_step` | `preset` / `80` | `Win` + `↑` `↓`: jump between 1/3, 1/2, full (`preset`) or move per step (`step`, in px) |
| `cheatsheet_key` | `slash` | Key used with Win to open the shortcut cheatsheet (or `off`) |
| `bind.<action>` | *(see below)* | Custom shortcut for an action, e.g. `bind.close = win+w` |
| `overview_hint` | `on` | Show key hints at the bottom of the overview |
| `overview_thumbnails` | `on` | Show window thumbnails in overview |
| `overview_label` | `title` | Label style: `title` / `app` / `reverse` |
| `alias.<exe>` | — | Display name for overview when using `app` label mode (e.g. `alias.code = VS Code`) |
| `overview_color_*` | see example | Colors for overview (`selected`, `active`, `view`, `background`, `box`) |

There is also an ignore file (`vscroll-wm.ignore`) to keep certain windows floating:

```
# Examples
exe:vlc.exe
title:Picture in Picture
class:CabinetWClass
```

### Keyboard Shortcuts

Press `Win` + `/` at any time to see the cheatsheet; it is generated from the shortcuts
that are actually in effect, including your custom ones.

| Shortcut | Action |
|----------|--------|
| `Win` + `↑` `↓` | Shorter / taller (presets or steps, see `height_mode`) |
| `Win` + `←` `→` | Move into the left / right column |
| `Win` + `]` / `[` | Move the column divider (`resize_step`) |
| `Win` + `\` | Column divider back to 50/50 |
| `Win` + `Enter` | Back to full width and height |
| `Win` + `Shift` + Arrow | Move focus to the neighbouring window |
| `Win` + `Alt` + Arrow | Swap places with the neighbouring window |
| `Win` + `Tab` *(or configured key)* | Open overview (strip map) |
| `Win` + `Alt` + `Space` | Overview (second default chord) |
| `Win` + `Ctrl` + `←` `→` | Previous / next workspace |
| `Win` + `Ctrl` + `1`…`9` | Go to workspace n |
| `Win` + `Ctrl` + `Shift` + `←` `→` / `1`…`9` | Send the active window to another workspace |
| `Win` + `Q` | Close active window |
| `Win` + `M` | Minimize active window |
| `Win` + `Alt` + `F` | Toggle float / tile |
| `Win` + `Alt` + `I` | Show window info (class / exe / title) |
| `Win` + `/` | Shortcut cheatsheet |
| `Win` + `Ctrl` + `Esc` | Quit and restore all windows (fixed) |
| `Ctrl` + `Alt` + … | Backup copies of snap, focus, Enter, F, I, Space; `Ctrl` + `Alt` + `Shift` + `Q` quits (fixed) |

While overview is open:
- Drag a window onto another workspace's column to move it there (like Task View). A marker shows the landing spot: on a full-width window = it is split in half (side by side, the dropped one on the cursor's side); on a half-width window = that window's height is halved and the dropped one takes the upper / lower half in the same column; on the empty half next to a window = it fills it; at the very top / bottom edge = full width above / below the block. Dropping on another window of the same workspace swaps them; Esc cancels the drag
- Arrow keys → move selection (wraps around at the ends unless `overview_wrap = off`; `←` `→` at an edge continue into the neighbouring workspace)
- Tab / Shift+Tab (or the overview key while holding Win) → next / previous window in the workspace
- `Ctrl` + `←` `→` or `1`…`9` → select a workspace
- Enter or click → jump to the window; Esc → cancel
- Release Win after moving selection → jump to that window

### Custom shortcuts

Every shortcut above (except the fixed ones) can be changed in `vscroll-wm.conf`:

```
bind.close = win+w                       # one chord
bind.overview = win+o, win+alt+space     # several chords, separated by commas
bind.minimize = off                      # disable
bind.workspace_goto = alt+1-9            # numbered actions take "1-9"
```

A chord is one or more modifiers (`win`, `ctrl`, `alt`, `shift`; at least one) joined with `+`,
followed by a key. The full list of actions, key names and defaults is in
`vscroll-wm.conf.example`. If two actions claim the same chord, the explicitly configured
one wins and a warning is printed. Reload with the tray menu → **Reload configuration**.

---

## Hope

I hope someone forks, clones, copies, or just steals the idea and makes a better version of this.

Right now it only supports **Windows**. The long-term wish is to support more operating systems.

### Planned features
- *(nothing queued right now)*

### Known issues
*(to be filled)*

---

## License
