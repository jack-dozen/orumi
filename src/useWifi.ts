import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type WifiStatus = {
  adapter: boolean;
  connected: boolean;
  ssid: string | null;
  signal: number;
  location_denied: boolean;
};

/** WiFi status: updated every 4 seconds and immediately when Windows reports a change. */
export function useWifi(active: boolean): WifiStatus | null {
  const [wifi, setWifi] = useState<WifiStatus | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const refresh = async () => {
      try {
        const s = await invoke<WifiStatus>("get_wifi");
        if (!cancelled) setWifi(s);
      } catch {
        /* try again later */
      }
    };

    refresh();
    const id = setInterval(refresh, 4000);
    const unlisten = listen("wifi-changed", refresh);
    return () => {
      cancelled = true;
      clearInterval(id);
      unlisten.then((fn) => fn());
    };
  }, [active]);

  return wifi;
}
