import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import AppIcon from "./AppIcon";
import Thumb from "./Thumb";

type Item = {
  hwnd: number;
  title: string;
  icon: string | null;
  thumb: string | null;
  minimized: boolean;
};

type Payload = { items: Item[]; top: number; width: number; height: number };

/** Window preview that grows from the bar edge. Rust sends its data when an icon is hovered. */
export default function Preview() {
  const visible = useEdgeVisible("preview");
  const [data, setData] = useState<Payload | null>(null);
  // Bumped when the content is replaced while the card is already shown (crossfade trigger).
  const [swap, setSwap] = useState(0);
  const visibleRef = useRef(false);
  visibleRef.current = visible;
  const dataRef = useRef<Payload | null>(null);

  useEffect(() => {
    const unlisten = listen<Payload>("preview-data", (e) => {
      // Crossfade only when a different set of windows replaces the shown one; a refresh of the
      // same windows (new thumbnails) updates in place.
      const prev = dataRef.current;
      const sameSet =
        !!prev &&
        prev.items.length === e.payload.items.length &&
        prev.items.every((it, i) => it.hwnd === e.payload.items[i].hwnd);
      if (visibleRef.current && !sameSet) setSwap((n) => n + 1);
      dataRef.current = e.payload;
      setData(e.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const items = data?.items ?? [];

  return (
    <div className="pvroot">
      <div
        className={`pvcard ${visible ? "pvcard--visible" : ""}`}
        style={data ? { top: data.top, width: data.width, height: data.height } : undefined}
        onMouseEnter={() => invoke("preview_keep").catch(() => {})}
        onMouseLeave={() => invoke("preview_close_soon").catch(() => {})}
      >
        <div key={swap} className={swap > 0 ? "pvitems pvitems--swap" : "pvitems"}>
          {items.map((it) => (
            <div
              key={it.hwnd}
              className="pvitem"
              onClick={() => {
                invoke("focus_window", { hwnd: it.hwnd }).catch(() => {});
                invoke("preview_close").catch(() => {});
              }}
            >
              <div className="pvthumb">
                {it.thumb ? (
                  <Thumb src={convertFileSrc(it.thumb)} />
                ) : (
                  <div className="pvthumb__empty">
                    <AppIcon target={it.icon} fallback="?" size={48} />
                  </div>
                )}
                {it.minimized && <span className="pvbadge">diperkecil</span>}
              </div>
              <div className="pvtitle">
                <AppIcon target={it.icon} fallback="•" size={16} />
                <span>{it.title}</span>
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
