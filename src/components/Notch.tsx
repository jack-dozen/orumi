import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import { useGpu } from "../useGpu";
import { useLyrics } from "../useLyrics";
import { useMedia, type MediaState } from "../useMedia";
import { useNotifications, type NotifEntry } from "../useNotifications";
import { useVisualizerBars, useVisualizerSource } from "../useVisualizer";
import { useNow } from "../useNow";
import { useProcesses, type ProcEntry } from "../useProcesses";
import { useStats, type Stats } from "../useStats";
import { useWeather, type Weather } from "../useWeather";
import { useWifi, type WifiStatus } from "../useWifi";
import AppIcon from "./AppIcon";
import Gauge from "./Gauge";
import Icon from "./Icon";
import { MediaTab, MiniPlayer } from "./Media";
import WeatherCard from "./Weather";
import { useConfig, type Config } from "../useConfig";
import { useQuick, type PowerMode, type QuickKey, type QuickState } from "../useQuick";
import { useBluetooth } from "../useBluetooth";
import { BluetoothPanel, WifiPanel } from "./QuickPanels";
import VolumeBrightness from "./VolumeBrightness";
import Launcher, { type LauncherView } from "./Launcher";
import WmOverview from "./WmOverview";
import Switcher, { type SwitcherItem, type SwitcherLayout, type SwitcherState } from "./Switcher";
import Activities from "../activities/Activities";
import type { SnapWindow } from "../activities/useActivities";
import { PillBar, PillOverlay, type PillOverlayMode } from "./PillViews";

const pad = (n: number) => String(n).padStart(2, "0");

const TABS = [
  { id: "dashboard", label: "dashboard", icon: "dashboard" },
  { id: "media", label: "media", icon: "radio" },
  { id: "notification", label: "notification", icon: "notifications" },
  { id: "system", label: "system", icon: "monitoring" },
  { id: "settings", label: "settings", icon: "settings" },
] as const;

type TabId = (typeof TABS)[number]["id"];

const DAYS = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];

const gib = (bytes: number) => (bytes / 1024 ** 3).toFixed(1);

const fmtMem = (bytes: number) =>
  bytes >= 1024 ** 3
    ? `${(bytes / 1024 ** 3).toFixed(1)} GB`
    : `${Math.round(bytes / 1024 ** 2)} MB`;

const fmtRate = (bytesPerSec: number) => {
  if (bytesPerSec < 1024) return `${Math.round(bytesPerSec)} B/s`;
  if (bytesPerSec < 1024 * 1024) {
    const kb = bytesPerSec / 1024;
    return `${kb >= 100 ? Math.round(kb) : kb.toFixed(1)} KB/s`;
  }
  return `${(bytesPerSec / 1024 ** 2).toFixed(1)} MB/s`;
};

const HIST_LEN = 28;

type SysHistory = {
  seeded: boolean;
  cpu: number[];
  ram: number[];
  gpu: number[];
  down: number[];
  up: number[];
};

function emptyHistory(): SysHistory {
  return {
    seeded: false,
    cpu: Array(HIST_LEN).fill(0),
    ram: Array(HIST_LEN).fill(0),
    gpu: Array(HIST_LEN).fill(0),
    down: Array(HIST_LEN).fill(0),
    up: Array(HIST_LEN).fill(0),
  };
}

function pushSample(arr: number[], value: number): number[] {
  return [...arr.slice(1), value];
}

