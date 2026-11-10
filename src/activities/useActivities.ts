import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type AppEntry = { name: string; id: string };

export type SnapWindow = {
  hwnd: number;
  title: string;
  exe: string;
  minimized: boolean;
  focused: boolean;
  /** JPEG base64 tanpa prefix; null kalau window tidak bisa ditangkap. */
  thumb: string | null;
};

export type Workspace = {
  id: string;
  name: string;
  active: boolean;
  windows: number[];
};

export const closeActivities = () => invoke("activities_close").catch(() => {});

/**
 * Status dan data Activities. Rust memancarkan:
 *  - "activities-open"  (payload: daftar window + thumbnail) saat window tampil
 *  - "activities-close" saat mulai menutup
 */
export function useActivities() {
  const [visible, setVisible] = useState(false);
  /** Naik setiap kali dibuka; dipakai React untuk mereset tampilan. */
  const [openCount, setOpenCount] = useState(0);
  const [wins, setWins] = useState<SnapWindow[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [apps, setApps] = useState<AppEntry[]>([]);

  const refreshWorkspaces = useCallback(() => {
    return invoke<Workspace[]>("ws_list")
      .then(setWorkspaces)
      .catch(() => setWorkspaces([]));
  }, []);

  // Daftar app dimuat sekali (di-cache oleh Rust, sama dengan yang dipakai launcher).
  useEffect(() => {
    invoke<AppEntry[]>("list_apps", { refresh: false })
      .then(setApps)
      .catch(() => {});
  }, []);

  useEffect(() => {
    const offs = [
      listen<SnapWindow[]>("activities-open", (e) => {
        setWins(e.payload ?? []);
        setOpenCount((n) => n + 1);
        refreshWorkspaces();
        // Dua frame supaya transisi buka benar-benar terlihat.
        requestAnimationFrame(() => requestAnimationFrame(() => setVisible(true)));
      }),
      listen("activities-close", () => setVisible(false)),
    ];
    return () => {
      offs.forEach((p) => p.then((off) => off()));
    };
  }, [refreshWorkspaces]);

  return { visible, openCount, wins, workspaces, apps, refreshWorkspaces };
}
