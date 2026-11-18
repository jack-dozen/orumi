import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type WheelEvent as ReactWheelEvent,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Workspace } from "../activities/useActivities";
import { useBattery, type BatteryStatus } from "../useBattery";
import type { Config } from "../useConfig";
import { useNow } from "../useNow";
import type { PowerMode, QuickState } from "../useQuick";
import { useWifi } from "../useWifi";
import { useWindows, windowIconTarget, type AppGroup } from "../useWindows";
import AppIcon from "./AppIcon";
import Icon from "./Icon";
import Thumb from "./Thumb";

export type PillTabId = "dashboard" | "media" | "notification" | "system" | "settings";
export type PillOverlayMode = "power" | "mixer" | "volume" | "brightness";

type PreviewItem = {
  hwnd: number;
  title: string;
  icon: string | null;
  thumb: string | null;
  minimized: boolean;
};

type Vol = { volume: number; muted: boolean };

const pad = (n: number) => String(n).padStart(2, "0");

const POWER_LABEL: Record<PowerMode, string> = {
  saver: "Power saver",
  balanced: "Balanced",
  performance: "Best performance",
};

const POWER_BADGE: Record<PowerMode, string> = {
  saver: "ECO",
  balanced: "BAL",
  performance: "PERF",
};

const POWER_ITEMS = [
  { id: "lock", label: "Lock", icon: "lock" },
  { id: "sleep", label: "Sleep", icon: "bedtime" },
  { id: "hibernate", label: "Hibernate", icon: "schedule" },
  { id: "shutdown", label: "Shutdown", icon: "power_settings_new", dangerous: true },
  { id: "restart", label: "Restart", icon: "restart_alt", dangerous: true },
];

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

