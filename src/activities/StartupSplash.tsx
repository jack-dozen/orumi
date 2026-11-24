import { useEffect, useMemo } from "react";

const SPLASH_DURATION_MS = 2400;
/** Fires right as the four droplets reach the screen edges (at ~78-80% of SPLASH_DURATION_MS). */
const IMPACT_MS = 1860;

function greetingFor(hour: number): string {
  if (hour >= 5 && hour < 12) return "Good morning";
  if (hour >= 12 && hour < 17) return "Good afternoon";
  if (hour >= 17 && hour < 21) return "Good evening";
  return "Good night";
}

/**
 * Fullscreen click-through startup splash:
 * forms a liquid pill at the screen center with a dynamic greeting + "Welcome",
 * then splits into four droplets that fall and absorb into the four screen edges
 * (or spread out into the screen border when `borderActive` is enabled).
 */
export default function StartupSplash({
  borderActive,
  onBorderImpact,
  onDone,
}: {
  borderActive: boolean;
  onBorderImpact: () => void;
  onDone: () => void;
}) {
  const greeting = useMemo(() => greetingFor(new Date().getHours()), []);

  useEffect(() => {
    const impactTimer = borderActive ? window.setTimeout(onBorderImpact, IMPACT_MS) : null;
    const doneTimer = window.setTimeout(onDone, SPLASH_DURATION_MS);
    return () => {
      if (impactTimer !== null) window.clearTimeout(impactTimer);
      window.clearTimeout(doneTimer);
    };
  }, [borderActive, onBorderImpact, onDone]);

  return (
    <div className={`act-splash ${borderActive ? "act-splash--border" : ""}`} aria-hidden="true">
      {/* Compact SVG filter kept strictly around the center 380x380 stage for low GPU cost. */}
      <svg className="act-splash__defs" width="0" height="0" aria-hidden="true">
        <defs>
          <filter id="orumi-goo">
            <feGaussianBlur in="SourceGraphic" stdDeviation="11" result="blur" />
            <feColorMatrix
              in="blur"
              mode="matrix"
              values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 22 -10"
            />
          </filter>
        </defs>
      </svg>

      {/* Center liquid metaball stage */}
      <div className="act-splash__goo">
        <div className="act-splash__core" />
        <div className="act-splash__gdrop act-splash__gdrop--top" />
        <div className="act-splash__gdrop act-splash__gdrop--left" />
        <div className="act-splash__gdrop act-splash__gdrop--right" />
        <div className="act-splash__gdrop act-splash__gdrop--bottom" />
      </div>

      {/* Crisp text rendered outside the SVG threshold filter */}
      <div className="act-splash__label">
        <span className="act-splash__greeting">{greeting}</span>
        <span className="act-splash__welcome">Welcome</span>
      </div>

      {/* Four droplets gliding from the split radius to each screen edge */}
      <div className="act-splash__drop act-splash__drop--top" />
      <div className="act-splash__drop act-splash__drop--left" />
      <div className="act-splash__drop act-splash__drop--right" />
      <div className="act-splash__drop act-splash__drop--bottom" />

      {/* Edge landing capsules that absorb into the four sides */}
      <div className="act-splash__edge act-splash__edge--top" />
      <div className="act-splash__edge act-splash__edge--left" />
      <div className="act-splash__edge act-splash__edge--right" />
      <div className="act-splash__edge act-splash__edge--bottom" />
    </div>
  );
}
