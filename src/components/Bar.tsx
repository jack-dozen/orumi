import { useEffect, useRef, type MouseEvent as ReactMouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emitTo, listen } from "@tauri-apps/api/event";
import { useBattery, type BatteryStatus } from "../useBattery";
import { useConfig } from "../useConfig";
import { useEdgeVisible } from "../useEdgeVisible";
import { useNow } from "../useNow";
import { useWifi } from "../useWifi";
import { useWindows, windowIconTarget, type AppGroup } from "../useWindows";
import AppIcon from "./AppIcon";
import Icon from "./Icon";

const pad = (n: number) => String(n).padStart(2, "0");

/** Click an app to switch to it; if already focused, minimize it or switch windows. */
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
  const cfg = useConfig();
  const wifi = useWifi(visible);
  const battery = useBattery(visible);
  const groups = useWindows(visible);
  const asIcons = (cfg?.bar_app_style ?? "icons") === "icons";
  const previewOpen = useEdgeVisible("preview");
  const notchOpen = useEdgeVisible("notch");
  const activeNotchTab = useRef<string>("dashboard");
  const hoverTimer = useRef<number | undefined>(undefined);

  useEffect(() => {
    const unlisten = listen<string>("notch-tab-active", (e) => {
      activeNotchTab.current = e.payload;
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  /** Close notch if already open on `tab`; otherwise switch to `tab` and keep/open it. */
  async function handleNotchTab(tab: string) {
    if (notchOpen && activeNotchTab.current === tab) {
      invoke("set_panel", { label: "notch", open: false }).catch(() => {});
      return;
    }
    activeNotchTab.current = tab;
    await emitTo("notch", "notch-tab", tab);
    if (!notchOpen) {
      invoke("toggle_panel", { label: "notch" }).catch(() => {});
    } else {
      invoke("set_panel", { label: "notch", open: true }).catch(() => {});
    }
  }

  // Bar hidden: close the thumbnail preview.
  useEffect(() => {
    if (!visible) invoke("preview_close").catch(() => {});
  }, [visible]);

  const hoverApp = (g: AppGroup) => (e: ReactMouseEvent<HTMLElement>) => {
    const el = e.currentTarget;
    window.clearTimeout(hoverTimer.current);
    // Hover intent: wait briefly; if the preview is already open, update its contents almost immediately.
    hoverTimer.current = window.setTimeout(
      () => {
        const r = el.getBoundingClientRect();
        invoke("preview_open", {
          items: g.windows.slice(0, 4).map((w) => ({
            hwnd: w.hwnd,
            title: w.title,
            icon: windowIconTarget(w),
          })),
          anchorY: r.top + r.height / 2,
        }).catch(() => {});
      },
      previewOpen ? 60 : 260,
    );
  };
  const leaveApp = () => {
    window.clearTimeout(hoverTimer.current);
    invoke("preview_close_soon").catch(() => {});
  };

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
          const label = `${g.name}${g.windows.length > 1 ? ` (${g.windows.length})` : ""}`;
          const click = () => {
            invoke("preview_close").catch(() => {});
            onAppClick(g);
          };
          return asIcons ? (
            <div
              key={g.key}
              className={`appicon ${g.focused ? "appicon--focus" : ""} ${
                g.allMinimized ? "appicon--min" : ""
              }`}
              title={`${label} — ${shown.title}`}
              onClick={click}
              onMouseEnter={hoverApp(g)}
              onMouseLeave={leaveApp}
            >
              <AppIcon target={g.icon} fallback={g.name.charAt(0).toUpperCase()} size={24} />
            </div>
          ) : (
            <div
              key={g.key}
              className={`appdot ${g.focused ? "appdot--focus" : ""} ${
                g.allMinimized ? "appdot--min" : ""
              }`}
              title={`${label} — ${shown.title}`}
              onClick={click}
              onMouseEnter={hoverApp(g)}
              onMouseLeave={leaveApp}
            />
          );
        })}
      </div>

      <div className="bar__group">
        <div
          className="bar__item"
          title="Ikon tray (ikon tersembunyi)"
          onClick={() => invoke("tray_toggle").catch(() => {})}
        >
          <Icon name="chevron_right" size={22} />
        </div>
        <div className="bar__item bar__clock" onClick={() => handleNotchTab("dashboard")}>
          {pad(now.getHours())}
          <br />
          {pad(now.getMinutes())}
        </div>
        <div className="bar__status">
          <div
            className={`bar__item ${wifiOn ? "" : "bar__off"}`}
            title={wifiTitle}
            onClick={() => handleNotchTab("settings")}
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
