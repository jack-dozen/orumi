import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Icon from "./Icon";

type Vol = { volume: number; muted: boolean };

/** Volume and brightness sliders (notch dashboard and quick settings). */
export default function VolumeBrightness({
  active,
  vertical = false,
}: {
  active: boolean;
  /** Two vertical sliders side by side (narrow dashboard column). */
  vertical?: boolean;
}) {
  const [vol, setVol] = useState<Vol | null>(null);
  const [bright, setBright] = useState<number | null>(null);
  const lastVol = useRef(-1);
  const lastBright = useRef(-1);
  const dragging = useRef(false);

  useEffect(() => {
    if (!active) return;
    invoke<Vol | null>("get_volume")
      .then((v) => v && setVol(v))
      .catch(() => {});
    invoke<number | null>("get_brightness")
      .then((b) => setBright(b))
      .catch(() => {});
    const a = listen<Vol>("volume-changed", (e) => {
      if (!dragging.current) setVol(e.payload);
    });
    const b = listen<number>("brightness-changed", (e) => {
      if (!dragging.current) setBright(e.payload);
    });
    return () => {
      a.then((fn) => fn());
      b.then((fn) => fn());
    };
  }, [active]);

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

  const volIcon = !vol || vol.muted || vol.volume === 0 ? "volume_off" : vol.volume < 40 ? "volume_down" : "volume_up";
  const slider = (value: number, onChange: (v: number) => void) => (
    <input
      className="vbslider"
      type="range"
      min={0}
      max={100}
      value={Math.round(value)}
      style={{ ["--fill" as string]: `${value}%` }}
      onPointerDown={() => (dragging.current = true)}
      onPointerUp={() => (dragging.current = false)}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );

  return (
    <div className={`card vb ${vertical ? "vb--vertical" : ""}`}>
      <div className="vb__row">
        <button
          className="vb__icon"
          title="Mute"
          onClick={() => invoke("toggle_mute").catch(() => {})}
        >
          <Icon name={volIcon} size={22} />
        </button>
        {slider(vol?.volume ?? 0, onVolume)}
        <span className="vb__val">{vol ? Math.round(vol.volume) : "—"}</span>
      </div>
      <div className="vb__row">
        <span className="vb__icon vb__icon--static">
          <Icon name="brightness_medium" size={22} />
        </span>
        {bright === null ? (
          <span className="vb__na">Brightness tidak tersedia (bukan layar laptop)</span>
        ) : (
          slider(bright, onBrightness)
        )}
        <span className="vb__val">{bright === null ? "" : Math.round(bright)}</span>
      </div>
    </div>
  );
}
