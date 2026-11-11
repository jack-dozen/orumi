import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import { windowIconTarget } from "../useWindows";
import AppIcon from "./AppIcon";

type Item = {
  hwnd: number;
  title: string;
  exe: string;
  minimized: boolean;
  focused: boolean;
  path: string;
  aumid: string;
};

/**
 * Alt+Tab display. All logic (ordering, keys, selection) is in Rust; this component
 * only renders the received list and the currently selected index.
 */
export default function Switcher() {
  const visible = useEdgeVisible("switcher");
  const [items, setItems] = useState<Item[]>([]);
  const [index, setIndex] = useState(0);
  const activeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const a = listen<{ items: Item[]; index: number }>("switcher-items", (e) => {
      setItems(e.payload.items);
      setIndex(e.payload.index);
    });
    const b = listen<number>("switcher-select", (e) => setIndex(e.payload));
    return () => {
      a.then((fn) => fn());
      b.then((fn) => fn());
    };
  }, []);

  // Keep the selected item visible when the list is taller than the panel.
  useEffect(() => {
    activeRef.current?.scrollIntoView({ inline: "center", block: "nearest" });
  }, [index, items]);

  return (
    <div className="swroot">
      <div className={`swpanel ${visible ? "swpanel--visible" : ""}`}>
        <div className="swrow">
          {items.map((it, i) => {
            const name = it.exe || it.title;
            return (
              <div
                key={it.hwnd}
                ref={i === index ? activeRef : undefined}
                className={`switem ${i === index ? "switem--active" : ""} ${
                  it.minimized ? "switem--min" : ""
                }`}
                onClick={() => invoke("switcher_pick", { index: i })}
              >
                <AppIcon
                  target={windowIconTarget(it)}
                  fallback={name.charAt(0).toUpperCase()}
                  size={48}
                />
                <div className="swname">{name}</div>
                <div className="swtitle">{it.title}</div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
