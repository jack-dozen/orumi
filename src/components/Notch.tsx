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

/** Calendar cell for a full month: null = empty, number = date. */
function monthCells(now: Date): (number | null)[] {
  const first = new Date(now.getFullYear(), now.getMonth(), 1).getDay();
  const total = new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate();
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
  const ramPct = stats ? (stats.mem_used / stats.mem_total) * 100 : null;

  return (
    <div className="dash">
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

      <div className="card calendar">
        {DAYS.map((d, i) => (
          <div key={d} className={`calendar__head ${i === 0 ? "calendar__sun" : ""}`}>
            {d}
          </div>
        ))}
        {monthCells(now).map((d, i) => (
          <div
            key={i}
            className={`calendar__day ${i % 7 === 0 ? "calendar__sun" : ""} ${
              d === today ? "calendar__today" : ""
            }`}
          >
            {d}
          </div>
        ))}
      </div>

      <MiniPlayer media={media} />
      <VolumeBrightness active={visible} vertical />
    </div>
  );
}

function StatBar({ label, pct, detail }: { label: string; pct: number; detail: string }) {
  return (
    <div className="sysrow">
      <div className="sysrow__head">
        <span>{label}</span>
        <span>{detail}</span>
      </div>
      <div className="sysrow__track">
        <div className="sysrow__fill" style={{ width: `${Math.min(100, pct)}%` }} />
      </div>
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
  wifi,
}: {
  stats: Stats | null;
  procs: ProcEntry[];
  gpu: number | null;
  wifi: WifiStatus | null;
}) {
  if (!stats) return <div className="card card--placeholder card--fill">loading…</div>;

  return (
    <div className="card system">
      <div className="system__bars">
        <StatBar label="CPU" pct={stats.cpu} detail={`${Math.round(stats.cpu)}%`} />
        <StatBar
          label="RAM"
          pct={(stats.mem_used / stats.mem_total) * 100}
          detail={`${gib(stats.mem_used)} / ${gib(stats.mem_total)} GB`}
        />
        <StatBar
          label="Disk C:"
          pct={stats.disk_total ? (stats.disk_used / stats.disk_total) * 100 : 0}
          detail={`${gib(stats.disk_used)} / ${gib(stats.disk_total)} GB`}
        />
        <StatBar
          label="GPU"
          pct={gpu ?? 0}
          detail={gpu === null ? "—" : `${Math.round(gpu)}%`}
        />
        <StatBar
          label="WiFi"
          pct={wifi?.connected ? wifi.signal : 0}
          detail={wifiLabel(wifi)}
        />
      </div>

      <div className="procs">
        <div className="procs__title">Top processes</div>
        {procs.length === 0 && <div className="procs__empty">loading…</div>}
        {procs.map((p) => (
          <div className="proc" key={p.name}>
            <span className="proc__name">
              {p.name}
              {p.count > 1 ? ` ×${p.count}` : ""}
            </span>
            <span className="proc__cpu">{p.cpu.toFixed(1)}%</span>
            <span className="proc__mem">{fmtMem(p.mem)}</span>
          </div>
        ))}
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

export default function Notch() {
  const visible = useEdgeVisible("notch");
  const [tab, setTab] = useState<TabId>("dashboard");
  const [instantHide, setInstantHide] = useState(false);

  // Fetch data only while the notch is visible and its tab uses it.
  const stats = useStats(visible && (tab === "dashboard" || tab === "system"));
  const media = useMedia(visible && (tab === "dashboard" || tab === "media"));
  const procs = useProcesses(visible && tab === "system");
  const gpu = useGpu(visible && (tab === "dashboard" || tab === "system"));
  const wifi = useWifi(visible && (tab === "system" || tab === "settings"));
  const cfg = useConfig();
  const weather = useWeather(cfg?.weather_city ?? "Bandung");

  // Follow content height dynamically (keeps short tabs like quick settings compact).
  const innerRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const measure = () => {
      const h = el.offsetHeight;
      setHeight(h);
      invoke("notch_set_height", { height: h }).catch(() => {});
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

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

  return (
    <div
      className={`notch ${visible && !instantHide ? "notch--visible" : ""} ${
        instantHide ? "notch--instant" : ""
      }`}
      style={{ height: height ?? undefined }}
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
          {tab === "media" && <MediaTab media={media} />}
          {tab === "system" && <SystemTab stats={stats} procs={procs} gpu={gpu} wifi={wifi} />}
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
  );
}
