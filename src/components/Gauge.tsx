const R = 16;
const C = 2 * Math.PI * R;

/** Cincin persentase. value = null berarti data belum ada. */
export default function Gauge({ label, value }: { label: string; value: number | null }) {
  const pct = value === null ? 0 : Math.max(0, Math.min(100, value));

  return (
    <div className="gauge">
      <svg width="44" height="44" viewBox="0 0 44 44">
        <circle className="gauge__track" cx="22" cy="22" r={R} fill="none" strokeWidth="5" />
        <circle
          className="gauge__fill"
          cx="22"
          cy="22"
          r={R}
          fill="none"
          strokeWidth="5"
          strokeLinecap="round"
          strokeDasharray={C}
          strokeDashoffset={C * (1 - pct / 100)}
          transform="rotate(-90 22 22)"
        />
        <text x="22" y="26" textAnchor="middle" className="gauge__text">
          {value === null ? "—" : Math.round(pct)}
        </text>
      </svg>
      <span>{label}</span>
    </div>
  );
}
