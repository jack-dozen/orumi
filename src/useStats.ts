import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type Stats = {
  cpu: number;
  mem_used: number;
  mem_total: number;
  disk_used: number;
  disk_total: number;
};

/** Fetch statistics every second, only while `active` = true. */
export function useStats(active: boolean): Stats | null {
  const [stats, setStats] = useState<Stats | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const tick = async () => {
      try {
        const s = await invoke<Stats>("get_stats");
        if (!cancelled) setStats(s);
      } catch {
        /* ignore and try again on the next tick */
      }
    };

    tick();
    const id = setInterval(tick, 1000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [active]);

  return stats;
}
