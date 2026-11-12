import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * True while the panel with this label is being shown by Rust.
 */
export function useEdgeVisible(label: string): boolean {
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const unlisten = listen<boolean>(`edge-visible:${label}`, (e) => setShown(e.payload));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [label]);

  return shown;
}
