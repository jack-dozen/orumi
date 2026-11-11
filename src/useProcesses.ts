import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type ProcEntry = {
  name: string;
  cpu: number;
  mem: number;
  count: number;
};

/** Fetch the top processes every 2 seconds, only while `active` = true. */
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
        /* try again on the next tick */
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
