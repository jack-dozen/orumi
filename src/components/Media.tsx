import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MediaState } from "../useMedia";
import Icon from "./Icon";

type Vol = { volume: number; muted: boolean };
type AudioOutput = { id: string; name: string; active: boolean };

const volIconFor = (v: Vol | null) =>
  !v || v.muted || v.volume === 0 ? "volume_off" : v.volume < 40 ? "volume_down" : "volume_up";

const VIZ_BARS = [
  { dur: 520, delay: -80 },
  { dur: 640, delay: -240 },
  { dur: 460, delay: -120 },
  { dur: 700, delay: -410 },
  { dur: 490, delay: -290 },
  { dur: 610, delay: -60 },
  { dur: 430, delay: -350 },
  { dur: 680, delay: -180 },
  { dur: 550, delay: -440 },
  { dur: 470, delay: -150 },
  { dur: 660, delay: -310 },
  { dur: 510, delay: -90 },
  { dur: 630, delay: -390 },
  { dur: 450, delay: -210 },
  { dur: 590, delay: -470 },
  { dur: 530, delay: -140 },
];

const BG_VIZ_BARS = [
  { dur: 760, delay: -110, h: 52 },
  { dur: 610, delay: -340, h: 74 },
  { dur: 840, delay: -220, h: 63 },
  { dur: 560, delay: -480, h: 88 },
  { dur: 710, delay: -90, h: 68 },
  { dur: 650, delay: -390, h: 92 },
  { dur: 790, delay: -170, h: 58 },
  { dur: 590, delay: -430, h: 82 },
  { dur: 730, delay: -260, h: 94 },
  { dur: 620, delay: -60, h: 70 },
  { dur: 820, delay: -310, h: 86 },
  { dur: 570, delay: -450, h: 64 },
  { dur: 690, delay: -140, h: 90 },
  { dur: 770, delay: -370, h: 76 },
  { dur: 600, delay: -200, h: 56 },
  { dur: 850, delay: -490, h: 84 },
  { dur: 640, delay: -120, h: 72 },
  { dur: 740, delay: -280, h: 92 },
  { dur: 580, delay: -410, h: 66 },
  { dur: 700, delay: -70, h: 80 },
  { dur: 800, delay: -330, h: 60 },
  { dur: 630, delay: -190, h: 86 },
  { dur: 720, delay: -440, h: 70 },
  { dur: 670, delay: -150, h: 50 },
];

const fmt = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

