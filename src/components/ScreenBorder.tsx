import { useEffect, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useConfig } from "../useConfig";

const BORDER_RADIUS = 24;
/** Must match `BORDER_BLEED_DIP` in `src-tauri/src/edge.rs`. */
const BORDER_BLEED = 16;
const SPREAD_DURATION_MS = 700;

type BorderMode = "hidden" | "visible" | "spread";

/**
 * Fullscreen click-through Caelestia-style screen border frame with rounded inner corners.
 * Uses a single fullscreen masked surface so solid and pixel-aligned Mica backdrops have zero
 * seams between edges and corners at any DPI or UI scale.
 */
export default function ScreenBorder() {
  const cfg = useConfig();
  const ref = useRef<HTMLDivElement | null>(null);
  const [mode, setMode] = useState<BorderMode>("hidden");
  const [spreadKey, setSpreadKey] = useState(0);
  const [size, setSize] = useState<{ w: number; h: number }>({
    w: window.innerWidth,
    h: window.innerHeight,
  });

  useEffect(() => {
    let cancelled = false;
    const unlistenVisible = listen<boolean>("border-visible", (e) => {
      if (!cancelled) setMode(e.payload ? "visible" : "hidden");
    });
    const unlistenSpread = listen("border-spread", () => {
      if (cancelled) return;
      setSpreadKey((k) => k + 1);
      setMode("spread");
    });

    void Promise.all([unlistenVisible, unlistenSpread]).then(() => {
      if (cancelled) return;
      invoke<string>("border_mode")
        .then((initial) => {
          if (cancelled) return;
          if (initial === "spread") {
            setSpreadKey((k) => k + 1);
            setMode("spread");
          } else if (initial === "visible") {
            setMode("visible");
          } else {
            setMode("hidden");
          }
        })
        .catch(() => {});
    });

    return () => {
      cancelled = true;
      unlistenVisible.then((fn) => fn());
      unlistenSpread.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (mode !== "spread") return;
    const id = window.setTimeout(
      () => setMode((m) => (m === "spread" ? "visible" : m)),
      SPREAD_DURATION_MS,
    );
    return () => window.clearTimeout(id);
  }, [mode, spreadKey]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const update = () => {
      const w = Math.max(1, el.clientWidth || window.innerWidth);
      const h = Math.max(1, el.clientHeight || window.innerHeight);
      setSize((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    window.addEventListener("resize", update);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", update);
    };
  }, []);

  const enabled = Boolean(cfg?.screen_border);
  const thickness = Math.max(6, Math.min(16, cfg?.screen_border_size ?? 10));
  const inset = BORDER_BLEED + thickness;
  const innerW = Math.max(0, size.w - inset * 2);
  const innerH = Math.max(0, size.h - inset * 2);
  const radius = Math.min(BORDER_RADIUS, innerW / 2, innerH / 2);
  const band = inset + radius + 2;
  const spreading = enabled && mode === "spread";
  const rootClass = spreading
    ? "sborder sborder--spreading"
    : enabled && mode === "visible"
      ? "sborder sborder--visible"
      : "sborder";

  return (
    <div
      ref={ref}
      className={rootClass}
      style={
        {
          "--border-band": `${band}px`,
          "--border-bleed": `${BORDER_BLEED}px`,
        } as CSSProperties
      }
    >
      <svg className="sborder__defs" width="0" height="0" aria-hidden="true">
        <defs>
          <mask
            id="orumi-sborder-mask"
            maskUnits="userSpaceOnUse"
            x={-2}
            y={-2}
            width={size.w + 4}
            height={size.h + 4}
          >
            <rect x={-2} y={-2} width={size.w + 4} height={size.h + 4} fill="#ffffff" />
            <rect
              x={inset}
              y={inset}
              width={innerW}
              height={innerH}
              rx={radius}
              ry={radius}
              fill="#000000"
            />
          </mask>
        </defs>
      </svg>
      <div className="sborder__canvas">
        {spreading ? (
          (["top", "bottom", "left", "right"] as const).map((side) => (
            <div key={`${spreadKey}-${side}`} className={`sborder__frame sborder__wave sborder__wave--${side}`} />
          ))
        ) : (
          <div className="sborder__frame" />
        )}
      </div>
    </div>
  );
}
