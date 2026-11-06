import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type Stats = {
  cpu: number;
  mem_used: number;
  mem_total: number;
  disk_used: number;
  disk_total: number;
};

/** Ambil statistik tiap detik, hanya saat `active` = true. */
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
        /* abaikan, coba lagi di tick berikutnya */
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
