import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** GPU usage (0-100), or null if unavailable. */
export function useGpu(active: boolean): number | null {
  const [gpu, setGpu] = useState<number | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const tick = async () => {
      try {
        const v = await invoke<number | null>("get_gpu");
        if (!cancelled) setGpu(v);
      } catch {
        /* try again on the next tick */
      }
    };

    tick();
    const id = setInterval(tick, 1000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [active]);

  return gpu;
}