function MarqueeTitle({ text, className }: { text: string; className: string }) {
  const boxRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const [marquee, setMarquee] = useState(false);

  useLayoutEffect(() => {
    const box = boxRef.current;
    const textEl = textRef.current;
    if (!box || !textEl) {
      setMarquee(false);
      return;
    }
    const measure = () => setMarquee(textEl.offsetWidth > box.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(box);
    return () => ro.disconnect();
  }, [text]);

  return (
    <div ref={boxRef} className={`${className} ${marquee ? `${className}--marquee` : ""}`}>
      <div className="marquee__track">
        <span ref={textRef} className="marquee__item">
          {text}
        </span>
        {marquee && (
          <span className="marquee__item" aria-hidden="true">
            {text}
          </span>
        )}
      </div>
    </div>
  );
}

function Progress({ media }: { media: MediaState }) {
  const { info, seek } = media;
  if (!info) return null;
  const pct =
    info.duration_ms > 0 ? Math.min(100, (info.position_ms / info.duration_ms) * 100) : 0;

  return (
    <div
      className="progress"
      onClick={(e) => {
        if (info.duration_ms <= 0) return;
        const r = e.currentTarget.getBoundingClientRect();
        seek(((e.clientX - r.left) / r.width) * info.duration_ms);
      }}
    >
      <div className="progress__fill" style={{ width: `${pct}%` }} />
    </div>
  );
}

function Controls({
  media,
  extended = false,
  audioOpen = false,
  volIcon = "volume_up",
  onToggleAudio,
}: {
  media: MediaState;
  extended?: boolean;
  audioOpen?: boolean;
  volIcon?: string;
  onToggleAudio?: () => void;
}) {
  const { info, control } = media;
  const repeat = info?.repeat ?? "off";
  return (
    <div className={`mcontrols ${extended ? "mcontrols--mtab" : ""}`}>
      {extended && (
        <button
          type="button"
          className={`mcontrols__opt ${info?.shuffle ? "mcontrols__opt--on" : ""}`}
          onClick={() => control("shuffle")}
          title={info?.shuffle ? "Shuffle on" : "Shuffle off"}
        >
          <Icon name="shuffle" size={20} />
        </button>
      )}
      <button type="button" onClick={() => control("prev")} title="Previous">
        <Icon name="skip_previous" size={26} fill />
      </button>
      <button
        type="button"
        className="mcontrols__play"
        onClick={() => control("toggle")}
        title="Play / pause"
      >
        <Icon name={info?.playing ? "pause" : "play_arrow"} size={30} fill />
      </button>
      <button type="button" onClick={() => control("next")} title="Next">
        <Icon name="skip_next" size={26} fill />
      </button>
      {extended && (
        <>
          <button
            type="button"
            className={`mcontrols__opt ${repeat !== "off" ? "mcontrols__opt--on" : ""}`}
            onClick={() => control("repeat")}
            title={
              repeat === "track" ? "Repeat one" : repeat === "list" ? "Repeat all" : "Repeat off"
            }
          >
            <Icon name={repeat === "track" ? "repeat_one" : "repeat"} size={20} />
          </button>
          <button
            type="button"
            className={`mcontrols__opt mcontrols__speaker ${audioOpen ? "mcontrols__opt--on" : ""}`}
            onClick={onToggleAudio}
            title="Volume & audio output"
          >
            <Icon name={volIcon} size={20} />
            <Icon
              name="expand_more"
              size={16}
              className={`mcontrols__chev ${audioOpen ? "mcontrols__chev--open" : ""}`}
            />
          </button>
        </>
      )}
    </div>
  );
}

function Cover({ art, className }: { art: string | null; className: string }) {
  return art ? (
    <img className={className} src={art} alt="" />
  ) : (
    <div className={`${className} cover--empty`}>
      <Icon name="music_note" size={40} />
    </div>
  );
}

/** Small card for the dashboard. */
export function MiniPlayer({
  media,
  active = true,
}: {
  media: MediaState;
  active?: boolean;
}) {
  const { info, art } = media;
  const [volOpen, setVolOpen] = useState(false);
  const [vol, setVol] = useState<Vol | null>(null);
  const osdRef = useRef<HTMLDivElement>(null);
  const autoTimer = useRef<number | undefined>(undefined);
  const lastVol = useRef(-1);
  const dragging = useRef(false);

  const armAutoClose = useCallback(() => {
    window.clearTimeout(autoTimer.current);
    autoTimer.current = window.setTimeout(() => setVolOpen(false), 2200);
  }, []);

  useEffect(() => {
    if (!active) {
      window.clearTimeout(autoTimer.current);
      setVolOpen(false);
    }
  }, [active]);

  useEffect(() => {
    if (!active || !volOpen) return;
    let alive = true;
    armAutoClose();
    invoke<Vol | null>("get_volume")
      .then((v) => {
        if (alive && v) setVol(v);
      })
      .catch(() => {});
    const unlisten = listen<Vol>("volume-changed", (e) => {
      if (!dragging.current) setVol(e.payload);
    });
    return () => {
      alive = false;
      dragging.current = false;
      window.clearTimeout(autoTimer.current);
      unlisten.then((fn) => fn());
    };
  }, [active, volOpen, armAutoClose]);

  if (!info) return null;

  const setVolume = (pct: number) => {
    const clamped = Math.max(0, Math.min(100, pct));
    setVol({ volume: clamped, muted: false });
    const r = Math.round(clamped);
    if (r !== lastVol.current) {
      lastVol.current = r;
      invoke("set_volume", { percent: r }).catch(() => {});
    }
  };

  const calcVol = (clientX: number) => {
    const r = osdRef.current!.getBoundingClientRect();
    return Math.max(0, Math.min(100, ((clientX - r.left) / r.width) * 100));
  };

  const endDrag = () => {
    if (!dragging.current) return;
    dragging.current = false;
    armAutoClose();
  };

  const volIcon = volIconFor(vol);
  const volPct = vol?.muted ? 0 : (vol?.volume ?? 0);

  return (
    <div className="card mini">
      <Cover art={art} className="mini__cover" />
      <div className="mini__meta">
        <MarqueeTitle text={info.title || "—"} className="mini__title" />
        <div className="mini__artist">{info.artist || info.source || "—"}</div>
      </div>
      <div className="mini__mid">
        {volOpen ? (
          <div
            ref={osdRef}
            className="mini__osd"
            title="Drag to adjust volume · Click icon to mute · Right-click to close"
            onPointerDown={(e) => {
              window.clearTimeout(autoTimer.current);
              const r = osdRef.current!.getBoundingClientRect();
              if (e.clientX < r.left + r.height) {
                invoke("toggle_mute").catch(() => {});
                armAutoClose();
                return;
              }
              e.currentTarget.setPointerCapture(e.pointerId);
              dragging.current = true;
              setVolume(calcVol(e.clientX));
            }}
            onPointerMove={(e) => {
              if (dragging.current) setVolume(calcVol(e.clientX));
            }}
            onPointerUp={(e) => {
              if (!dragging.current) return;
              setVolume(calcVol(e.clientX));
              endDrag();
            }}
            onPointerCancel={endDrag}
            onLostPointerCapture={endDrag}
            onWheel={(e) => {
              if (!vol) return;
              setVolume(vol.volume + (e.deltaY < 0 ? 5 : -5));
              armAutoClose();
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              setVolOpen(false);
            }}
          >
            <span className="mini__osd-icon">
              <Icon name={volIcon} size={20} />
            </span>
            <div className="mini__osd-fill" style={{ width: `${volPct}%` }}>
              <span className="mini__osd-icon mini__osd-icon--on">
                <Icon name={volIcon} size={20} />
              </span>
            </div>
          </div>
        ) : (
          <button
            type="button"
            className={`mini__viz ${info.playing ? "mini__viz--playing" : ""}`}
            onClick={() => setVolOpen(true)}
            title="Adjust volume"
          >
            {VIZ_BARS.map((b, i) => (
              <span
                key={i}
                className="mini__bar"
                style={{
                  animationDuration: `${b.dur}ms`,
                  animationDelay: `${b.delay}ms`,
                }}
              />
            ))}
          </button>
        )}
      </div>
      <Progress media={media} />
      <Controls media={media} />
    </div>
  );
}

/** Full media tab. */
export function MediaTab({
  media,
  active = true,
}: {
  media: MediaState;
  active?: boolean;
}) {
  const { info, art } = media;
  const [audioOpen, setAudioOpen] = useState(false);
  const [vol, setVol] = useState<Vol | null>(null);
  const [outputs, setOutputs] = useState<AudioOutput[]>([]);
  const dragging = useRef(false);
  const lastVol = useRef(-1);

  useEffect(() => {
    if (!active) {
      dragging.current = false;
      return;
    }
    let alive = true;
    invoke<Vol | null>("get_volume")
      .then((v) => {
        if (alive && v) setVol(v);
      })
      .catch(() => {});
    const unlisten = listen<Vol>("volume-changed", (e) => {
      if (!dragging.current) setVol(e.payload);
    });
    return () => {
      alive = false;
      unlisten.then((fn) => fn());
    };
  }, [active]);

  useEffect(() => {
    if (!active || !audioOpen) return;
    let alive = true;
    invoke<AudioOutput[]>("list_audio_outputs")
      .then((list) => {
        if (alive) setOutputs(list);
      })
      .catch(() => {
        if (alive) setOutputs([]);
      });
    return () => {
      alive = false;
    };
  }, [active, audioOpen]);

  if (!info) {
    return <div className="card card--placeholder card--fill">No media playing</div>;
  }

  const onVolume = (v: number) => {
    setVol({ volume: v, muted: false });
    const r = Math.round(v);
    if (r !== lastVol.current) {
      lastVol.current = r;
      invoke("set_volume", { percent: r }).catch(() => {});
    }
  };

  const selectOutput = (id: string) => {
    setOutputs((prev) => prev.map((d) => ({ ...d, active: d.id === id })));
    invoke("set_audio_output", { id })
      .then(() => invoke<Vol | null>("get_volume").then((v) => v && setVol(v)))
      .catch(() => {});
  };

  const volVal = vol?.volume ?? 0;
  const volIcon = volIconFor(vol);

  return (
    <div className="card mtab">
      <div
        className={`mtab__bgviz ${info.playing ? "mtab__bgviz--playing" : ""}`}
        aria-hidden="true"
      >
        {BG_VIZ_BARS.map((b, i) => (
          <span
            key={i}
            className="mtab__bgbar"
            style={{
              height: `${b.h}%`,
              animationDuration: `${b.dur}ms`,
              animationDelay: `${b.delay}ms`,
            }}
          />
        ))}
      </div>
      <div className="mtab__main">
        <Cover art={art} className="mtab__cover" />
        <div className="mtab__info">
          <div className="mtab__source">{info.source}</div>
          <MarqueeTitle text={info.title || "—"} className="mtab__title" />
          <div className="mtab__artist">
            {[info.artist, info.album].filter(Boolean).join(" · ")}
          </div>
          <Progress media={media} />
          <div className="mtab__times">
            <span>{fmt(info.position_ms)}</span>
            <span>{info.duration_ms > 0 ? fmt(info.duration_ms) : "--:--"}</span>
          </div>
          <Controls
            media={media}
            extended
            audioOpen={audioOpen}
            volIcon={volIcon}
            onToggleAudio={() => setAudioOpen((v) => !v)}
          />
        </div>
      </div>
      {audioOpen && (
        <div className="mtab__audio">
          <div className="vb__row">
            <button
              type="button"
              className="vb__icon"
              title="Mute"
              onClick={() => invoke("toggle_mute").catch(() => {})}
            >
              <Icon name={volIcon} size={22} />
            </button>
            <input
              className="vbslider"
              type="range"
              min={0}
              max={100}
              value={Math.round(volVal)}
              style={{ ["--fill" as string]: `${volVal}%` }}
              onPointerDown={() => (dragging.current = true)}
              onPointerUp={() => (dragging.current = false)}
              onChange={(e) => onVolume(Number(e.target.value))}
            />
            <span className="vb__val">{vol ? Math.round(volVal) : "—"}</span>
          </div>
          <div className="vb__devices">
            {outputs.length === 0 ? (
              <div className="vb__na">No output devices found</div>
            ) : (
              outputs.map((d) => (
                <button
                  key={d.id}
                  type="button"
                  className={`vb__dev ${d.active ? "vb__dev--active" : ""}`}
                  onClick={() => selectOutput(d.id)}
                >
                  <Icon
                    name={
                      /headphone|headset|earphone|earbud|airpods|buds/i.test(d.name)
                        ? "headphones"
                        : "speaker"
                    }
                    size={18}
                  />
                  <span className="vb__devname">{d.name}</span>
                  {d.active && <Icon name="check" size={18} />}
                </button>
              ))
            )}
          </div>
        </div>
      )}
    </div>
  );
}
