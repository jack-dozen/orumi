import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type QuickState = {
  /** null = this PC has no such radio. */
  wifi: boolean | null;
  bluetooth: boolean | null;
  airplane: boolean;
  dnd: boolean;
};

export type QuickKey = "wifi" | "bluetooth" | "airplane" | "dnd";

/** Quick-settings state: refreshed every 3 s while active, and right after a toggle. */
export function useQuick(active: boolean) {
  const [state, setState] = useState<QuickState | null>(null);
  const [busy, setBusy] = useState<QuickKey | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    const refresh = () =>
      invoke<QuickState>("quick_get")
        .then((s) => !cancelled && setState(s))
        .catch(() => {});
    refresh();
    const id = setInterval(refresh, 3000);
    return () => {
      cancelled = true;
      clearInterval(id);
    };
  }, [active]);

  const toggle = useCallback(async (which: QuickKey, on: boolean) => {
    setBusy(which);
    try {
      setState(await invoke<QuickState>("quick_set", { which, on }));
    } catch {
      /* the next refresh shows the real state */
    } finally {
      setBusy(null);
    }
  }, []);

  return { state, busy, toggle };
}

