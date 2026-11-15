import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Icon from "./Icon";

type Vol = { volume: number; muted: boolean };
type AudioOutput = { id: string; name: string; active: boolean };

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
  const [outputsOpen, setOutputsOpen] = useState(false);
  const [outputs, setOutputs] = useState<AudioOutput[]>([]);
  const lastVol = useRef(-1);
  const lastBright = useRef(-1);
  const dragging = useRef(false);

  useEffect(() => {
    if (!active) {
      setOutputsOpen(false);
      return;
    }
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

  const toggleOutputs = () => {
    if (outputsOpen) {
      setOutputsOpen(false);
      return;
    }
    setOutputsOpen(true);
    invoke<AudioOutput[]>("list_audio_outputs")
      .then((list) => setOutputs(list))
      .catch(() => setOutputs([]));
  };

  const selectOutput = (id: string) => {
    setOutputs((prev) => prev.map((d) => ({ ...d, active: d.id === id })));
    invoke("set_audio_output", { id })
      .then(() => invoke<Vol | null>("get_volume").then((v) => v && setVol(v)))
      .catch(() => {});
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
        {!vertical && (
          <button
            className={`vb__pick ${outputsOpen ? "vb__pick--open" : ""}`}
            title="Pilih perangkat output audio"
            onClick={toggleOutputs}
          >
            <Icon name="expand_more" size={20} />
          </button>
        )}
      </div>
      {!vertical && outputsOpen && (
        <div className="vb__devices">
          {outputs.length === 0 ? (
            <div className="vb__na">Tidak ada perangkat output ditemukan</div>
          ) : (
            outputs.map((d) => (
              <button
                key={d.id}
                className={`vb__dev ${d.active ? "vb__dev--active" : ""}`}
                onClick={() => selectOutput(d.id)}
              >
                <Icon
                  name={/headphone|headset|earphone|earbud|airpods|buds/i.test(d.name) ? "headphones" : "speaker"}
                  size={18}
                />
                <span className="vb__devname">{d.name}</span>
                {d.active && <Icon name="check" size={18} />}
              </button>
            ))
          )}
        </div>
      )}
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
