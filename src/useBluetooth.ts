import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type BtCategory =
  | "headphones"
  | "speaker"
  | "audio"
  | "keyboard"
  | "mouse"
  | "gamepad"
  | "phone"
  | "other";

export type BtDevice = {
  id: string;
  address: number;
  address_str: string;
  name: string;
  category: BtCategory;
  connected: boolean;
  is_le: boolean;
  battery: number | null;
  can_connect: boolean;
};

export type BtListResult = {
  adapter: boolean;
  radio_on: boolean;
  devices: BtDevice[];
};

export type BtNearbyDevice = {
  id: string;
  name: string;
  category: BtCategory;
  is_le: boolean;
  can_pair: boolean;
};

export type BtPairPrompt = {
  id: string;
  name: string;
  kind: "confirm_pin" | "display_pin" | "provide_pin";
  pin: string | null;
};

/** Paired Bluetooth devices, live connect/disconnect, discovery scan, and custom PIN/confirm pairing. */
export function useBluetooth(active: boolean, detailOpen: boolean) {
  const [data, setData] = useState<BtListResult | null>(null);
  const [nearby, setNearby] = useState<BtNearbyDevice[]>([]);
  const [scanning, setScanning] = useState(false);
  const [pairPrompt, setPairPrompt] = useState<BtPairPrompt | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await invoke<BtListResult>("bt_list");
      setData(res);
    } catch {
      /* ignore transient error */
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const run = async () => {
      try {
        const res = await invoke<BtListResult>("bt_list");
        if (!cancelled) setData(res);
      } catch {
        /* ignore */
      }
    };
    run();
    const id = setInterval(run, detailOpen ? 4000 : 8000);
    const unlistenBt = listen("bt-changed", run);
    const unlistenQuick = listen("quick-changed", run);
    return () => {
      cancelled = true;
      clearInterval(id);
      unlistenBt.then((fn) => fn());
      unlistenQuick.then((fn) => fn());
    };
  }, [active, detailOpen]);

  useEffect(() => {
    if (!detailOpen) {
      setScanning(false);
      setNearby([]);
      setPairPrompt(null);
      invoke("bt_scan_stop").catch(() => {});
      return;
    }
    const uNearby = listen<BtNearbyDevice[]>("bt-nearby-changed", (e) => {
      setNearby(e.payload);
    });
    const uPrompt = listen<BtPairPrompt | null>("bt-pair-prompt", (e) => {
      setPairPrompt(e.payload);
    });
    return () => {
      uNearby.then((fn) => fn());
      uPrompt.then((fn) => fn());
      invoke("bt_scan_stop").catch(() => {});
    };
  }, [detailOpen]);

  const startScan = useCallback(async () => {
    setScanning(true);
    try {
      await invoke("bt_scan_start");
    } catch {
      setScanning(false);
    }
  }, []);

  const stopScan = useCallback(async () => {
    setScanning(false);
    await invoke("bt_scan_stop").catch(() => {});
  }, []);

  const connect = useCallback(
    async (d: BtDevice) => {
      await invoke("bt_connect", {
        id: d.id,
        address: d.address,
        isLe: d.is_le,
        category: d.category,
      });
      await refresh();
    },
    [refresh],
  );

  const disconnect = useCallback(
    async (d: BtDevice) => {
      await invoke("bt_disconnect", {
        id: d.id,
        address: d.address,
        isLe: d.is_le,
        category: d.category,
      });
      await refresh();
    },
    [refresh],
  );

  const unpair = useCallback(
    async (d: BtDevice) => {
      await invoke("bt_unpair", { id: d.id });
      await refresh();
    },
    [refresh],
  );

  const pair = useCallback(
    async (d: BtNearbyDevice) => {
      await invoke("bt_pair", { id: d.id, name: d.name });
      await refresh();
    },
    [refresh],
  );

  const respondPair = useCallback(async (accept: boolean, pin?: string) => {
    await invoke("bt_pair_respond", { accept, pin: pin ?? null });
    if (!accept) setPairPrompt(null);
  }, []);

  return {
    data,
    nearby,
    scanning,
    pairPrompt,
    refresh,
    startScan,
    stopScan,
    connect,
    disconnect,
    unpair,
    pair,
    respondPair,
  };
}
