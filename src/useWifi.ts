import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type WifiStatus = {
  adapter: boolean;
  connected: boolean;
  ssid: string | null;
  signal: number;
  location_denied: boolean;
};

export type WifiNetwork = {
  ssid: string;
  signal: number;
  connected: boolean;
  saved: boolean;
  profile_name: string | null;
  secured: boolean;
  auth: string;
  auth_kind: string;
  connectable: boolean;
};

export type WifiListResult = {
  adapter: boolean;
  radio_on: boolean;
  location_denied: boolean;
  connected_ssid: string | null;
  networks: WifiNetwork[];
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

/** Full WiFi network list, scan, connect (with optional password), disconnect, and forget. */
export function useWifiList(active: boolean) {
  const [data, setData] = useState<WifiListResult | null>(null);
  const [scanning, setScanning] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const res = await invoke<WifiListResult>("wifi_list");
      setData(res);
    } catch {
      /* ignore transient errors */
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const run = async () => {
      try {
        const res = await invoke<WifiListResult>("wifi_list");
        if (!cancelled) setData(res);
      } catch {
        /* ignore */
      }
    };
    run();
    const id = setInterval(run, 5000);
    const unlisten = listen("wifi-changed", run);
    const unlistenQuick = listen("quick-changed", run);
    return () => {
      cancelled = true;
      clearInterval(id);
      unlisten.then((fn) => fn());
      unlistenQuick.then((fn) => fn());
    };
  }, [active]);

  const scan = useCallback(async () => {
    setScanning(true);
    try {
      await invoke("wifi_scan");
      await new Promise((r) => setTimeout(r, 900));
      await refresh();
    } catch {
      await refresh();
    } finally {
      setScanning(false);
    }
  }, [refresh]);

  const connect = useCallback(
    async (
      ssid: string,
      opts?: { password?: string; autoConnect?: boolean; authKind?: string },
    ) => {
      await invoke("wifi_connect", {
        ssid,
        password: opts?.password ?? null,
        autoConnect: opts?.autoConnect ?? true,
        authKind: opts?.authKind ?? null,
      });
      await refresh();
    },
    [refresh],
  );

  const disconnect = useCallback(async () => {
    await invoke("wifi_disconnect");
    await refresh();
  }, [refresh]);

  const forget = useCallback(
    async (profileName: string) => {
      await invoke("wifi_forget", { profileName });
      await refresh();
    },
    [refresh],
  );

  return { data, scanning, refresh, scan, connect, disconnect, forget };
}