function sparkPaths(values: number[], max: number, w = 200, h = 48) {
  const safeMax = Math.max(1, max);
  const pts = values.map((v, i) => {
    const x = (i / Math.max(1, values.length - 1)) * w;
    const ratio = Math.max(0, Math.min(1, v / safeMax));
    const y = h - 3 - ratio * (h - 8);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const line = `M ${pts.join(" L ")}`;
  const area = `${line} L ${w},${h} L 0,${h} Z`;
  return { line, area };
}

/** Calendar cell for a full month: null = empty, number = date. */
function monthCells(year: number, month: number): (number | null)[] {
  const first = new Date(year, month, 1).getDay();
  const total = new Date(year, month + 1, 0).getDate();
  const cells: (number | null)[] = Array(first).fill(null);
  for (let d = 1; d <= total; d++) cells.push(d);
  return cells;
}

function Dashboard({
  stats,
  media,
  gpu,
  weather,
  visible,
}: {
  stats: Stats | null;
  media: MediaState;
  gpu: number | null;
  weather: Weather | null;
  visible: boolean;
}) {
  const now = useNow();
  const today = now.getDate();
  const [monthOffset, setMonthOffset] = useState(0);
  const ramPct = stats ? (stats.mem_used / stats.mem_total) * 100 : null;
  const showMini = Boolean(media.info);

  useEffect(() => {
    if (!visible) setMonthOffset(0);
  }, [visible]);

  const viewDate = new Date(now.getFullYear(), now.getMonth() + monthOffset, 1);
  const viewYear = viewDate.getFullYear();
  const viewMonth = viewDate.getMonth();
  const isCurrentMonth = viewYear === now.getFullYear() && viewMonth === now.getMonth();
  const monthLabel = viewDate.toLocaleDateString("en-US", { month: "long", year: "numeric" });

  return (
    <div className={`dash ${showMini ? "dash--media" : ""}`}>
      <div className="dash__col">
        <div className="card card--clock">
          {pad(now.getHours())} : {pad(now.getMinutes())}
        </div>
        <WeatherCard weather={weather} />
        <div className="card card--grow gauges">
          <Gauge label="RAM" value={ramPct} />
          <Gauge label="CPU" value={stats ? stats.cpu : null} />
          <Gauge label="GPU" value={gpu} />
        </div>
      </div>

      <div
        className="card calendar"
        onWheel={(e) => {
          if (e.deltaY === 0) return;
          setMonthOffset((m) => m + (e.deltaY > 0 ? 1 : -1));
        }}
      >
        <div className="calendar__nav">
          <button
            type="button"
            className="calendar__btn"
            title="Previous month"
            onClick={() => setMonthOffset((m) => m - 1)}
          >
            <Icon name="chevron_right" size={18} className="calendar__prev" />
          </button>
          <button
            type="button"
            className="calendar__title"
            title={monthOffset !== 0 ? "Back to current month" : undefined}
            onClick={() => setMonthOffset(0)}
          >
            {monthLabel}
          </button>
          <button
            type="button"
            className="calendar__btn"
            title="Next month"
            onClick={() => setMonthOffset((m) => m + 1)}
          >
            <Icon name="chevron_right" size={18} />
          </button>
        </div>
        <div className="calendar__grid">
          {DAYS.map((d, i) => (
            <div key={d} className={`calendar__head ${i === 0 ? "calendar__sun" : ""}`}>
              {d}
            </div>
          ))}
          {monthCells(viewYear, viewMonth).map((d, i) => (
            <div
              key={i}
              className={`calendar__day ${i % 7 === 0 ? "calendar__sun" : ""} ${
                isCurrentMonth && d === today ? "calendar__today" : ""
              }`}
            >
              {d}
            </div>
          ))}
        </div>
      </div>

      {showMini && <MiniPlayer media={media} active={visible} />}
    </div>
  );
}

function SparkGrid() {
  return (
    <>
      {[12, 24, 36].map((y) => (
        <line
          key={y}
          className="sysgraph__grid"
          x1="0"
          y1={y}
          x2="200"
          y2={y}
          vectorEffect="non-scaling-stroke"
        />
      ))}
    </>
  );
}

function MetricGraphCard({
  id,
  title,
  value,
  sub,
  series,
}: {
  id: string;
  title: string;
  value: string;
  sub?: string;
  series: number[];
}) {
  const { line, area } = sparkPaths(series, 100);
  const gradId = `sys-grad-${id}`;
  return (
    <div className="sysmetric">
      <div className="sysmetric__head">
        <div className="sysmetric__meta">
          <span className="sysmetric__title">{title}</span>
          {sub && <span className="sysmetric__sub">{sub}</span>}
        </div>
        <span className="sysmetric__val">{value}</span>
      </div>
      <svg className="sysgraph" viewBox="0 0 200 48" preserveAspectRatio="none">
        <defs>
          <linearGradient id={gradId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="var(--primary)" stopOpacity="0.4" />
            <stop offset="100%" stopColor="var(--primary)" stopOpacity="0.02" />
          </linearGradient>
        </defs>
        <SparkGrid />
        <path d={area} fill={`url(#${gradId})`} />
        <path
          d={line}
          fill="none"
          stroke="var(--primary)"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
    </div>
  );
}

function WifiGraphCard({
  down,
  up,
  downHist,
  upHist,
}: {
  down: number;
  up: number;
  downHist: number[];
  upHist: number[];
}) {
  const peak = Math.max(64 * 1024, ...downHist, ...upHist);
  const downPath = sparkPaths(downHist, peak);
  const upPath = sparkPaths(upHist, peak);
  return (
    <div className="sysmetric">
      <div className="sysmetric__head">
        <span className="sysmetric__title">Wi-Fi</span>
        <div className="sysmetric__rates">
          <span className="sysrate--down">↓ {fmtRate(down)}</span>
          <span className="sysrate--up">↑ {fmtRate(up)}</span>
        </div>
      </div>
      <svg className="sysgraph" viewBox="0 0 200 48" preserveAspectRatio="none">
        <defs>
          <linearGradient id="sys-grad-down" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="var(--primary)" stopOpacity="0.38" />
            <stop offset="100%" stopColor="var(--primary)" stopOpacity="0.02" />
          </linearGradient>
          <linearGradient id="sys-grad-up" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="var(--on-secondary-container)" stopOpacity="0.24" />
            <stop offset="100%" stopColor="var(--on-secondary-container)" stopOpacity="0.02" />
          </linearGradient>
        </defs>
        <SparkGrid />
        <path d={upPath.area} fill="url(#sys-grad-up)" />
        <path d={downPath.area} fill="url(#sys-grad-down)" />
        <path
          d={upPath.line}
          fill="none"
          stroke="var(--on-secondary-container)"
          strokeWidth="1.4"
          strokeDasharray="4 3"
          strokeLinecap="round"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
        <path
          d={downPath.line}
          fill="none"
          stroke="var(--primary)"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
      </svg>
    </div>
  );
}

function wifiLabel(wifi: WifiStatus | null): string {
  if (!wifi) return "loading…";
  if (!wifi.adapter) return "no adapter";
  if (wifi.location_denied) return "blocked by Windows";
  if (!wifi.connected) return "disconnected";
  return `${wifi.ssid ?? "connected"} · ${wifi.signal}%`;
}

function SystemTab({
  stats,
  procs,
  gpu,
  hist,
}: {
  stats: Stats | null;
  procs: ProcEntry[];
  gpu: number | null;
  hist: SysHistory;
}) {
  if (!stats) return <div className="card card--placeholder card--fill">loading…</div>;

  const ramPct = stats.mem_total > 0 ? (stats.mem_used / stats.mem_total) * 100 : 0;
  const diskPct = stats.disk_total > 0 ? (stats.disk_used / stats.disk_total) * 100 : 0;
  const procPeak = Math.max(1, ...procs.map((p) => p.cpu));

  return (
    <div className="card system">
      <div className="system__left">
        <div className="sysgrid">
          <MetricGraphCard
            id="cpu"
            title="CPU"
            value={`${Math.round(stats.cpu)}%`}
            series={hist.cpu}
          />
          <MetricGraphCard
            id="ram"
            title="RAM"
            sub={`${gib(stats.mem_used)} / ${gib(stats.mem_total)} GB`}
            value={`${Math.round(ramPct)}%`}
            series={hist.ram}
          />
          <MetricGraphCard
            id="gpu"
            title="GPU"
            value={gpu === null ? "—" : `${Math.round(gpu)}%`}
            series={hist.gpu}
          />
          <WifiGraphCard
            down={stats.net_down}
            up={stats.net_up}
            downHist={hist.down}
            upHist={hist.up}
          />
        </div>

        <div className="sysmetric sysmetric--disk">
          <div className="sysmetric__head">
            <div className="sysmetric__meta">
              <span className="sysmetric__title">Disk C:</span>
              <span className="sysmetric__sub">
                {gib(stats.disk_used)} / {gib(stats.disk_total)} GB
              </span>
            </div>
            <span className="sysmetric__val">{Math.round(diskPct)}%</span>
          </div>
          <div className="sysdisk__track">
            <div className="sysdisk__fill" style={{ width: `${Math.min(100, diskPct)}%` }} />
          </div>
        </div>
      </div>

      <div className="procs">
        <div className="procs__head">
          <span className="procs__title">Top processes</span>
          <span className="procs__legend">CPU · RAM</span>
        </div>
        {procs.length === 0 ? (
          <div className="procs__empty">loading…</div>
        ) : (
          <div className="procs__list">
            {procs.map((p) => (
              <div className="proc" key={p.name}>
                <span
                  className="proc__fill"
                  style={{ width: `${Math.min(100, (p.cpu / procPeak) * 100)}%` }}
                  aria-hidden="true"
                />
                <span className="proc__name">
                  <span className="proc__label">{p.name}</span>
                  {p.count > 1 && <span className="proc__badge">×{p.count}</span>}
                </span>
                <span className="proc__cpu">{p.cpu.toFixed(1)}%</span>
                <span className="proc__mem">{fmtMem(p.mem)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

type QuickTileDef =
  | {
      kind: "toggle";
      key: QuickKey;
      title: string;
      icon: (state: QuickState) => string;
      active: (state: QuickState) => boolean;
      sub: (state: QuickState, wifi: WifiStatus | null, autoTheme: boolean) => string;
      settings: string;
      locked?: (autoTheme: boolean) => boolean;
    }
  | {
      kind: "action";
      key: string;
      title: string;
      icon: string;
      sub: string;
      settings?: string;
      run: (onInstantHide: () => void) => void;
    };

const POWER_SUB: Record<PowerMode, string> = {
  saver: "Power saver",
  balanced: "Balanced",
  performance: "Best performance",
};

const QUICK_TILES: QuickTileDef[] = [
  {
    kind: "toggle",
    key: "wifi",
    title: "Wi-Fi",
    icon: (s) => (s.wifi ? "wifi" : "wifi_off"),
    active: (s) => s.wifi === true,
    sub: (s, wifi) => (s.wifi ? wifiLabel(wifi) : "Off"),
    settings: "wifi",
  },
  {
    kind: "toggle",
    key: "bluetooth",
    title: "Bluetooth",
    icon: (s) => (s.bluetooth ? "bluetooth" : "bluetooth_disabled"),
    active: (s) => s.bluetooth === true,
    sub: (s) => (s.bluetooth ? "On" : "Off"),
    settings: "bluetooth",
  },
  {
    kind: "toggle",
    key: "airplane",
    title: "Airplane mode",
    icon: () => "airplanemode_active",
    active: (s) => s.airplane,
    sub: (s) => (s.airplane ? "On" : "Off"),
    settings: "airplane",
  },
  {
    kind: "toggle",
    key: "hotspot",
    title: "Hotspot",
    icon: () => "wifi_tethering",
    active: (s) => s.hotspot === true,
    sub: (s) => (s.hotspot ? "On" : "Off"),
    settings: "hotspot",
  },
  {
    kind: "toggle",
    key: "dnd",
    title: "Do not disturb",
    icon: () => "do_not_disturb_on",
    active: (s) => s.dnd,
    sub: (s) => (s.dnd ? "Muted" : "Off"),
    settings: "dnd",
  },
  {
    kind: "toggle",
    key: "mic",
    title: "Microphone",
    icon: (s) => (s.mic ? "mic" : "mic_off"),
    active: (s) => s.mic === true,
    sub: (s) => (s.mic ? "Active" : "Muted"),
    settings: "sound",
  },
  {
    kind: "toggle",
    key: "caffeine",
    title: "Keep awake",
    icon: () => "coffee",
    active: (s) => s.caffeine,
    sub: (s) => (s.caffeine ? "Prevent sleep" : "Off"),
    settings: "power",
  },
  {
    kind: "toggle",
    key: "power_mode",
    title: "Power mode",
    icon: (s) =>
      s.power_mode === "saver" ? "eco" : s.power_mode === "performance" ? "bolt" : "battery_full",
    active: (s) => s.power_mode !== "balanced",
    sub: (s) => POWER_SUB[s.power_mode] ?? "Balanced",
    settings: "power",
  },
  {
    kind: "toggle",
    key: "dark_mode",
    title: "Windows theme",
    icon: (s) => (s.dark_mode ? "dark_mode" : "light_mode"),
    active: (s) => s.dark_mode,
    sub: (s, _w, autoTheme) => {
      const label = s.dark_mode ? "Dark" : "Light";
      return autoTheme ? `Follow wallpaper · ${label}` : label;
    },
    settings: "colors",
    locked: (autoTheme) => autoTheme,
  },
  {
    kind: "action",
    key: "snip",
    title: "Screen snip",
    icon: "content_cut",
    sub: "Win+Shift+S",
    settings: "display",
    run: (onInstantHide) => {
      onInstantHide();
      invoke("set_panel", { label: "notch", open: false }).catch(() => {});
      window.setTimeout(() => {
        invoke("open_settings_page", { page: "snip" }).catch(() => {});
      }, 120);
    },
  },
  {
    kind: "action",
    key: "display_off",
    title: "Display off",
    icon: "bedtime",
    sub: "Turn off screen",
    settings: "power",
    run: () => {
      invoke("set_panel", { label: "notch", open: false }).catch(() => {});
      invoke("run_action", { id: "display_off" }).catch(() => {});
    },
  },
  {
    kind: "action",
    key: "lock",
    title: "Lock screen",
    icon: "lock",
    sub: "Lock PC",
    run: () => {
      invoke("set_panel", { label: "notch", open: false }).catch(() => {});
      invoke("run_action", { id: "lock" }).catch(() => {});
    },
  },
];

type QuickSubView = "grid" | "wifi" | "bluetooth";

function bluetoothTileSub(
  state: QuickState,
  connectedNames: string[],
): string {
  if (!state.bluetooth) return "Off";
  if (connectedNames.length === 1) return connectedNames[0];
  if (connectedNames.length > 1) return `${connectedNames.length} connected`;
  return "On";
}

/** Notch "settings" tab = Windows quick settings. ORUMI's own settings live in the launcher (>settings). */
function QuickSettings({
  wifi,
  visible,
  cfg,
  subView,
  onSubView,
  onInstantHide,
}: {
  wifi: WifiStatus | null;
  visible: boolean;
  cfg: Config | null;
  subView: QuickSubView;
  onSubView: (v: QuickSubView) => void;
  onInstantHide: () => void;
}) {
  const { state, busy, toggle } = useQuick(visible);
  const bt = useBluetooth(visible, subView === "bluetooth");
  const autoTheme = cfg?.windows_theme_mode === "auto";
  const openPage = (page: string) => invoke("open_settings_page", { page }).catch(() => {});

  if (subView === "wifi") {
    return (
      <WifiPanel
        radioOn={state ? state.wifi : null}
        busyToggle={busy === "wifi"}
        visible={visible}
        onToggleRadio={(on) => toggle("wifi", on)}
        onBack={() => onSubView("grid")}
        onOpenSettings={openPage}
      />
    );
  }

  if (subView === "bluetooth") {
    return (
      <BluetoothPanel
        radioOn={state ? state.bluetooth : null}
        busyToggle={busy === "bluetooth"}
        visible={visible}
        btData={bt.data}
        nearby={bt.nearby}
        scanning={bt.scanning}
        pairPrompt={bt.pairPrompt}
        onStartScan={bt.startScan}
        onStopScan={bt.stopScan}
        onConnect={bt.connect}
        onDisconnect={bt.disconnect}
        onUnpair={bt.unpair}
        onPair={bt.pair}
        onRespondPair={bt.respondPair}
        onToggleRadio={(on) => toggle("bluetooth", on)}
        onBack={() => onSubView("grid")}
        onOpenSettings={openPage}
      />
    );
  }

  const btConnectedNames = (bt.data?.devices ?? [])
    .filter((d) => d.connected)
    .map((d) => d.name);

  return (
    <div className="quick">
      <div className="quick__tiles">
        {QUICK_TILES.map((t) => {
          if (t.kind === "action") {
            return (
              <button
                key={t.key}
                className="qtile"
                onClick={() => t.run(onInstantHide)}
                onContextMenu={
                  t.settings
                    ? (e) => {
                        e.preventDefault();
                        openPage(t.settings!);
                      }
                    : undefined
                }
              >
                <Icon name={t.icon} size={24} />
                <div className="qtile__text">
                  <span className="qtile__title">{t.title}</span>
                  <span className="qtile__sub">{t.sub}</span>
                </div>
              </button>
            );
          }

          const raw = state ? state[t.key] : null;
          const missing = state !== null && raw === null;
          const on = state ? t.active(state) : false;
          const locked = t.locked?.(autoTheme) ?? false;
          const nextValue = t.key === "power_mode" ? true : !(raw === true);
          const subText = !state
            ? "loading…"
            : missing
              ? "unavailable"
              : busy === t.key
                ? "…"
                : t.key === "bluetooth"
                  ? bluetoothTileSub(state, btConnectedNames)
                  : t.sub(state, wifi, autoTheme);

          if (t.key === "wifi" || t.key === "bluetooth") {
            const targetView: QuickSubView = t.key;
            return (
              <div
                key={t.key}
                className={`qtile qtile--split ${on ? "qtile--on" : ""} ${
                  missing ? "qtile--missing" : ""
                }`}
                onContextMenu={(e) => {
                  e.preventDefault();
                  openPage(t.settings);
                }}
              >
                <button
                  type="button"
                  className="qtile__main"
                  disabled={!state || missing || busy !== null}
                  title={`Toggle ${t.title} · Right-click for Windows Settings`}
                  onClick={() => toggle(t.key, nextValue)}
                >
                  <Icon name={state ? t.icon(state) : t.icon({} as QuickState)} size={24} fill={on} />
                  <div className="qtile__text">
                    <span className="qtile__title">{t.title}</span>
                    <span className="qtile__sub">{subText}</span>
                  </div>
                </button>
                <button
                  type="button"
                  className="qtile__arrow"
                  title={`Manage ${t.title}`}
                  aria-label={`Manage ${t.title}`}
                  onClick={() => onSubView(targetView)}
                >
                  <Icon name="chevron_right" size={18} />
                </button>
              </div>
            );
          }

          return (
            <button
              key={t.key}
              className={`qtile ${on ? "qtile--on" : ""} ${missing ? "qtile--missing" : ""} ${
                locked ? "qtile--locked" : ""
              }`}
              disabled={!state || missing || locked || busy !== null}
              title={
                locked
                  ? "Windows theme follows wallpaper automatically (change in >settings)"
                  : "Right-click to open Windows Settings"
              }
              onClick={() => toggle(t.key, nextValue)}
              onContextMenu={(e) => {
                e.preventDefault();
                openPage(t.settings);
              }}
            >
              <Icon name={state ? t.icon(state) : t.icon({} as QuickState)} size={24} fill={on} />
              <div className="qtile__text">
                <span className="qtile__title">{t.title}</span>
                <span className="qtile__sub">{subText}</span>
              </div>
            </button>
          );
        })}
      </div>

      {wifi?.location_denied && (
        <div className="settings__warn">
          Windows blocked Wi-Fi access for this app. Enable "Let desktop apps access your location"
          in Location settings.{" "}
          <button className="settings__link" onClick={() => openPage("location")}>
            Open Location settings
          </button>
        </div>
      )}

      <div className="quick__hint">
        Click arrow on Wi-Fi / Bluetooth to connect · Right-click a tile for Windows Settings
      </div>

      <VolumeBrightness active={visible} />
    </div>
  );
}

function formatNotifTime(arrivedMs: number): string {
  if (!arrivedMs) return "";
  const diffSec = Math.max(0, Math.floor((Date.now() - arrivedMs) / 1000));
  if (diffSec < 60) return "now";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m`;
  const d = new Date(arrivedMs);
  if (diffSec < 86400) return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return d.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

function NotifAvatar({ item, size }: { item: NotifEntry; size: number }) {
  const [imgFailed, setImgFailed] = useState(false);

  useEffect(() => {
    setImgFailed(false);
  }, [item.id, item.image]);

  if (item.image && !imgFailed) {
    return (
      <img
        className="notif__img"
        src={convertFileSrc(item.image)}
        alt=""
        draggable={false}
        style={{ width: size, height: size }}
        onError={() => setImgFailed(true)}
      />
    );
  }
  const fallback = (item.app || item.title || "?").trim().charAt(0).toUpperCase() || "?";
  return <AppIcon target={item.icon_target} fallback={fallback} size={size} />;
}

function NotificationPopup({
  item,
  open,
  onHover,
  onOpen,
  onDismiss,
}: {
  item: NotifEntry;
  open: boolean;
  onHover: (hovered: boolean) => void;
  onOpen: (id: number) => void;
  onDismiss: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !open) {
      invoke("notch_set_toast_rect", { width: 0, height: 0 }).catch(() => {});
      return;
    }
    const update = () => {
      invoke("notch_set_toast_rect", {
        width: el.offsetWidth,
        height: el.offsetHeight,
      }).catch(() => {});
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => {
      ro.disconnect();
      invoke("notch_set_toast_rect", { width: 0, height: 0 }).catch(() => {});
    };
  }, [open, item.id]);

  return (
    <div
      ref={ref}
      className={`notifosd ${open ? "notifosd--visible" : ""}`}
      onMouseEnter={() => onHover(true)}
      onMouseLeave={() => onHover(false)}
      onClick={() => onOpen(item.id)}
      role="status"
    >
      <div className="notifosd__avatar">
        <NotifAvatar item={item} size={34} />
      </div>
      <div className="notifosd__content">
        <div className="notifosd__meta">
          <span className="notifosd__app">{item.app || "Notification"}</span>
          <span className="notifosd__time">{formatNotifTime(item.arrived_ms)}</span>
        </div>
        <div className="notifosd__title">{item.title}</div>
        {item.body && <div className="notifosd__body">{item.body}</div>}
      </div>
      <button
        type="button"
        className="notifosd__close"
        title="Mark as read"
        aria-label="Mark as read"
        onClick={(e) => {
          e.stopPropagation();
          onDismiss();
        }}
      >
        <Icon name="close" size={16} />
      </button>
    </div>
  );
}

function NotificationsTab({
  items,
  enabled,
  onOpen,
  onMarkRead,
  onClearAll,
}: {
  items: NotifEntry[];
  enabled: boolean;
  onOpen: (id: number) => void;
  onMarkRead: (id: number) => void;
  onClearAll: () => void;
}) {
  return (
    <div className="card notifs">
      <div className="notifs__head">
        <div className="notifs__title-wrap">
          <span className="notifs__title">Unread notifications</span>
          {items.length > 0 && <span className="notifs__count">{items.length}</span>}
        </div>
        {items.length > 0 && (
          <button type="button" className="notifs__clear" onClick={onClearAll}>
            <Icon name="done_all" size={16} />
            Clear all
          </button>
        )}
      </div>

      {!enabled ? (
        <div className="notifs__empty">
          <Icon name="notifications_off" size={28} />
          <span>Notifications are disabled in ORUMI settings</span>
        </div>
      ) : items.length === 0 ? (
        <div className="notifs__empty">
          <Icon name="notifications" size={28} />
          <span>No unread notifications</span>
        </div>
      ) : (
        <div className="notifs__list">
          {items.map((item) => (
            <div
              key={item.id}
              className="notifcard"
              onClick={() => onOpen(item.id)}
              role="button"
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onOpen(item.id);
                }
              }}
            >
              <div className="notifcard__avatar">
                <NotifAvatar item={item} size={32} />
              </div>
              <div className="notifcard__content">
                <div className="notifcard__meta">
                  <span className="notifcard__app">{item.app || "Notification"}</span>
                  <span className="notifcard__time">{formatNotifTime(item.arrived_ms)}</span>
                </div>
                <div className="notifcard__title">{item.title}</div>
                {item.body && <div className="notifcard__body">{item.body}</div>}
              </div>
              <button
                type="button"
                className="notifcard__close"
                title="Mark as read"
                aria-label="Mark as read"
                onClick={(e) => {
                  e.stopPropagation();
                  onMarkRead(item.id);
                }}
              >
                <Icon name="close" size={16} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

type WsOsd = { active: number; total: number };

const COMPACT_VIZ_BARS = 16;

function CompactNotch({
  media,
  active,
  onExpand,
  onOpenLyrics,
  onOpenBar,
}: {
  media: MediaState;
  active: boolean;
  onExpand: (targetTab?: TabId) => void;
  onOpenLyrics: () => void;
  onOpenBar?: () => void;
}) {
  const now = useNow();
  const playing = Boolean(media.info?.playing);
  const { barRefs } = useVisualizerBars(active && playing, COMPACT_VIZ_BARS, {
    minScale: 0.18,
    beatLift: 0.08,
  });

  const timeText = `${pad(now.getHours())}:${pad(now.getMinutes())}`;
  const dateText = now.toLocaleDateString("en-US", {
    weekday: "short",
    month: "short",
    day: "numeric",
  });

  const onNonClockActivate = () => {
    if (onOpenBar) onOpenBar();
    else onExpand();
  };

  return (
    <div
      role="button"
      tabIndex={0}
      className={`cnotch ${playing ? "cnotch--media" : ""}`}
      onClick={onNonClockActivate}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onNonClockActivate();
        }
      }}
      title={
        onOpenBar
          ? "Click clock to open Notch · Click elsewhere to open Bar"
          : playing && media.info
            ? `${media.info.title || "Playing media"} — Click to expand Notch`
            : "Click to expand Notch"
      }
    >
      {playing ? (
        <>
          <button
            type="button"
            className="cnotch__coverbtn"
            onClick={(e) => {
              e.stopPropagation();
              onExpand("media");
            }}
            title="Open Media tab"
            aria-label="Open Media tab"
          >
            {media.art ? (
              <img className="cnotch__cover" src={media.art} alt="" draggable={false} />
            ) : (
              <span className="cnotch__cover cnotch__cover--empty">
                <Icon name="music_note" size={14} />
              </span>
            )}
          </button>
          <button
            type="button"
            className="cnotch__viz"
            onClick={(e) => {
              e.stopPropagation();
              onOpenLyrics();
            }}
            title="Live lyrics (pin at notch)"
            aria-label="Open live lyrics"
          >
            {Array.from({ length: COMPACT_VIZ_BARS }, (_, i) => (
              <span
                key={i}
                ref={(el) => {
                  barRefs.current[i] = el;
                }}
                className="cnotch__bar"
              />
            ))}
          </button>
          <span
            className="cnotch__time cnotch__time--media"
            title="Open Full Notch"
            onClick={(e) => {
              e.stopPropagation();
              onExpand();
            }}
          >
            {timeText}
          </span>
        </>
      ) : (
        <>
          <span
            className="cnotch__time"
            title="Open Full Notch"
            onClick={(e) => {
              e.stopPropagation();
              onExpand();
            }}
          >
            {timeText}
          </span>
          <span className="cnotch__sep" aria-hidden="true">
            ·
          </span>
          <span className="cnotch__date">{dateText}</span>
        </>
      )}
    </div>
  );
}

function CompactLyrics({
  media,
  active,
  onExit,
}: {
  media: MediaState;
  active: boolean;
  onExit: () => void;
}) {
  const { info, art } = media;
  const lyrics = useLyrics(info, active);

  let primary = info?.title || "Live lyrics";
  let secondary = info?.artist || info?.source || "";

  if (!info) {
    primary = "No media playing";
    secondary = "Click to return";
  } else if (lyrics.status === "loading" || lyrics.status === "idle") {
    secondary = "Loading lyrics…";
  } else if (lyrics.status === "error") {
    primary = "Couldn't load lyrics";
    secondary = info.title || "Click to return";
  } else if (!lyrics.data || lyrics.data.kind === "none") {
    secondary = "No lyrics found";
  } else if (lyrics.data.kind === "instrumental") {
    secondary = "♪ Instrumental ♪";
  } else if (lyrics.data.kind === "plain") {
    secondary = "Unsynced lyrics · Click to return";
  } else if (lyrics.data.kind === "synced") {
    const lines = lyrics.data.lines;
    const idx = lyrics.activeIndex;
    if (idx >= 0 && idx < lines.length) {
      primary = lines[idx].text;
      secondary = lines[idx + 1]?.text ?? (info.artist ? `${info.title} · ${info.artist}` : info.title);
    } else {
      secondary = [info.artist, "♪"].filter(Boolean).join(" · ");
    }
  }

  return (
    <div
      role="button"
      tabIndex={0}
      className={`clyrics ${info && !info.playing ? "clyrics--paused" : ""}`}
      onClick={onExit}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onExit();
        }
      }}
      title="Click to return to compact notch"
    >
      {art ? (
        <img className="clyrics__cover" src={art} alt="" draggable={false} />
      ) : (
        <span className="clyrics__cover clyrics__cover--empty">
          <Icon name="music_note" size={16} />
        </span>
      )}
      <div className="clyrics__body">
        <div
          key={lyrics.data?.kind === "synced" ? `line-${lyrics.activeIndex}` : "line-static"}
          className="clyrics__line"
        >
          {primary}
        </div>
        {secondary && <div className="clyrics__next">{secondary}</div>}
      </div>
      {lyrics.status === "error" && (
        <button
          type="button"
          className="clyrics__btn"
          title="Retry loading lyrics"
          aria-label="Retry loading lyrics"
          onClick={(e) => {
            e.stopPropagation();
            lyrics.retry();
          }}
        >
          <Icon name="refresh" size={16} />
        </button>
      )}
      <span className="clyrics__close" aria-hidden="true">
        <Icon name="close" size={16} />
      </span>
    </div>
  );
}

type PillView =
  | "compact"
  | "lyrics"
  | "bar"
  | "full"
  | "overlay"
  | "overview"
  | "launcher"
  | "switcher"
  | "activities";

type NotchSurfaceMode = "hidden" | "ws" | "toast" | PillView;

export default function Notch() {
  const visible = useEdgeVisible("notch");
  const cfg = useConfig();
  const pillMode = cfg?.shell_mode === "pill";

  const [opening, setOpening] = useState(false);
  const [compact, setCompact] = useState(false);
  const [compactLyrics, setCompactLyrics] = useState(false);
  const [tab, setTab] = useState<TabId>("dashboard");
  const [quickSubView, setQuickSubView] = useState<QuickSubView>("grid");
  const [instantHide, setInstantHide] = useState(false);
  const [wsOsd, setWsOsd] = useState<WsOsd | null>(null);
  const [osdOpen, setOsdOpen] = useState(false);
  const osdTimer = useRef<number | undefined>(undefined);

  const [pillView, setPillView] = useState<PillView>("compact");
  const [pillBarWidth, setPillBarWidth] = useState(980);
  const [pillLauncherWidth, setPillLauncherWidth] = useState(640);
  const [pillSwitcherWidth, setPillSwitcherWidth] = useState(680);
  const [pillOverlayWidth, setPillOverlayWidth] = useState(400);
  const [launcherSub, setLauncherSub] = useState<LauncherView>("list");
  const [overlayMode, setOverlayMode] = useState<PillOverlayMode>("mixer");
  const [overlayManual, setOverlayManual] = useState(true);
  const [overlaySeq, setOverlaySeq] = useState(0);
  const [overlayFromBar, setOverlayFromBar] = useState(false);
  const [activitiesWins, setActivitiesWins] = useState<SnapWindow[] | undefined>(undefined);
  const [swState, setSwState] = useState<SwitcherState>({
    items: [],
    layout: null,
    index: 0,
    thumbs: {},
  });

  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const pillViewRef = useRef(pillView);
  pillViewRef.current = pillView;
  const launcherSubRef = useRef(launcherSub);
  launcherSubRef.current = launcherSub;
  const overlayModeRef = useRef(overlayMode);
  overlayModeRef.current = overlayMode;

  const closePill = () => {
    invoke("pill_open", { open: false, pinned: false, focus: false }).catch(() => {});
  };

  const setLyricsPin = (next: boolean) => {
    setCompactLyrics(next);
    invoke("notch_set_lyrics_pin", { pinned: next }).catch(() => {});
  };

  const fullShown = pillMode ? visible && pillView === "full" : visible && !compact;
  const compactShown = pillMode ? visible && pillView === "compact" : visible && compact;

  // Fetch data only while the notch is visible and its view uses it.
  const stats = useStats(fullShown && (tab === "dashboard" || tab === "system"));
  const media = useMedia(
    (opening || visible) &&
      (compactShown || (fullShown && (tab === "dashboard" || tab === "media"))),
  );
  const compactMediaPlaying = compactShown && Boolean(media.info?.playing);
  useVisualizerSource(
    (opening || visible) &&
      ((compactMediaPlaying && !compactLyrics) ||
        (fullShown && (tab === "dashboard" || tab === "media") && Boolean(media.info))),
  );
  const procs = useProcesses(fullShown && tab === "system");
  const gpu = useGpu(fullShown && (tab === "dashboard" || tab === "system"));
  const wifi = useWifi(fullShown && tab === "settings");
  const weather = useWeather(cfg?.weather_city ?? "Bandung");
  const notifsEnabled = cfg?.notifications_enabled ?? true;
  const notifsPopup = cfg?.notifications_popup ?? true;
  const notifs = useNotifications(notifsEnabled, notifsPopup, visible);

  const [sysHist, setSysHist] = useState<SysHistory>(emptyHistory);
  const gpuRef = useRef<number | null>(gpu);
  gpuRef.current = gpu;

  useEffect(() => {
    if (!stats) return;
    const ramPct = stats.mem_total > 0 ? (stats.mem_used / stats.mem_total) * 100 : 0;
    const gpuVal = gpuRef.current ?? 0;
    setSysHist((prev) => {
      if (!prev.seeded) {
        return {
          seeded: true,
          cpu: Array(HIST_LEN).fill(stats.cpu),
          ram: Array(HIST_LEN).fill(ramPct),
          gpu: Array(HIST_LEN).fill(gpuVal),
          down: pushSample(prev.down, stats.net_down),
          up: pushSample(prev.up, stats.net_up),
        };
      }
      return {
        seeded: true,
        cpu: pushSample(prev.cpu, stats.cpu),
        ram: pushSample(prev.ram, ramPct),
        gpu: pushSample(prev.gpu, gpuVal),
        down: pushSample(prev.down, stats.net_down),
        up: pushSample(prev.up, stats.net_up),
      };
    });
  }, [stats]);

  const showMini = Boolean(media.info);
  const mode: NotchSurfaceMode = visible
    ? pillMode
      ? pillView === "compact" && compactLyrics
        ? "lyrics"
        : pillView
      : compact && compactLyrics
        ? "lyrics"
        : compact
          ? "compact"
          : "full"
    : osdOpen && wsOsd
      ? "ws"
      : notifs.activeToast && notifs.toastOpen
        ? "toast"
        : "hidden";

  const lastModeRef = useRef<Exclude<NotchSurfaceMode, "hidden">>("compact");
  if (mode !== "hidden") lastModeRef.current = mode;
  const activeMode = mode !== "hidden" ? mode : lastModeRef.current;

  const lastToastRef = useRef(notifs.activeToast);
  if (notifs.activeToast) lastToastRef.current = notifs.activeToast;
  const shownToast = notifs.activeToast ?? lastToastRef.current;

  const wsWidth = wsOsd ? Math.min(520, Math.max(236, 178 + wsOsd.total * 18)) : 248;
  const width =
    activeMode === "ws"
      ? wsWidth
      : activeMode === "toast" || activeMode === "lyrics"
        ? 390
        : activeMode === "compact"
          ? compactMediaPlaying
            ? 272
            : 248
          : activeMode === "bar"
            ? pillBarWidth
            : activeMode === "overlay"
              ? pillOverlayWidth
              : activeMode === "launcher"
                ? pillLauncherWidth
                : activeMode === "switcher"
                  ? pillSwitcherWidth
                  : activeMode === "overview"
                    ? 390
                    : activeMode === "activities"
                      ? 1160
                      : tab === "dashboard"
                        ? showMini
                          ? 740
                          : 550
                        : tab === "media"
                          ? 740
                          : tab === "notification"
                            ? 700
                            : tab === "system"
                              ? 760
                              : 820;

  // Follow content size dynamically (keeps compact tabs slim and centered).
  const innerRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const measure = () => setHeight(el.offsetHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useLayoutEffect(() => {
    const el = innerRef.current;
    if (el) setHeight(el.offsetHeight);
  }, [
    activeMode,
    tab,
    quickSubView,
    showMini,
    wsOsd,
    shownToast?.id,
    pillBarWidth,
    pillLauncherWidth,
    pillSwitcherWidth,
    pillOverlayWidth,
  ]);

  useLayoutEffect(() => {
    if (height === null) return;
    invoke("notch_set_height", { height, width }).catch(() => {});
  }, [height, width]);

  // Notch closed: return to "does not take focus" mode (if it was used for typing).
  useEffect(() => {
    setInstantHide(false);
    if (!visible) {
      setQuickSubView("grid");
      setCompactLyrics(false);
      setOverlayFromBar(false);
      invoke("notch_set_lyrics_pin", { pinned: false }).catch(() => {});
      invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
      const id = window.setTimeout(() => {
        lastModeRef.current = "compact";
        setPillView("compact");
      }, 320);
      return () => window.clearTimeout(id);
    }
  }, [visible]);

  useEffect(() => {
    if ((!pillMode || pillView === "full") && tab !== "settings") {
      setQuickSubView("grid");
      invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
    }
  }, [pillMode, pillView, tab]);

  // Window lost focus (clicked another app): release focus hold, or close interactive pill view.
  useEffect(() => {
    const onBlur = () => {
      // The native color dialog makes the window lose focus; keep the focus hold during it.
      const el = document.activeElement as HTMLInputElement | null;
      if (el?.tagName === "INPUT" && el.type === "color") return;
      invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
      if (
        visibleRef.current &&
        (pillViewRef.current === "launcher" || pillViewRef.current === "activities")
      ) {
        closePill();
      }
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Track compact-vs-full open mode, Morphing Pill commands, Switcher & Activities payloads.
  useEffect(() => {
    const unlistenCompact = listen<boolean>("notch-compact", (e) => {
      setCompact(e.payload);
      if (!e.payload) setCompactLyrics(false);
    });
    const unlistenLyricsPin = listen<boolean>("notch-lyrics-pin", (e) => {
      setCompactLyrics(e.payload);
    });
    const unlistenOpen = listen<boolean>("edge-visible:notch", (e) => {
      setOpening(e.payload);
      if (!e.payload) setCompactLyrics(false);
    });
    const unlistenEdgePill = listen<"compact" | "bar">("pill-edge-open", (e) => {
      setCompactLyrics(false);
      setOverlayFromBar(false);
      setPillView(e.payload);
    });
    const unlistenPillSw = listen<boolean>("pill-switcher", (e) => {
      if (e.payload) {
        setPillView("switcher");
      } else if (pillViewRef.current === "switcher") {
        setPillView("compact");
      }
    });
    const unlistenPillCmd = listen<{
      mode: string;
      sub?: string;
      manual?: boolean;
      toggle?: boolean;
    }>("pill-command", (e) => {
      const p = e.payload;
      if (!p?.mode) return;
      if (p.mode === "close") {
        closePill();
        return;
      }
      if (p.mode === "close-activities") {
        if (pillViewRef.current === "activities") closePill();
        return;
      }
      if (p.mode === "close-overlay") {
        if (pillViewRef.current === "overlay") closePill();
        return;
      }
      if (p.mode === "activities") {
        setPillView("activities");
        return;
      }
      if (p.mode === "bar") {
        if (p.toggle && visibleRef.current && pillViewRef.current === "bar") {
          closePill();
          return;
        }
        setCompactLyrics(false);
        setPillView("bar");
        if (p.toggle) {
          invoke("pill_open", { open: true, pinned: true, focus: false }).catch(() => {});
        }
        return;
      }
      if (p.mode === "full") {
        if (p.toggle && visibleRef.current && pillViewRef.current === "full") {
          closePill();
          return;
        }
        setCompactLyrics(false);
        setPillView("full");
        if (p.toggle) {
          invoke("pill_open", { open: true, pinned: true, focus: false }).catch(() => {});
        }
        return;
      }
      if (p.mode === "launcher") {
        const sub: LauncherView =
          p.sub === "settings" || p.sub === "cheatsheet" || p.sub === "wallpaper"
            ? p.sub
            : "list";
        if (
          p.toggle &&
          visibleRef.current &&
          pillViewRef.current === "launcher" &&
          (!p.sub || launcherSubRef.current === sub)
        ) {
          closePill();
          return;
        }
        setLauncherSub(sub);
        setPillView("launcher");
        invoke("pill_open", { open: true, pinned: true, focus: true }).catch(() => {});
        return;
      }
      if (p.mode === "overview") {
        if (p.toggle && visibleRef.current && pillViewRef.current === "overview") {
          closePill();
          return;
        }
        setPillView("overview");
        invoke("pill_open", { open: true, pinned: true, focus: true }).catch(() => {});
        return;
      }
      if (p.mode === "overlay") {
        const sub = (p.sub ?? "mixer") as PillOverlayMode;
        const manual = p.manual ?? true;
        if (
          p.toggle &&
          visibleRef.current &&
          pillViewRef.current === "overlay" &&
          overlayModeRef.current === sub
        ) {
          closePill();
          return;
        }
        setOverlayMode(sub);
        setOverlayManual(manual);
        setOverlaySeq((n) => n + 1);
        setOverlayFromBar(false);
        setPillView("overlay");
        if (manual) {
          invoke("pill_open", { open: true, pinned: true, focus: false }).catch(() => {});
        }
      }
    });
    const unlistenSwItems = listen<{
      items: SwitcherItem[];
      index: number;
      layout: SwitcherLayout;
    }>("switcher-items", (e) => {
      const thumbs: Record<number, string> = {};
      for (const it of e.payload.items) if (it.thumb) thumbs[it.hwnd] = it.thumb;
      setSwState({
        items: e.payload.items,
        index: e.payload.index,
        layout: e.payload.layout,
        thumbs,
      });
    });
    const unlistenSwSel = listen<number>("switcher-select", (e) => {
      setSwState((s) => ({ ...s, index: e.payload }));
    });
    const unlistenSwThumb = listen<{ hwnd: number; thumb: string }>("switcher-thumb", (e) => {
      setSwState((s) => ({
        ...s,
        thumbs: { ...s.thumbs, [e.payload.hwnd]: e.payload.thumb },
      }));
    });
    const unlistenActOpen = listen<SnapWindow[]>("activities-open", (e) => {
      setActivitiesWins(e.payload ?? []);
    });

    return () => {
      unlistenCompact.then((fn) => fn());
      unlistenLyricsPin.then((fn) => fn());
      unlistenOpen.then((fn) => fn());
      unlistenEdgePill.then((fn) => fn());
      unlistenPillSw.then((fn) => fn());
      unlistenPillCmd.then((fn) => fn());
      unlistenSwItems.then((fn) => fn());
      unlistenSwSel.then((fn) => fn());
      unlistenSwThumb.then((fn) => fn());
      unlistenActOpen.then((fn) => fn());
    };
  }, []);

  // The bar can ask the notch to switch to a specific tab (clock or WiFi icon click).
  useEffect(() => {
    const unlisten = listen<string>("notch-tab", (e) => {
      setLyricsPin(false);
      setCompact(false);
      const [t, sub] = e.payload.split("/");
      if (TABS.some((x) => x.id === t)) setTab(t as TabId);
      if (t === "settings" && (sub === "wifi" || sub === "bluetooth" || sub === "grid")) {
        setQuickSubView(sub);
      } else if (t === "settings") {
        setQuickSubView("grid");
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Broadcast active tab so the bar knows whether clicking clock/WiFi should switch or close.
  useEffect(() => {
    emit("notch-tab-active", tab).catch(() => {});
  }, [tab]);

  // Top-notch workspace OSD when switching workspaces while the full notch is closed.
  useEffect(() => {
    const unlisten = listen<WsOsd>("workspace-osd", (e) => {
      setWsOsd(e.payload);
      setOsdOpen(true);
      window.clearTimeout(osdTimer.current);
      osdTimer.current = window.setTimeout(() => setOsdOpen(false), 1100);
    });
    return () => {
      unlisten.then((fn) => fn());
      window.clearTimeout(osdTimer.current);
    };
  }, []);

  // Hide the workspace OSD immediately if the user hovers/opens the full notch.
  useEffect(() => {
    if (visible) {
      window.clearTimeout(osdTimer.current);
      setOsdOpen(false);
    }
  }, [visible]);

  const surfaceOpen = mode !== "hidden";
  const isPill =
    activeMode === "compact" ||
    activeMode === "lyrics" ||
    activeMode === "ws" ||
    activeMode === "toast" ||
    activeMode === "bar" ||
    activeMode === "overlay";
  const zeroPadInner =
    isPill ||
    activeMode === "launcher" ||
    activeMode === "overview" ||
    activeMode === "switcher" ||
    activeMode === "activities";

  return (
    <div
      className={`notch ${surfaceOpen && !instantHide ? "notch--visible" : ""} ${
        instantHide ? "notch--instant" : ""
      } ${isPill ? "notch--compact" : ""} ${zeroPadInner ? "notch--flush" : ""} ${
        activeMode === "ws" ? "notch--passthrough" : ""
      }`}
      style={{ width, height: height ?? undefined }}
    >
      <div className="notch__clip">
        <div className="notch__inner" ref={innerRef}>
          {activeMode === "ws" && wsOsd ? (
            <div key="ws" className="notch__stage">
              <div className="wsosd" aria-hidden="true">
                <span className="wsosd__label">Workspace {wsOsd.active + 1}</span>
                <span className="wsosd__dots">
                  {Array.from({ length: wsOsd.total }, (_, i) => (
                    <span
                      key={i}
                      className={`wsosd__dot ${i === wsOsd.active ? "wsosd__dot--active" : ""}`}
                    />
                  ))}
                </span>
              </div>
            </div>
          ) : activeMode === "toast" && shownToast ? (
            <div key={`toast-${shownToast.id}`} className="notch__stage">
              <NotificationPopup
                item={shownToast}
                open={mode === "toast"}
                onHover={notifs.setToastHovered}
                onOpen={notifs.openNotif}
                onDismiss={() => notifs.dismissToast(true)}
              />
            </div>
          ) : activeMode === "lyrics" ? (
            <div key="lyrics" className="notch__stage">
              <CompactLyrics
                media={media}
                active={visible && compactShown && compactLyrics}
                onExit={() => setLyricsPin(false)}
              />
            </div>
          ) : activeMode === "compact" ? (
            <div key="compact" className="notch__stage">
              <CompactNotch
                media={media}
                active={compactShown}
                onExpand={(targetTab) => {
                  if (targetTab) setTab(targetTab);
                  setLyricsPin(false);
                  if (pillMode) setPillView("full");
                  else setCompact(false);
                }}
                onOpenLyrics={() => setLyricsPin(true)}
                onOpenBar={pillMode ? () => { setLyricsPin(false); setPillView("bar"); } : undefined}
              />
            </div>
          ) : activeMode === "bar" ? (
            <div key="bar" className="notch__stage">
              <PillBar
                active={visible && pillView === "bar"}
                cfg={cfg}
                onCollapseToCompact={() => {
                  setLyricsPin(false);
                  setPillView("compact");
                }}
                onOpenFullNotch={(targetTab) => {
                  if (targetTab) setTab(targetTab);
                  setLyricsPin(false);
                  setPillView("full");
                }}
                onOpenLauncher={() => {
                  setLauncherSub("list");
                  setPillView("launcher");
                  invoke("pill_open", { open: true, pinned: true, focus: true }).catch(() => {});
                }}
                onOpenOverview={() => {
                  setPillView("overview");
                  invoke("pill_open", { open: true, pinned: true, focus: true }).catch(() => {});
                }}
                onOpenOverlay={(m) => {
                  setOverlayMode(m);
                  setOverlayManual(true);
                  setOverlaySeq((n) => n + 1);
                  setOverlayFromBar(true);
                  setPillView("overlay");
                  invoke("pill_open", { open: true, pinned: true, focus: false }).catch(() => {});
                }}
                onWidthChange={setPillBarWidth}
              />
            </div>
          ) : activeMode === "overlay" ? (
            <div key="overlay" className="notch__stage">
              <PillOverlay
                active={visible && pillView === "overlay"}
                mode={overlayMode}
                manual={overlayManual}
                openedAtSeq={overlaySeq}
                onBackToBar={
                  overlayFromBar
                    ? () => {
                        setPillView("bar");
                        invoke("pill_open", { open: true, pinned: false, focus: false }).catch(
                          () => {},
                        );
                      }
                    : undefined
                }
                onClose={closePill}
                onWidthChange={setPillOverlayWidth}
              />
            </div>
          ) : activeMode === "launcher" ? (
            <div key="launcher" className="notch__stage">
              <Launcher
                embedded
                initialView={launcherSub}
                onWidthChange={setPillLauncherWidth}
                onRequestClose={closePill}
              />
            </div>
          ) : activeMode === "overview" ? (
            <div key="overview" className="notch__stage">
              <WmOverview embedded onClose={closePill} />
            </div>
          ) : activeMode === "switcher" ? (
            <div key="switcher" className="notch__stage">
              <Switcher
                embedded
                externalState={swState}
                onWidthChange={setPillSwitcherWidth}
              />
            </div>
          ) : activeMode === "activities" ? (
            <div key="activities" className="notch__stage">
              <Activities
                embedded
                initialWins={activitiesWins}
                onRequestClose={closePill}
              />
            </div>
          ) : (
            <div key="full" className="notch__stage">
              <div className="tabs">
                {TABS.map((t) => (
                  <button
                    key={t.id}
                    className={`tabs__item ${tab === t.id ? "tabs__item--active" : ""}`}
                    onClick={() => setTab(t.id)}
                  >
                    <span className="tabs__icon-wrap">
                      <Icon name={t.icon} size={22} fill={tab === t.id} />
                      {t.id === "notification" && notifs.unreadCount > 0 && (
                        <span className="tabs__badge">
                          {notifs.unreadCount > 99 ? "99+" : notifs.unreadCount}
                        </span>
                      )}
                    </span>
                    {t.label}
                  </button>
                ))}
                {pillMode && (
                  <button
                    type="button"
                    className="tabs__item tabs__item--bar"
                    title="Switch to Top Bar"
                    onClick={() => setPillView("bar")}
                  >
                    <span className="tabs__icon-wrap">
                      <Icon name="arrow_back" size={20} />
                    </span>
                    bar
                  </button>
                )}
              </div>

              <div
                key={`${tab}:${tab === "settings" ? quickSubView : ""}`}
                className="notch__body notch__stage"
              >
                {tab === "dashboard" && (
                  <Dashboard stats={stats} media={media} gpu={gpu} weather={weather} visible={visible} />
                )}
                {tab === "media" && <MediaTab media={media} active={visible} />}
                {tab === "notification" && (
                  <NotificationsTab
                    items={notifs.items}
                    enabled={notifsEnabled}
                    onOpen={notifs.openNotif}
                    onMarkRead={notifs.markRead}
                    onClearAll={notifs.clearAll}
                  />
                )}
                {tab === "system" && (
                  <SystemTab stats={stats} procs={procs} gpu={gpu} hist={sysHist} />
                )}
                {tab === "settings" && (
                  <QuickSettings
                    wifi={wifi}
                    visible={visible}
                    cfg={cfg}
                    subView={quickSubView}
                    onSubView={setQuickSubView}
                    onInstantHide={() => setInstantHide(true)}
                  />
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
