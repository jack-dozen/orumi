import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type ProcEntry = {
  name: string;
  cpu: number;
  mem: number;
  count: number;
};

/** Ambil proses teratas tiap 2 detik, hanya saat `active` = true. */
export function useProcesses(active: boolean): ProcEntry[] {
  const [procs, setProcs] = useState<ProcEntry[]>([]);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const tick = async () => {
      try {
        const p = await invoke<ProcEntry[]>("get_top_processes");
        if (!cancelled) setProcs(p);
      } catch {
        /* coba lagi di tick berikutnya */
      }
    };

    tick();
    const id = setInterval(tick, 2000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [active]);

  return procs;
}
