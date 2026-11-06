import type { MediaState } from "../useMedia";
import Icon from "./Icon";

const fmt = (ms: number) => {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

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

function Controls({ media }: { media: MediaState }) {
  const { info, control } = media;
  return (
    <div className="mcontrols">
      <button onClick={() => control("prev")} title="Sebelumnya">
        <Icon name="skip_previous" size={26} fill />
      </button>
      <button className="mcontrols__play" onClick={() => control("toggle")} title="Putar / jeda">
        <Icon name={info?.playing ? "pause" : "play_arrow"} size={30} fill />
      </button>
      <button onClick={() => control("next")} title="Berikutnya">
        <Icon name="skip_next" size={26} fill />
      </button>
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

/** Kartu kecil untuk dashboard. */
export function MiniPlayer({ media }: { media: MediaState }) {
  const { info, art } = media;
  if (!info) {
    return <div className="card card--placeholder">tidak ada media</div>;
  }
  return (
    <div className="card mini">
      <Cover art={art} className="mini__cover" />
      <div className="mini__title">{info.title || "—"}</div>
      <Progress media={media} />
      <Controls media={media} />
    </div>
  );
}

/** Tab media penuh. */
export function MediaTab({ media }: { media: MediaState }) {
  const { info, art } = media;
  if (!info) {
    return <div className="card card--placeholder card--fill">Tidak ada media yang diputar</div>;
  }
  return (
    <div className="card mtab">
      <Cover art={art} className="mtab__cover" />
      <div className="mtab__info">
        <div className="mtab__source">{info.source}</div>
        <div className="mtab__title">{info.title || "—"}</div>
        <div className="mtab__artist">
          {[info.artist, info.album].filter(Boolean).join(" · ")}
        </div>
        <Progress media={media} />
        <div className="mtab__times">
          <span>{fmt(info.position_ms)}</span>
          <span>{info.duration_ms > 0 ? fmt(info.duration_ms) : "--:--"}</span>
        </div>
        <Controls media={media} />
      </div>
    </div>
  );
}
