import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** Pemakaian GPU (0-100), atau null kalau belum ada/tidak tersedia. */
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
        /* coba lagi di tick berikutnya */
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
