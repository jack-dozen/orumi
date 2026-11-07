import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
import AppSettings, { ColorSettings } from "./Settings";

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

/** Sel kalender sebulan penuh: null = kosong, angka = tanggal. */
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
}: {
  stats: Stats | null;
  media: MediaState;
  gpu: number | null;
  weather: Weather | null;
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
      <div className="card card--placeholder">volume · brightness</div>
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

function QuickSettings({ wifi }: { wifi: WifiStatus | null }) {
  const openPage = (page: string) => invoke("open_settings_page", { page });
  const wifiOn = !!wifi?.adapter && wifi.connected;

  return (
    <div className="card settings">
      <div className="settings__tile">
        <Icon name={wifiOn ? "wifi" : "wifi_off"} size={28} className="settings__icon" />
        <div className="settings__text">
          <div className="settings__title">WiFi</div>
          <div className="settings__sub">{wifiLabel(wifi)}</div>
        </div>
        <button className="settings__btn" onClick={() => openPage("wifi")}>
          Pengaturan
        </button>
      </div>

      {wifi?.location_denied && (
        <div className="settings__warn">
          Windows memblokir akses WiFi untuk app ini. Aktifkan "Let desktop apps access
          your location" di pengaturan lokasi.{" "}
          <button className="settings__link" onClick={() => openPage("location")}>
            Buka pengaturan lokasi
          </button>
        </div>
      )}

      <div className="settings__todo">
        Kontrol lain (bluetooth, mode pesawat, dll.) belum dibuat
      </div>
    </div>
  );
}

type SettingsSection = "quick" | "app" | "color";

function SettingsTab({
  wifi,
  section,
  onSection,
}: {
  wifi: WifiStatus | null;
  section: SettingsSection;
  onSection: (s: SettingsSection) => void;
}) {
  const sections: [SettingsSection, string][] = [
    ["quick", "Cepat"],
    ["app", "Aplikasi"],
    ["color", "Warna"],
  ];
  return (
    <div className="settingsroot">
      <div className="segmented">
        {sections.map(([id, label]) => (
          <button
            key={id}
            className={section === id ? "segmented__on" : ""}
            onClick={() => onSection(id)}
          >
            {label}
          </button>
        ))}
      </div>
      {section === "quick" && <QuickSettings wifi={wifi} />}
      {section === "app" && <AppSettings />}
      {section === "color" && <ColorSettings />}
    </div>
  );
}

export default function Notch() {
  const visible = useEdgeVisible("notch");
  const [tab, setTab] = useState<TabId>("dashboard");

  // Data hanya diambil saat notch terlihat dan tab-nya memakainya.
  const stats = useStats(visible && (tab === "dashboard" || tab === "system"));
  const media = useMedia(visible && (tab === "dashboard" || tab === "media"));
  const procs = useProcesses(visible && tab === "system");
  const gpu = useGpu(visible && (tab === "dashboard" || tab === "system"));
  const wifi = useWifi(visible && (tab === "system" || tab === "settings"));
  const cfg = useConfig();
  const weather = useWeather(cfg?.weather_city ?? "Bandung");
  const [section, setSection] = useState<SettingsSection>("quick");

  // Notch ditutup: kembalikan ke mode "tidak merebut fokus" (kalau sempat dipakai mengetik).
  useEffect(() => {
    if (!visible) invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
  }, [visible]);

  // Window kehilangan fokus (klik ke app lain): lepas penahan "sedang mengetik".
  useEffect(() => {
    const onBlur = () => {
      // Dialog warna native membuat window kehilangan fokus; jangan lepas penahan selama itu.
      const el = document.activeElement as HTMLInputElement | null;
      if (el?.tagName === "INPUT" && el.type === "color") return;
      invoke("panel_activation", { label: "notch", enable: false }).catch(() => {});
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Bar bisa meminta notch pindah ke tab tertentu (klik jam atau ikon WiFi).
  useEffect(() => {
    const unlisten = listen<string>("notch-tab", (e) => {
      const [t, sub] = e.payload.split("/");
      if (TABS.some((x) => x.id === t)) setTab(t as TabId);
      if (t === "settings" && (sub === "quick" || sub === "app" || sub === "color")) {
        setSection(sub);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div className={`notch ${visible ? "notch--visible" : ""}`}>
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
          <Dashboard stats={stats} media={media} gpu={gpu} weather={weather} />
        )}
        {tab === "media" && <MediaTab media={media} />}
        {tab === "system" && <SystemTab stats={stats} procs={procs} gpu={gpu} wifi={wifi} />}
        {tab === "settings" && (
          <SettingsTab wifi={wifi} section={section} onSection={setSection} />
        )}
      </div>
    </div>
  );
}
