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

  useEffect(() => {
    const a = listen<{ items: Item[]; index: number; layout: Layout }>("switcher-items", (e) => {
      setItems(e.payload.items);
      setIndex(e.payload.index);
      setLayout(e.payload.layout);
      const t: Record<number, string> = {};
      for (const it of e.payload.items) if (it.thumb) t[it.hwnd] = it.thumb;
      setThumbs(t);
    });
    const b = listen<number>("switcher-select", (e) => setIndex(e.payload));
    const c = listen<{ hwnd: number; thumb: string }>("switcher-thumb", (e) =>
      setThumbs((m) => ({ ...m, [e.payload.hwnd]: e.payload.thumb })),
    );
    return () => {
      a.then((fn) => fn());
      b.then((fn) => fn());
      c.then((fn) => fn());
    };
  }, []);

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
      <div className={`swpanel ${visible ? "swpanel--visible" : ""}`}>
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
