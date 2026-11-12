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
  /** AppUserModelID (Store app) and program path, used to fetch the icon. */
  aumid: string;
  path: string;
  /** Absolute path of the thumbnail file (use convertFileSrc); null if there is none. */
  thumb: string | null;
};

export type Workspace = {
  id: string;
  name: string;
  active: boolean;
  windows: number[];
};

/** Icon address for a window, in the format understood by `AppIcon`. */
export const iconTarget = (w: { aumid: string; path: string }) =>
  w.aumid ? `aumid:${w.aumid}` : w.path ? `path:${w.path}` : null;

export const closeActivities = () => invoke("activities_close").catch(() => {});

/**
 * Activities status and data. Rust emits:
 *  - "activities-open"  (payload: window list + thumbnails) when the window is shown
 *  - "activities-close" when closing starts
 */
export function useActivities() {
  const [visible, setVisible] = useState(false);
  /** Increments each time it opens; used by React to reset the display. */
  const [openCount, setOpenCount] = useState(0);
  const [wins, setWins] = useState<SnapWindow[]>([]);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [apps, setApps] = useState<AppEntry[]>([]);

  const refreshWorkspaces = useCallback(() => {
    return invoke<Workspace[]>("ws_list")
      .then(setWorkspaces)
      .catch(() => setWorkspaces([]));
  }, []);

  // The app list is loaded once (cached by Rust, shared with the launcher).
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
        // Two frames ensure the opening transition is actually visible.
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