export function PillBar({
  active,
  cfg,
  onCollapseToCompact,
  onOpenFullNotch,
  onOpenLauncher,
  onOpenOverview,
  onOpenOverlay,
  onWidthChange,
}: {
  active: boolean;
  cfg: Config | null;
  onCollapseToCompact: () => void;
  onOpenFullNotch: (tab?: PillTabId) => void;
  onOpenLauncher: () => void;
  onOpenOverview: () => void;
  onOpenOverlay: (mode: PillOverlayMode) => void;
  onWidthChange: (w: number) => void;
}) {
  const now = useNow();
  const wifi = useWifi(active);
  const battery = useBattery(active);
  const groups = useWindows(active);
  const asIcons = (cfg?.bar_app_style ?? "icons") === "icons";

  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [powerMode, setPowerMode] = useState<PowerMode>("balanced");
  const [powerBadge, setPowerBadge] = useState<string | null>(null);
  const badgeTimer = useRef<number | undefined>(undefined);
  const wsWheel = useRef({ acc: 0, lastAt: 0, locked: false, lockedAt: 0, inFlight: false });

  const [previewItems, setPreviewItems] = useState<PreviewItem[]>([]);
  const hoverTimer = useRef<number | undefined>(undefined);
  const closePreviewTimer = useRef<number | undefined>(undefined);
  const rowRef = useRef<HTMLDivElement>(null);
  const appsRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const loadWs = () => {
      invoke<Workspace[]>("ws_list")
        .then(setWorkspaces)
        .catch(() => {});
    };
    loadWs();
    const unlistenWs = listen("workspaces-changed", loadWs);
    const unlistenQuick = listen<QuickState>("quick-changed", (e) => {
      setPowerMode(e.payload.power_mode);
    });
    const unlistenPreview = listen<PreviewItem[]>("pill-preview-data", (e) => {
      setPreviewItems(e.payload ?? []);
    });
    return () => {
      unlistenWs.then((fn) => fn());
      unlistenQuick.then((fn) => fn());
      unlistenPreview.then((fn) => fn());
      window.clearTimeout(badgeTimer.current);
      window.clearTimeout(hoverTimer.current);
      window.clearTimeout(closePreviewTimer.current);
    };
  }, []);

  useEffect(() => {
    if (!active) {
      window.clearTimeout(hoverTimer.current);
      window.clearTimeout(closePreviewTimer.current);
      setPreviewItems([]);
      return;
    }
    invoke<Workspace[]>("ws_list")
      .then(setWorkspaces)
      .catch(() => {});
    invoke<QuickState>("quick_get")
      .then((s) => setPowerMode(s.power_mode))
      .catch(() => {});
  }, [active]);

  const activeWsIdx = Math.max(
    0,
    workspaces.findIndex((w) => w.active),
  );
  const wsDotsCount = Math.max(workspaces.length, activeWsIdx + 1, 1);

  const reportWidth = useCallback(() => {
    const maxW = Math.max(720, window.innerWidth - 36);
    const normalBarW = Math.min(maxW, Math.max(920, Math.round(window.screen.availHeight - 48)));
    const appsEl = appsRef.current;
    const appsW = appsEl
      ? Array.from(appsEl.children).reduce(
          (sum, el) => sum + (el as HTMLElement).offsetWidth,
          Math.max(0, appsEl.children.length - 1) * 8,
        )
      : 0;
    const neededW = appsW + 420;
    const pvCount = Math.min(4, previewItems.length);
    const pvW = pvCount > 0 ? pvCount * 196 + Math.max(0, pvCount - 1) * 10 + 36 : 0;
    onWidthChange(Math.min(maxW, Math.max(normalBarW, neededW, pvW)));
  }, [previewItems.length, onWidthChange]);

  useLayoutEffect(() => {
    reportWidth();
  }, [reportWidth, groups.length, wsDotsCount, battery, powerBadge]);

  useLayoutEffect(() => {
    const rowEl = rowRef.current;
    const appsEl = appsRef.current;
    if (!rowEl && !appsEl) return;
    const ro = new ResizeObserver(reportWidth);
    if (rowEl) ro.observe(rowEl);
    if (appsEl) ro.observe(appsEl);
    return () => ro.disconnect();
  }, [reportWidth]);

  const cyclePower = () => {
    invoke<QuickState>("cycle_power_mode")
      .then((s) => {
        setPowerMode(s.power_mode);
        setPowerBadge(POWER_BADGE[s.power_mode]);
        window.clearTimeout(badgeTimer.current);
        badgeTimer.current = window.setTimeout(() => setPowerBadge(null), 1500);
      })
      .catch(() => {});
  };

  const onWsWheel = (e: ReactWheelEvent<HTMLDivElement>) => {
    e.preventDefault();
    if (workspaces.length <= 1) return;
    const nowMs = performance.now();
    const s = wsWheel.current;
    const gap = nowMs - s.lastAt;
    s.lastAt = nowMs;
    if (s.locked) {
      if (gap > 110 && nowMs >= s.lockedAt + 180) {
        s.locked = false;
        s.acc = 0;
      } else {
        return;
      }
    }
    if (gap > 110) s.acc = 0;
    const delta = Math.abs(e.deltaY) >= Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
    s.acc += delta;
    if (Math.abs(s.acc) < 40) return;

    const dir = s.acc > 0 ? 1 : -1;
    s.acc = 0;
    s.locked = true;
    s.lockedAt = nowMs;

    const targetIdx = Math.max(0, Math.min(workspaces.length - 1, activeWsIdx + dir));
    if (targetIdx === activeWsIdx || s.inFlight) return;
    const target = workspaces[targetIdx];
    if (!target) return;

    s.inFlight = true;
    invoke("ws_switch", { id: target.id })
      .catch(() => {})
      .finally(() => {
        s.inFlight = false;
      });
  };

  const hoverApp = (g: AppGroup) => () => {
    window.clearTimeout(hoverTimer.current);
    window.clearTimeout(closePreviewTimer.current);
    hoverTimer.current = window.setTimeout(
      () => {
        invoke("pill_preview_open", {
          items: g.windows.slice(0, 4).map((w) => ({
            hwnd: w.hwnd,
            title: w.title,
            icon: windowIconTarget(w),
          })),
        }).catch(() => {});
      },
      previewItems.length > 0 ? 50 : 220,
    );
  };

  const leavePreviewZone = () => {
    window.clearTimeout(hoverTimer.current);
    window.clearTimeout(closePreviewTimer.current);
    closePreviewTimer.current = window.setTimeout(() => {
      setPreviewItems([]);
    }, 260);
  };

  const keepPreviewZone = () => {
    window.clearTimeout(closePreviewTimer.current);
  };

  const handleEmptyClick = (e: ReactMouseEvent<HTMLElement>) => {
    const el = e.target as HTMLElement;
    if (
      el.closest(
        "button, .pbar__btn, .pbar__ws, .pbar__status, .pbar__appicon, .pbar__appdot, .pbar__preview",
      )
    ) {
      return;
    }
    onCollapseToCompact();
  };

  const wifiOn = !!wifi?.adapter && wifi.connected;
  const wifiTitle = !wifi
    ? "WiFi"
    : !wifi.adapter
      ? "No Wi-Fi adapter"
      : wifi.connected
        ? `${wifi.ssid ?? "Connected"} · ${wifi.signal}%`
        : "Wi-Fi disconnected";

  return (
    <div className="pbar" onClick={handleEmptyClick}>
      <div className="pbar__row" ref={rowRef}>
        <div className="pbar__group">
          <button
            type="button"
            className="pbar__btn"
            title="Launcher"
            aria-label="Launcher"
            onClick={onOpenLauncher}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="var(--primary)" aria-hidden="true">
              <rect x="2" y="2" width="9.5" height="9.5" rx="1" />
              <rect x="12.5" y="2" width="9.5" height="9.5" rx="1" />
              <rect x="2" y="12.5" width="9.5" height="9.5" rx="1" />
              <rect x="12.5" y="12.5" width="9.5" height="9.5" rx="1" />
            </svg>
          </button>
          <div
            className="pbar__ws"
            title="Workspaces · Click dot to switch · Click pill for overview"
            onClick={onOpenOverview}
            onWheel={onWsWheel}
          >
            {Array.from({ length: wsDotsCount }, (_, i) => {
              const target = workspaces[i];
              return (
                <button
                  key={i}
                  type="button"
                  className={`pbar__ws-dot ${i === activeWsIdx ? "pbar__ws-dot--active" : ""}`}
                  title={`Workspace ${i + 1}`}
                  aria-label={`Workspace ${i + 1}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (target && i !== activeWsIdx) {
                      invoke("ws_switch", { id: target.id }).catch(() => {});
                    }
                  }}
                />
              );
            })}
          </div>
        </div>

        <div
          className="pbar__apps"
          ref={appsRef}
          onMouseEnter={keepPreviewZone}
          onMouseLeave={leavePreviewZone}
        >
          {groups.map((g) => {
            const shown = g.windows.find((w) => w.focused) ?? g.windows[0];
            const label = `${g.name}${g.windows.length > 1 ? ` (${g.windows.length})` : ""}`;
            const click = () => {
              setPreviewItems([]);
              onAppClick(g);
            };
            return asIcons ? (
              <button
                key={g.key}
                type="button"
                className={`pbar__appicon ${g.focused ? "pbar__appicon--focus" : ""} ${
                  g.allMinimized ? "pbar__appicon--min" : ""
                }`}
                title={`${label} — ${shown.title}`}
                onClick={click}
                onMouseEnter={hoverApp(g)}
              >
                <AppIcon target={g.icon} fallback={g.name.charAt(0).toUpperCase()} size={24} />
              </button>
            ) : (
              <button
                key={g.key}
                type="button"
                className={`pbar__appdot ${g.focused ? "pbar__appdot--focus" : ""} ${
                  g.allMinimized ? "pbar__appdot--min" : ""
                }`}
                title={`${label} — ${shown.title}`}
                onClick={click}
                onMouseEnter={hoverApp(g)}
              />
            );
          })}
        </div>

        <div className="pbar__group">
          <button
            type="button"
            className="pbar__clock"
            title="Open Full Notch"
            onClick={() => onOpenFullNotch("dashboard")}
          >
            {pad(now.getHours())}:{pad(now.getMinutes())}
          </button>
          <button
            type="button"
            className="pbar__btn"
            title="Tray icons (hidden icons)"
            aria-label="Tray icons"
            onClick={(e) => {
              const r = e.currentTarget.getBoundingClientRect();
              invoke("tray_toggle", {
                anchorX: r.left + r.width / 2,
                anchorY: r.bottom + 8,
              }).catch(() => {});
            }}
          >
            <Icon name="expand_more" size={18} />
          </button>
          <div className="pbar__status">
            <button
              type="button"
              className={`pbar__btn ${wifiOn ? "" : "bar__off"}`}
              title={wifiTitle}
              aria-label="Wi-Fi settings"
              onClick={() => onOpenFullNotch("settings")}
            >
              <Icon name={wifiOn ? "wifi" : "wifi_off"} size={17} />
            </button>
            <button
              type="button"
              className="pbar__btn"
              title="Volume & brightness"
              aria-label="Volume"
              onClick={() => onOpenOverlay("mixer")}
            >
              <Icon name="volume_up" size={17} />
            </button>
            {battery && (
              <button
                type="button"
                className={`pbar__btn pbar__battery ${
                  powerMode === "performance"
                    ? "bar__battery--perf"
                    : powerMode === "saver"
                      ? "bar__battery--saver"
                      : ""
                }`}
                title={`${battery.percent}%${
                  battery.charging ? " · charging" : battery.plugged ? " · plugged in" : ""
                } · Power mode: ${POWER_LABEL[powerMode]} (click to switch)`}
                onClick={cyclePower}
                onContextMenu={(e) => {
                  e.preventDefault();
                  onOpenFullNotch("settings");
                }}
              >
                <Icon name={batteryIcon(battery)} size={17} />
                <span className="pbar__pct">{powerBadge ?? `${battery.percent}%`}</span>
              </button>
            )}
          </div>
          <button
            type="button"
            className="pbar__btn"
            title="Power menu"
            aria-label="Power menu"
            onClick={() => onOpenOverlay("power")}
          >
            <Icon name="power_settings_new" size={18} />
          </button>
        </div>
      </div>

      {previewItems.length > 0 && (
        <div
          className="pbar__preview"
          onMouseEnter={keepPreviewZone}
          onMouseLeave={leavePreviewZone}
        >
          {previewItems.map((it) => (
            <button
              key={it.hwnd}
              type="button"
              className="pbar__pvcard"
              onClick={() => {
                invoke("focus_window", { hwnd: it.hwnd }).catch(() => {});
                setPreviewItems([]);
              }}
            >
              <div className="pbar__pvthumb">
                {it.thumb ? (
                  <Thumb src={convertFileSrc(it.thumb)} />
                ) : (
                  <div className="pvthumb__empty">
                    <AppIcon target={it.icon} fallback="?" size={40} />
                  </div>
                )}
                {it.minimized && <span className="pvbadge">minimized</span>}
              </div>
              <div className="pvtitle">
                <AppIcon target={it.icon} fallback="•" size={15} />
                <span>{it.title}</span>
              </div>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function HorizontalSlider({
  value,
  disabled,
  onChange,
  onDragging,
  icon,
  onIconClick,
  label,
}: {
  value: number;
  disabled?: boolean;
  onChange: (v: number) => void;
  onDragging?: (d: boolean) => void;
  icon: string;
  onIconClick?: () => void;
  label: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  const calc = (clientX: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.max(0, Math.min(100, ((clientX - r.left) / r.width) * 100));
  };

  const endDrag = () => {
    if (!dragging.current) return;
    dragging.current = false;
    onDragging?.(false);
  };

  return (
    <div className="poslider-wrap">
      {onIconClick ? (
        <button type="button" className="poslider__iconbtn" onClick={onIconClick} title={label}>
          <Icon name={icon} size={18} />
        </button>
      ) : (
        <span className="poslider__icon" title={label}>
          <Icon name={icon} size={18} />
        </span>
      )}
      <div
        ref={ref}
        className={`poslider ${disabled ? "hslider--disabled" : ""}`}
        onPointerDown={(e) => {
          if (disabled) return;
          e.currentTarget.setPointerCapture(e.pointerId);
          dragging.current = true;
          onDragging?.(true);
          onChange(calc(e.clientX));
        }}
        onPointerMove={(e) => {
          if (dragging.current) onChange(calc(e.clientX));
        }}
        onPointerUp={(e) => {
          if (!dragging.current) return;
          onChange(calc(e.clientX));
          endDrag();
        }}
        onPointerCancel={endDrag}
        onLostPointerCapture={endDrag}
      >
        <div className="poslider__fill" style={{ width: `${value}%` }} />
      </div>
      <span className="poslider__val">{disabled ? "—" : `${Math.round(value)}%`}</span>
    </div>
  );
}

export function PillOverlay({
  active,
  mode,
  manual,
  openedAtSeq,
  onBackToBar,
  onClose,
  onWidthChange,
}: {
  active: boolean;
  mode: PillOverlayMode;
  manual: boolean;
  openedAtSeq: number;
  onBackToBar?: () => void;
  onClose: () => void;
  onWidthChange: (w: number) => void;
}) {
  const [vol, setVol] = useState<Vol>({ volume: 0, muted: false });
  const [bright, setBright] = useState<number | null>(null);
  const [armed, setArmed] = useState<string | null>(null);
  const [showPower, setShowPower] = useState(mode === "power");
  const [pinnedManual, setPinnedManual] = useState(manual);

  const autoTimer = useRef<number | undefined>(undefined);
  const draggingVol = useRef(false);
  const draggingBright = useRef(false);
  const lastVol = useRef(-1);
  const lastBright = useRef(-1);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  const armAutoClose = useCallback(() => {
    window.clearTimeout(autoTimer.current);
    autoTimer.current = window.setTimeout(() => onCloseRef.current(), 1800);
  }, []);

  useEffect(() => {
    if (!active) {
      window.clearTimeout(autoTimer.current);
      setArmed(null);
      return;
    }
    setPinnedManual(manual);
    setShowPower(mode === "power");
    setArmed(null);
    if (!manual) armAutoClose();
    else window.clearTimeout(autoTimer.current);
  }, [active, mode, manual, openedAtSeq, armAutoClose]);

  useEffect(() => {
    const u1 = listen<Vol>("volume-changed", (e) => {
      if (!draggingVol.current) setVol(e.payload);
      if (!pinnedManual) armAutoClose();
    });
    const u2 = listen<number>("brightness-changed", (e) => {
      if (!draggingBright.current) setBright(e.payload);
      if (!pinnedManual) armAutoClose();
    });
    return () => {
      u1.then((fn) => fn());
      u2.then((fn) => fn());
      window.clearTimeout(autoTimer.current);
    };
  }, [pinnedManual, armAutoClose]);

  useEffect(() => {
    if (!active) return;
    if (mode !== "power") {
      invoke<Vol | null>("get_volume")
        .then((v) => v && setVol(v))
        .catch(() => {});
      invoke<number | null>("get_brightness")
        .then(setBright)
        .catch(() => setBright(null));
    }
  }, [active, mode]);

  useEffect(() => {
    if (!armed) return;
    const id = window.setTimeout(() => setArmed(null), 3000);
    return () => window.clearTimeout(id);
  }, [armed]);

  useEffect(() => {
    if (!active) return;
    const w = mode === "power" ? 360 : showPower ? 540 : mode === "mixer" ? 440 : 340;
    onWidthChange(w);
  }, [active, mode, showPower, onWidthChange]);

  const showVol = mode === "mixer" || mode === "volume";
  const showBright = mode === "mixer" || mode === "brightness";
  const volIcon =
    vol.muted || vol.volume < 1 ? "volume_off" : vol.volume < 50 ? "volume_down" : "volume_up";

  const onVolume = (pct: number) => {
    setVol({ volume: pct, muted: false });
    const r = Math.round(pct);
    if (r !== lastVol.current) {
      lastVol.current = r;
      invoke("set_volume", { percent: r }).catch(() => {});
    }
  };

  const onBrightness = (pct: number) => {
    setBright(pct);
    const r = Math.round(pct);
    if (r !== lastBright.current) {
      lastBright.current = r;
      invoke("set_brightness", { percent: r }).catch(() => {});
    }
  };

  const onPower = (it: (typeof POWER_ITEMS)[number]) => {
    if (it.dangerous && armed !== it.id) {
      setArmed(it.id);
      return;
    }
    onClose();
    invoke("run_action", { id: it.id }).catch(() => {});
  };

  const promoteManual = () => {
    window.clearTimeout(autoTimer.current);
    setPinnedManual(true);
  };

  return (
    <div
      className="poverlay"
      onMouseEnter={() => {
        if (!pinnedManual) window.clearTimeout(autoTimer.current);
      }}
      onMouseLeave={() => {
        if (!pinnedManual) armAutoClose();
      }}
      onClick={(e) => {
        if (mode !== "power" && !(e.target as HTMLElement).closest("button, .poslider")) {
          promoteManual();
          setShowPower((p) => !p);
        }
      }}
    >
      <div className="poverlay__top">
        {onBackToBar && (
          <button
            type="button"
            className="pbar__btn"
            title="Back to Bar"
            aria-label="Back to Bar"
            onClick={onBackToBar}
          >
            <Icon name="arrow_back" size={18} />
          </button>
        )}
        <div className="poverlay__sliders">
          {showVol && (
            <HorizontalSlider
              value={vol.muted ? 0 : vol.volume}
              onChange={onVolume}
              onDragging={(d) => {
                draggingVol.current = d;
                if (d) promoteManual();
              }}
              icon={volIcon}
              onIconClick={() => {
                promoteManual();
                invoke("toggle_mute").catch(() => {});
              }}
              label="Volume"
            />
          )}
          {showBright && (
            <HorizontalSlider
              value={bright ?? 0}
              disabled={bright === null}
              onChange={onBrightness}
              onDragging={(d) => {
                draggingBright.current = d;
                if (d) promoteManual();
              }}
              icon="brightness_medium"
              label="Brightness"
            />
          )}
          {mode === "power" && <span className="poverlay__title">Power options</span>}
        </div>
        {mode !== "power" && (
          <button
            type="button"
            className={`pbar__btn ${showPower ? "pbar__btn--active" : ""}`}
            title="Power options"
            aria-label="Power options"
            onClick={() => {
              promoteManual();
              setShowPower((p) => !p);
            }}
          >
            <Icon name="power_settings_new" size={18} />
          </button>
        )}
      </div>

      {showPower && (
        <div className="poverlay__power">
          {POWER_ITEMS.map((it) => (
            <button
              key={it.id}
              type="button"
              className={`poverlay__pbtn ${armed === it.id ? "poverlay__pbtn--armed" : ""}`}
              title={it.label}
              onClick={() => onPower(it)}
            >
              <Icon name={it.icon} size={20} />
              <span>{armed === it.id ? "Confirm?" : it.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
