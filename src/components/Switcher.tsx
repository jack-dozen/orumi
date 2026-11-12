import { useEffect, useRef, useState, type CSSProperties } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import { windowIconTarget } from "../useWindows";
import AppIcon from "./AppIcon";
import Thumb from "./Thumb";

type Item = {
  hwnd: number;
  title: string;
  exe: string;
  minimized: boolean;
  focused: boolean;
  path: string;
  aumid: string;
  /** Absolute path of the newest thumbnail file, if there is one. */
  thumb: string | null;
};

/** Grid geometry in CSS px. Rust computes it and sizes the window to fit. */
type Layout = { cols: number; rows: number; card_w: number; card_h: number; gap: number; pad: number };

// Dev aid: opening animation. "slide" = slides up from the bottom edge, "fade" = short fade with
// a small rise, "none" = appears instantly. Edit and save: the dev server reloads it right away.
const SWITCHER_ANIM: "slide" | "fade" | "none" = "slide";

// Dev aid: logs frame timing for 600 ms after the panel appears (the terminal shows "[ui] ...").
// Set to false when no longer needed.
const FRAME_PROBE = true;

/**
 * Alt+Tab display: a grid of window previews (several rows when there are many windows).
 * All logic (ordering, keys, selection) is in Rust; this component only renders the received
 * list, the selected index, and thumbnails as they arrive.
 */
export default function Switcher() {
  const visible = useEdgeVisible("switcher");
  const [items, setItems] = useState<Item[]>([]);
  const [layout, setLayout] = useState<Layout | null>(null);
  const [index, setIndex] = useState(0);
  // Newest thumbnail file per window: set by the opening payload, refreshed by "switcher-thumb".
  const [thumbs, setThumbs] = useState<Record<number, string>>({});
  const activeRef = useRef<HTMLDivElement>(null);
  // Probe: when Rust first asked to show the panel in this session (see FRAME_PROBE).
  const shownAtRef = useRef<number | null>(null);

  useEffect(() => {
    const a = listen<{ items: Item[]; index: number; layout: Layout }>("switcher-items", (e) => {
      shownAtRef.current = null;
      setItems(e.payload.items);
      setIndex(e.payload.index);
      setLayout(e.payload.layout);
      const t: Record<number, string> = {};
      for (const it of e.payload.items) if (it.thumb) t[it.hwnd] = it.thumb;
      setThumbs(t);
    });
    const b = listen<number>("switcher-select", (e) => {
      if (shownAtRef.current === null) shownAtRef.current = performance.now();
      setIndex(e.payload);
    });
    const c = listen<{ hwnd: number; thumb: string }>("switcher-thumb", (e) =>
      setThumbs((m) => ({ ...m, [e.payload.hwnd]: e.payload.thumb })),
    );
    return () => {
      a.then((fn) => fn());
      b.then((fn) => fn());
      c.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (!FRAME_PROBE || !visible) return;
    const start = performance.now();
    const lag = shownAtRef.current === null ? -1 : Math.round(start - shownAtRef.current);
    let last = start;
    let frames = 0;
    let max = 0;
    const slow: string[] = [];
    let raf = 0;
    const tick = (t: number) => {
      const dt = t - last;
      last = t;
      frames++;
      if (dt > max) max = dt;
      if (dt > 24) slow.push(`${Math.round(t - start)}ms:+${Math.round(dt)}`);
      if (t - start < 600) {
        raf = requestAnimationFrame(tick);
      } else {
        invoke("dev_log", {
          line: `switcher open: asked->visible ${lag} ms, ${frames} frames in 600 ms, max gap ${Math.round(max)} ms, slow: [${slow.join(" ")}]`,
        }).catch(() => {});
      }
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [visible]);

  // Keep the selected card visible when there are more rows than fit on screen.
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [index, items]);

  const gridStyle: CSSProperties | undefined = layout
    ? {
        gridTemplateColumns: `repeat(${layout.cols}, ${layout.card_w}px)`,
        gridAutoRows: `${layout.card_h}px`,
        gap: layout.gap,
        padding: layout.pad,
      }
    : undefined;

  return (
    <div className="swroot">
      <div
        className={`swpanel swpanel--${SWITCHER_ANIM} ${visible ? "swpanel--visible" : ""}`}
      >
        <div className="swgrid" style={gridStyle}>
          {items.map((it, i) => {
            const name = it.exe || it.title;
            const thumb = thumbs[it.hwnd];
            return (
              <div
                key={it.hwnd}
                ref={i === index ? activeRef : undefined}
                className={`swcard ${i === index ? "swcard--active" : ""} ${
                  it.minimized ? "swcard--min" : ""
                }`}
                onClick={() => invoke("switcher_pick", { index: i })}
              >
                <div className="swthumb">
                  {thumb ? (
                    <Thumb src={convertFileSrc(thumb)} />
                  ) : (
                    <AppIcon
                      target={windowIconTarget(it)}
                      fallback={name.charAt(0).toUpperCase()}
                      size={48}
                    />
                  )}
                </div>
                <div className="swlabel">
                  <AppIcon
                    target={windowIconTarget(it)}
                    fallback={name.charAt(0).toUpperCase()}
                    size={16}
                  />
                  <span>{it.title}</span>
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
