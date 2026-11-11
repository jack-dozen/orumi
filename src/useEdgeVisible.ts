import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * True while the panel with this label is being shown by Rust.
 *
 * When it becomes true, wait for two actual frames before changing the display.
 * Otherwise, on the first opening WebView2 may not draw a frame until after the
 * animation finishes, making the panel appear to "jump" instead of sliding in.
 * If no frame arrives within 500 ms (WebView is not drawing), show it anyway.
 */
export function useEdgeVisible(label: string): boolean {
  const [requested, setRequested] = useState(false);
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const unlisten = listen<boolean>(`edge-visible:${label}`, (e) => setRequested(e.payload));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [label]);

  useEffect(() => {
    if (!requested) {
      setShown(false);
      return;
    }
    let raf2 = 0;
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => setShown(true));
    });
    const fallback = window.setTimeout(() => setShown(true), 500);
    return () => {
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
      window.clearTimeout(fallback);
    };
  }, [requested]);

  return shown;
}
