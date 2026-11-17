import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * True while the panel with this label is being shown by Rust.
 * Opening waits two animation frames so WebView2 commits the hidden initial
 * layout (mode, compact height, focus) before starting the CSS transition.
 */
export function useEdgeVisible(label: string): boolean {
  const [shown, setShown] = useState(false);
  const raf1 = useRef(0);
  const raf2 = useRef(0);

  useEffect(() => {
    const clearPending = () => {
      cancelAnimationFrame(raf1.current);
      cancelAnimationFrame(raf2.current);
      raf1.current = 0;
      raf2.current = 0;
    };

    const unlisten = listen<boolean>(`edge-visible:${label}`, (e) => {
      clearPending();
      if (!e.payload) {
        setShown(false);
        return;
      }
      raf1.current = requestAnimationFrame(() => {
        raf2.current = requestAnimationFrame(() => {
          setShown(true);
        });
      });
    });

    return () => {
      clearPending();
      unlisten.then((fn) => fn());
    };
  }, [label]);

  return shown;
}
