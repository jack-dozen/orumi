import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MediaState } from "../useMedia";
import { useLyrics, type LyricsState } from "../useLyrics";
import { useVisualizerBars } from "../useVisualizer";
import Icon from "./Icon";

type Vol = { volume: number; muted: boolean };
type AudioOutput = { id: string; name: string; active: boolean };

const volIconFor = (v: Vol | null) =>
  !v || v.muted || v.volume === 0 ? "volume_off" : v.volume < 40 ? "volume_down" : "volume_up";

const MINI_BAR_COUNT = 16;
const BG_BAR_COUNT = 24;

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

function LyricsIcon() {
  return (
    <svg
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M4 6h11" />
      <path d="M4 10h11" />
      <path d="M4 14h7" />
      <path d="M18 17V9l3-1" />
      <circle cx="16" cy="17" r="2" />
    </svg>
  );
}

function Controls({
  media,
  extended = false,
  audioOpen = false,
  lyricsOpen = false,
  volIcon = "volume_up",
  onToggleAudio,
  onToggleLyrics,
}: {
  media: MediaState;
  extended?: boolean;
  audioOpen?: boolean;
  lyricsOpen?: boolean;
  volIcon?: string;
  onToggleAudio?: () => void;
  onToggleLyrics?: () => void;
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
            className={`mcontrols__opt ${lyricsOpen ? "mcontrols__opt--on" : ""}`}
            onClick={onToggleLyrics}
            title="Live lyrics"
          >
            <LyricsIcon />
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
  const { barRefs: miniBarRefs } = useVisualizerBars(
    active && !volOpen && Boolean(info),
    MINI_BAR_COUNT,
    { minScale: 0.18, beatLift: 0.08 },
  );

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
            {Array.from({ length: MINI_BAR_COUNT }, (_, i) => (
              <span
                key={i}
                ref={(el) => {
                  miniBarRefs.current[i] = el;
                }}
                className="mini__bar"
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

function LyricsPanel({
  lyrics,
  onSeek,
}: {
  lyrics: LyricsState;
  onSeek: (ms: number) => void;
}) {
  const boxRef = useRef<HTMLDivElement>(null);
  const lineRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const manualUntilRef = useRef(0);

  useEffect(() => {
    manualUntilRef.current = 0;
    lineRefs.current = [];
    if (boxRef.current) boxRef.current.scrollTop = 0;
  }, [lyrics.data]);

  useEffect(() => {
    if (lyrics.activeIndex < 0) return;
    if (performance.now() < manualUntilRef.current) return;
    const box = boxRef.current;
    const el = lineRefs.current[lyrics.activeIndex];
    if (!box || !el) return;
    const target = el.offsetTop - box.clientHeight / 2 + el.clientHeight / 2;
    box.scrollTo({ top: Math.max(0, target), behavior: "smooth" });
  }, [lyrics.activeIndex, lyrics.data]);

  if (lyrics.status === "loading" || lyrics.status === "idle") {
    return (
      <div className="mtab__lyrics mtab__lyrics--msg">
        <span>Loading lyrics…</span>
      </div>
    );
  }

  if (lyrics.status === "error") {
    return (
      <div className="mtab__lyrics mtab__lyrics--msg">
        <span>Couldn't load lyrics</span>
        <button type="button" className="mtab__lyrics-retry" onClick={lyrics.retry}>
          Retry
        </button>
      </div>
    );
  }

  if (!lyrics.data || lyrics.data.kind === "none") {
    return (
      <div className="mtab__lyrics mtab__lyrics--msg">
        <span>No lyrics found</span>
      </div>
    );
  }

  if (lyrics.data.kind === "instrumental") {
    return (
      <div className="mtab__lyrics mtab__lyrics--msg">
        <span>♪ Instrumental ♪</span>
      </div>
    );
  }

  if (lyrics.data.kind === "plain") {
    return (
      <div ref={boxRef} className="mtab__lyrics mtab__lyrics--scroll">
        <div className="mtab__lyrics-list mtab__lyrics-list--plain">
          {lyrics.data.lines.map((line, i) => (
            <div key={i} className={`mtab__lyric ${line ? "mtab__lyric--plain" : "mtab__lyric--gap"}`}>
              {line}
            </div>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div
      ref={boxRef}
      className="mtab__lyrics mtab__lyrics--scroll"
      onWheel={() => {
        manualUntilRef.current = performance.now() + 3000;
      }}
      onPointerDown={() => {
        manualUntilRef.current = performance.now() + 3000;
      }}
    >
      <div className="mtab__lyrics-list">
        {lyrics.data.lines.map((line, i) => {
          const isActive = i === lyrics.activeIndex;
          const isPast = lyrics.activeIndex >= 0 && i < lyrics.activeIndex;
          return (
            <button
              key={`${line.timeMs}-${i}`}
              ref={(el) => {
                lineRefs.current[i] = el;
              }}
              type="button"
              className={`mtab__lyric mtab__lyric--sync ${
                isActive ? "mtab__lyric--active" : isPast ? "mtab__lyric--past" : ""
              }`}
              onClick={() => {
                manualUntilRef.current = 0;
                onSeek(line.timeMs);
              }}
              title="Jump to line"
            >
              {line.text}
            </button>
          );
        })}
      </div>
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
  const [lyricsOpen, setLyricsOpen] = useState(false);
  const [vol, setVol] = useState<Vol | null>(null);
  const [outputs, setOutputs] = useState<AudioOutput[]>([]);
  const dragging = useRef(false);
  const lastVol = useRef(-1);
  const lyrics = useLyrics(info, active && lyricsOpen);
  const { barRefs: bgBarRefs, containerRef: bgVizRef } = useVisualizerBars(
    active && Boolean(info),
    BG_BAR_COUNT,
    {
      minScale: 0.1,
      baseOpacity: info?.playing ? 0.36 : 0.24,
      beatOpacityBoost: info?.playing ? 0.3 : 0.16,
      beatLift: 0.12,
    },
  );

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
        ref={bgVizRef}
        className={`mtab__bgviz ${info.playing ? "mtab__bgviz--playing" : ""}`}
        aria-hidden="true"
      >
        {Array.from({ length: BG_BAR_COUNT }, (_, i) => (
          <span
            key={i}
            ref={(el) => {
              bgBarRefs.current[i] = el;
            }}
            className="mtab__bgbar"
          />
        ))}
      </div>
      {lyricsOpen ? (
        <div className="mtab__lyrics-view">
          <div className="mtab__lyrics-head">
            <div className="mtab__lyrics-track">
              {art ? (
                <img className="mtab__lyrics-art" src={art} alt="" />
              ) : (
                <span className="mtab__lyrics-art mtab__lyrics-art--empty">
                  <Icon name="music_note" size={14} />
                </span>
              )}
              <span className="mtab__lyrics-meta">
                {[info.title, info.artist].filter(Boolean).join(" · ") || "—"}
              </span>
            </div>
            <button
              type="button"
              className="mtab__lyrics-back"
              onClick={() => setLyricsOpen(false)}
              title="Back to player"
            >
              <LyricsIcon />
              <span>Player</span>
            </button>
          </div>
          <LyricsPanel lyrics={lyrics} onSeek={media.seek} />
        </div>
      ) : (
        <>
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
                lyricsOpen={lyricsOpen}
                volIcon={volIcon}
                onToggleAudio={() => setAudioOpen((v) => !v)}
                onToggleLyrics={() => {
                  setAudioOpen(false);
                  setLyricsOpen(true);
                }}
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
        </>
      )}
    </div>
  );
}
