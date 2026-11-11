import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type BatteryStatus = {
  percent: number;
  charging: boolean;
  plugged: boolean;
};

/** Battery status every 15 seconds while `active`. null = no battery. */
export function useBattery(active: boolean): BatteryStatus | null {
  const [battery, setBattery] = useState<BatteryStatus | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const tick = async () => {
      try {
        const b = await invoke<BatteryStatus | null>("get_battery");
        if (!cancelled) setBattery(b);
      } catch {
        /* try again later */
      }
    };

    tick();
    const id = setInterval(tick, 15000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [active]);

  return battery;
}
