import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import AppIcon from "./AppIcon";

type Item = {
  hwnd: number;
  title: string;
  icon: string | null;
  thumb: string | null;
  minimized: boolean;
};

/** Pratinjau window yang tumbuh dari tepi bar. Datanya dikirim Rust saat ikon di-hover. */
export default function Preview() {
  const visible = useEdgeVisible("preview");
  const [items, setItems] = useState<Item[]>([]);

  useEffect(() => {
    const unlisten = listen<Item[]>("preview-data", (e) => setItems(e.payload));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <div
      className="pvroot"
      onMouseEnter={() => invoke("preview_keep").catch(() => {})}
      onMouseLeave={() => invoke("preview_close_soon").catch(() => {})}
    >
      <div className={`pvcard ${visible ? "pvcard--visible" : ""}`}>
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
                <img src={convertFileSrc(it.thumb)} alt="" draggable={false} />
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
  );
}
