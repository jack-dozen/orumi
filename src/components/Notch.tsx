import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import { useGpu } from "../useGpu";
import { useMedia, type MediaState } from "../useMedia";
import { useNow } from "../useNow";
import { useProcesses, type ProcEntry } from "../useProcesses";
import { useStats, type Stats } from "../useStats";
import { useWeather, type Weather } from "../useWeather";
import { useWifi, type WifiStatus } from "../useWifi";
import Gauge from "./Gauge";
import Icon from "./Icon";
import { MediaTab, MiniPlayer } from "./Media";
import WeatherCard from "./Weather";
import { useConfig, type Config } from "../useConfig";
import { useQuick, type PowerMode, type QuickKey, type QuickState } from "../useQuick";
import VolumeBrightness from "./VolumeBrightness";

const pad = (n: number) => String(n).padStart(2, "0");

const TABS = [
  { id: "dashboard", label: "dashboard", icon: "dashboard" },
  { id: "media", label: "media", icon: "radio" },
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

/** Notch "settings" tab = Windows quick settings. ORUMI's own settings live in the launcher (>settings). */
function QuickSettings({
  wifi,
  visible,
  cfg,
  onInstantHide,
}: {
  wifi: WifiStatus | null;
  visible: boolean;
  cfg: Config | null;
  onInstantHide: () => void;
}) {
  const { state, busy, toggle } = useQuick(visible);
  const autoTheme = cfg?.windows_theme_mode === "auto";
  const openPage = (page: string) => invoke("open_settings_page", { page }).catch(() => {});

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
                <span className="qtile__sub">
                  {!state
                    ? "loading…"
                    : missing
                      ? "unavailable"
                      : busy === t.key
                        ? "…"
                        : t.sub(state, wifi, autoTheme)}
                </span>
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

      <div className="quick__hint">Right-click a tile to open Windows Settings</div>

      <VolumeBrightness active={visible} />
    </div>
  );
}

type WsOsd = { active: number; total: number };

export default function Notch() {
  const visible = useEdgeVisible("notch");
  const [tab, setTab] = useState<TabId>("dashboard");
  const [instantHide, setInstantHide] = useState(false);
  const [wsOsd, setWsOsd] = useState<WsOsd | null>(null);
  const [osdOpen, setOsdOpen] = useState(false);
  const osdTimer = useRef<number | undefined>(undefined);

  // Fetch data only while the notch is visible and its tab uses it.
  const stats = useStats(visible && (tab === "dashboard" || tab === "system"));
  const media = useMedia(visible && (tab === "dashboard" || tab === "media"));
  const procs = useProcesses(visible && tab === "system");
  const gpu = useGpu(visible && (tab === "dashboard" || tab === "system"));
  const wifi = useWifi(visible && tab === "settings");
  const cfg = useConfig();
  const weather = useWeather(cfg?.weather_city ?? "Bandung");

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
  const width =
    tab === "dashboard"
      ? showMini
        ? 740
        : 550
      : tab === "media"
        ? 740
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
    if (height === null) return;
    invoke("notch_set_height", { height, width }).catch(() => {});
  }, [height, width]);

  // Notch closed: return to "does not take focus" mode (if it was used for typing).
  useEffect(() => {
    setInstantHide(false);
    if (!visible) invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
  }, [visible]);

  // Window lost focus (clicked another app): release the "currently typing" focus hold.
  useEffect(() => {
    const onBlur = () => {
      // The native color dialog makes the window lose focus; keep the focus hold during it.
      const el = document.activeElement as HTMLInputElement | null;
      if (el?.tagName === "INPUT" && el.type === "color") return;
      invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // The bar can ask the notch to switch to a specific tab (clock or WiFi icon click).
  useEffect(() => {
    const unlisten = listen<string>("notch-tab", (e) => {
      const [t] = e.payload.split("/");
      if (TABS.some((x) => x.id === t)) setTab(t as TabId);
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

  return (
    <>
      {wsOsd && (
        <div
          className={`wsosd ${osdOpen && !visible ? "wsosd--visible" : ""}`}
          aria-hidden="true"
        >
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
      )}
      <div
        className={`notch ${visible && !instantHide ? "notch--visible" : ""} ${
          instantHide ? "notch--instant" : ""
        }`}
        style={{ width, height: height ?? undefined }}
      >
      <div className="notch__inner" ref={innerRef}>
        <div className="tabs">
          {TABS.map((t) => (
            <button
              key={t.id}
              className={`tabs__item ${tab === t.id ? "tabs__item--active" : ""}`}
              onClick={() => setTab(t.id)}
            >
              <Icon name={t.icon} size={22} fill={tab === t.id} />
              {t.label}
            </button>
          ))}
        </div>

        <div className="notch__body">
          {tab === "dashboard" && (
            <Dashboard stats={stats} media={media} gpu={gpu} weather={weather} visible={visible} />
          )}
          {tab === "media" && <MediaTab media={media} active={visible} />}
          {tab === "system" && (
            <SystemTab stats={stats} procs={procs} gpu={gpu} hist={sysHist} />
          )}
          {tab === "settings" && (
            <QuickSettings
              wifi={wifi}
              visible={visible}
              cfg={cfg}
              onInstantHide={() => setInstantHide(true)}
            />
          )}
        </div>
      </div>
      </div>
    </>
  );
}
