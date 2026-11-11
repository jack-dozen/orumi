import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import Icon from "./Icon";

type Mode = "power" | "mixer" | "volume" | "brightness";
type Vol = { volume: number; muted: boolean };

const closeOverlay = () => invoke("overlay_close");

const POWER_ITEMS = [
  { id: "lock", label: "Lock", icon: "lock" },
  { id: "sleep", label: "Sleep", icon: "bedtime" },
  { id: "hibernate", label: "Hibernate", icon: "schedule" },
  { id: "shutdown", label: "Shutdown", icon: "power_settings_new", dangerous: true },
  { id: "restart", label: "Restart", icon: "restart_alt", dangerous: true },
];

/** Vertical slider: drag or click to set a value from 0 to 100. */
function VSlider(props: {
  value: number;
  disabled?: boolean;
  onChange: (v: number) => void;
  onDragging?: (d: boolean) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const dragging = useRef(false);

  const calc = (clientY: number) => {
    const r = ref.current!.getBoundingClientRect();
    return Math.max(0, Math.min(100, (1 - (clientY - r.top) / r.height) * 100));
  };

  const endDrag = () => {
    if (!dragging.current) return;
    dragging.current = false;
    props.onDragging?.(false);
  };

  return (
    <div
      ref={ref}
      className={`vslider ${props.disabled ? "vslider--disabled" : ""}`}
      onPointerDown={(e) => {
        if (props.disabled) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        dragging.current = true;
        props.onDragging?.(true);
        props.onChange(calc(e.clientY));
      }}
      onPointerMove={(e) => {
        if (dragging.current) props.onChange(calc(e.clientY));
      }}
      onPointerUp={(e) => {
        if (!dragging.current) return;
        props.onChange(calc(e.clientY));
        endDrag();
      }}
      onPointerCancel={endDrag}
      onLostPointerCapture={endDrag}
    >
      <div className="vslider__fill" style={{ height: `${props.value}%` }} />
    </div>
  );
}

export default function Overlay() {
  const visible = useEdgeVisible("overlay");
  const [mode, setMode] = useState<Mode>("power");
  const [vol, setVol] = useState<Vol>({ volume: 0, muted: false });
  const [bright, setBright] = useState<number | null>(null);
  const [armed, setArmed] = useState<string | null>(null);

  const manual = useRef(true);
  const openedAt = useRef(0);
  const autoTimer = useRef<number | undefined>(undefined);
  const draggingVol = useRef(false);
  const draggingBright = useRef(false);
  const lastVol = useRef(-1);
  const lastBright = useRef(-1);

  const armAutoClose = useCallback(() => {
    window.clearTimeout(autoTimer.current);
    autoTimer.current = window.setTimeout(closeOverlay, 1800);
  }, []);

  // Command from Rust: content to display, opened manually or as an automatic popup.
  useEffect(() => {
    const unlisten = listen<{ mode: Mode; manual: boolean }>("overlay-mode", (e) => {
      window.clearTimeout(autoTimer.current);
      manual.current = e.payload.manual;
      openedAt.current = Date.now();
      setMode(e.payload.mode);
      setArmed(null);
      if (!e.payload.manual) armAutoClose();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [armAutoClose]);

  // Volume changed (keyboard key, etc.): update the slider and extend the popup.
  useEffect(() => {
    const unlisten = listen<Vol>("volume-changed", (e) => {
      if (!draggingVol.current) setVol(e.payload);
      if (!manual.current) armAutoClose();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [armAutoClose]);

  // Brightness changed (Fn key, etc.).
  useEffect(() => {
    const unlisten = listen<number>("brightness-changed", (e) => {
      if (!draggingBright.current) setBright(e.payload);
      if (!manual.current) armAutoClose();
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [armAutoClose]);

  // When shown, fetch the current value for the displayed content.
  useEffect(() => {
    if (!visible) return;
    if (mode === "mixer" || mode === "volume") {
      invoke<Vol | null>("get_volume")
        .then((v) => v && setVol(v))
        .catch(() => {});
    }
    if (mode === "mixer" || mode === "brightness") {
      invoke<number | null>("get_brightness")
        .then(setBright)
        .catch(() => setBright(null));
    }
  }, [visible, mode]);

  // A manual overlay closes when it loses focus (outside click).
  useEffect(() => {
    const onBlur = () => {
      if (manual.current && Date.now() - openedAt.current > 400) closeOverlay();
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Clicking the transparent area outside the overlay pill counts as an outside click: close.
  useEffect(() => {
    const onDown = (e: PointerEvent) => {
      if (!manual.current) return;
      if (!(e.target as HTMLElement).closest(".overlay")) closeOverlay();
    };
    document.addEventListener("pointerdown", onDown);
    return () => document.removeEventListener("pointerdown", onDown);
  }, []);

  // Shutdown/restart confirmation cancels itself after 3 seconds or when the overlay closes.
  useEffect(() => {
    if (!armed) return;
    const id = setTimeout(() => setArmed(null), 3000);
    return () => clearTimeout(id);
  }, [armed]);
  useEffect(() => {
    if (!visible) setArmed(null);
  }, [visible]);

  // If the overlay closes during a drag, clear the "being dragged" state.
  useEffect(() => {
    if (!visible) {
      draggingVol.current = false;
      draggingBright.current = false;
    }
  }, [visible]);

  const onPower = (item: (typeof POWER_ITEMS)[number]) => {
    if (item.dangerous && armed !== item.id) {
      setArmed(item.id);
      return;
    }
    closeOverlay();
    invoke("run_action", { id: item.id }).catch(() => {});
  };

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

  const showVol = mode === "mixer" || mode === "volume";
  const showBright = mode === "mixer" || mode === "brightness";
  const volIcon =
    vol.muted || vol.volume < 1 ? "volume_off" : vol.volume < 50 ? "volume_down" : "volume_up";

  return (
    <div className={`overlay ${visible ? "overlay--visible" : ""}`}>
      {mode === "power" ? (
        <>
          <div className="opill opill--power">
            {POWER_ITEMS.map((it) => (
              <button
                key={it.id}
                title={it.label}
                className={`obtn ${armed === it.id ? "obtn--armed" : ""}`}
                onClick={() => onPower(it)}
              >
                <Icon name={it.icon} size={26} />
              </button>
            ))}
          </div>
          {armed && <div className="ocaption">klik lagi untuk konfirmasi</div>}
        </>
      ) : (
        <div className={`opill ${mode === "mixer" ? "opill--mixer" : "opill--single"}`}>
          {showVol && (
            <div className="mixer__col">
              <VSlider
                value={vol.muted ? 0 : vol.volume}
                onChange={onVolume}
                onDragging={(d) => (draggingVol.current = d)}
              />
              <button
                className="mixer__icon"
                title="Mute / unmute"
                onClick={() => invoke("toggle_mute").catch(() => {})}
              >
                <Icon name={volIcon} size={22} />
              </button>
              <span className="mixer__num">{vol.muted ? "mute" : Math.round(vol.volume)}</span>
            </div>
          )}
          {showBright && (
            <div className="mixer__col">
              <VSlider
                value={bright ?? 0}
                disabled={bright === null}
                onChange={onBrightness}
                onDragging={(d) => (draggingBright.current = d)}
              />
              <span className="mixer__icon">
                <Icon name="brightness_medium" size={22} />
              </span>
              <span className="mixer__num">{bright === null ? "—" : Math.round(bright)}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
