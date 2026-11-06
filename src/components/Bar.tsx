import { invoke } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import { useBattery, type BatteryStatus } from "../useBattery";
import { useEdgeVisible } from "../useEdgeVisible";
import { useNow } from "../useNow";
import { useWifi } from "../useWifi";
import { useWindows, type AppGroup } from "../useWindows";
import Icon from "./Icon";

const pad = (n: number) => String(n).padStart(2, "0");

/** Pilih tab di notch, lalu buka (atau tutup) notch-nya. */
async function notchTab(tab: string, toggle: boolean) {
  await emitTo("notch", "notch-tab", tab);
  if (toggle) invoke("toggle_panel", { label: "notch" });
  else invoke("set_panel", { label: "notch", open: true });
}

/** Klik titik app: pindah ke app itu; kalau sudah fokus, minimalkan atau ganti window. */
function onAppClick(g: AppGroup) {
  if (!g.focused) {
    invoke("focus_window", { hwnd: g.windows[0].hwnd }).catch(() => {});
    return;
  }
  if (g.windows.length > 1) {
    const i = g.windows.findIndex((w) => w.focused);
    const next = g.windows[(i + 1) % g.windows.length];
    invoke("focus_window", { hwnd: next.hwnd }).catch(() => {});
  } else {
    invoke("minimize_window", { hwnd: g.windows[0].hwnd });
  }
}

function batteryIcon(b: BatteryStatus): string {
  if (b.charging || b.plugged) return "battery_charging_full";
  const p = b.percent;
  if (p >= 95) return "battery_full";
  if (p >= 80) return "battery_6_bar";
  if (p >= 65) return "battery_5_bar";
  if (p >= 50) return "battery_4_bar";
  if (p >= 35) return "battery_3_bar";
  if (p >= 20) return "battery_2_bar";
  if (p >= 8) return "battery_1_bar";
  return "battery_alert";
}

export default function Bar() {
  const visible = useEdgeVisible("bar");
  const now = useNow();
  const wifi = useWifi(visible);
  const battery = useBattery(visible);
  const groups = useWindows(visible);

  const wifiOn = !!wifi?.adapter && wifi.connected;
  const wifiTitle = !wifi
    ? "WiFi"
    : !wifi.adapter
      ? "Tidak ada adapter WiFi"
      : wifi.connected
        ? `${wifi.ssid ?? "Tersambung"} · ${wifi.signal}%`
        : "WiFi tidak tersambung";

  return (
    <div className={`bar ${visible ? "bar--visible" : ""}`}>
      <div className="bar__group">
        <div
          className="bar__item"
          title="Launcher"
          onClick={() => invoke("toggle_panel", { label: "launcher" })}
        >
          <Icon name="apps" size={26} />
        </div>
        <div className="bar__item" title="Workspace (belum dibuat)">
          <Icon name="more_vert" size={22} />
        </div>
      </div>

      <div className="bar__apps">
        {groups.map((g) => {
          const shown = g.windows.find((w) => w.focused) ?? g.windows[0];
          return (
            <div
              key={g.key}
              className={`appdot ${g.focused ? "appdot--focus" : ""} ${
                g.allMinimized ? "appdot--min" : ""
              }`}
              title={`${g.name}${g.windows.length > 1 ? ` (${g.windows.length})` : ""} — ${shown.title}`}
              onClick={() => onAppClick(g)}
            />
          );
        })}
      </div>

      <div className="bar__group">
        <div className="bar__item bar__clock" onClick={() => notchTab("dashboard", true)}>
          {pad(now.getHours())}
          <br />
          {pad(now.getMinutes())}
        </div>
        <div className="bar__status">
          <div
            className={`bar__item ${wifiOn ? "" : "bar__off"}`}
            title={wifiTitle}
            onClick={() => notchTab("settings", false)}
          >
            <Icon name={wifiOn ? "wifi" : "wifi_off"} size={20} />
          </div>
          <div
            className="bar__item"
            title="Volume"
            onClick={() => invoke("overlay_toggle", { mode: "mixer" })}
          >
            <Icon name="volume_up" size={20} />
          </div>
          {battery && (
            <div
              className="bar__item bar__battery"
              title={`${battery.percent}%${
                battery.charging ? " · mengisi" : battery.plugged ? " · tersambung ke listrik" : ""
              }`}
            >
              <Icon name={batteryIcon(battery)} size={20} />
              <span className="bar__pct">{battery.percent}</span>
            </div>
          )}
        </div>
        <div
          className="bar__item"
          title="Power"
          onClick={() => invoke("overlay_toggle", { mode: "power" })}
        >
          <Icon name="power_settings_new" size={24} />
        </div>
      </div>
    </div>
  );
}
