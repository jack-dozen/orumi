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
import { useConfig } from "../useConfig";
import { useQuick, type QuickKey } from "../useQuick";
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
  if (!wifi) return "memuat…";
  if (!wifi.adapter) return "tidak ada adapter";
  if (wifi.location_denied) return "akses diblokir Windows";
  if (!wifi.connected) return "tidak tersambung";
  return `${wifi.ssid ?? "tersambung"} · ${wifi.signal}%`;
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
  if (!stats) return <div className="card card--placeholder card--fill">memuat…</div>;

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
        <div className="procs__title">Proses teratas</div>
        {procs.length === 0 && <div className="procs__empty">memuat…</div>}
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

type QuickTile = {
  key: QuickKey;
  title: string;
  icon: string;
  iconOff: string;
  sub: (on: boolean, wifi: WifiStatus | null) => string;
};

const QUICK_TILES: QuickTile[] = [
  {
    key: "wifi",
    title: "Wi-Fi",
    icon: "wifi",
    iconOff: "wifi_off",
    sub: (on, wifi) => (on ? wifiLabel(wifi) : "Mati"),
  },
  {
    key: "bluetooth",
    title: "Bluetooth",
    icon: "bluetooth",
    iconOff: "bluetooth_disabled",
    sub: (on) => (on ? "Nyala" : "Mati"),
  },
  {
    key: "airplane",
    title: "Mode pesawat",
    icon: "airplanemode_active",
    iconOff: "airplanemode_active",
    sub: (on) => (on ? "Nyala" : "Mati"),
  },
  {
    key: "dnd",
    title: "Jangan ganggu",
    icon: "do_not_disturb_on",
    iconOff: "do_not_disturb_on",
    sub: (on) => (on ? "Notifikasi dibisukan" : "Mati"),
  },
];

/** Notch "settings" tab = Windows quick settings. ORUMI's own settings live in the launcher (>settings). */
function QuickSettings({ wifi, visible }: { wifi: WifiStatus | null; visible: boolean }) {
  const { state, busy, toggle } = useQuick(visible);
  const openPage = (page: string) => invoke("open_settings_page", { page });

  return (
    <div className="quick">
      <div className="quick__tiles">
        {QUICK_TILES.map((t) => {
          const value = state ? state[t.key] : null;
          const missing = state !== null && value === null; // no such radio on this PC
          const on = value === true;
          return (
            <button
              key={t.key}
              className={`qtile ${on ? "qtile--on" : ""} ${missing ? "qtile--missing" : ""}`}
              disabled={!state || missing || busy !== null}
              onClick={() => toggle(t.key, !on)}
            >
              <Icon name={on ? t.icon : t.iconOff} size={26} fill={on} />
              <span className="qtile__title">{t.title}</span>
              <span className="qtile__sub">
                {!state ? "memuat…" : missing ? "tidak ada" : busy === t.key ? "…" : t.sub(on, wifi)}
              </span>
            </button>
          );
        })}
      </div>

      {wifi?.location_denied && (
        <div className="settings__warn">
          Windows memblokir akses WiFi untuk app ini. Aktifkan "Let desktop apps access your
          location" di pengaturan lokasi.{" "}
          <button className="settings__link" onClick={() => openPage("location")}>
            Buka pengaturan lokasi
          </button>
        </div>
      )}

      <VolumeBrightness active={visible} />
    </div>
  );
}

export default function Notch() {
  const visible = useEdgeVisible("notch");
  const [tab, setTab] = useState<TabId>("dashboard");

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
      className={`notch ${visible ? "notch--visible" : ""}`}
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
          {tab === "settings" && <QuickSettings wifi={wifi} visible={visible} />}
        </div>
      </div>
    </div>
  );
}
