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

/** Icon address for a window with fallback chain, in the format understood by `AppIcon`. */
export const iconTarget = (w: { aumid: string; path: string; hwnd?: number }) => {
  const parts: string[] = [];
  if (w.aumid) parts.push(`aumid:${w.aumid}`);
  if (w.path) parts.push(`path:${w.path}`);
  if (w.hwnd) parts.push(`hwnd:${w.hwnd}`);
  return parts.length > 0 ? parts.join("\n") : null;
};

export const closeActivities = () => invoke("activities_close").catch(() => {});

/**
 * Activities status and data. Rust emits:
 *  - "activities-open"  (payload: window list + thumbnails) when the window is shown
 *  - "activities-close" when closing starts
 */
export function useActivities(initialWins?: SnapWindow[], embedded = false) {
  const [visible, setVisible] = useState(embedded);
  /** Increments each time it opens; used by React to reset the display. */
  const [openCount, setOpenCount] = useState(embedded ? 1 : 0);
  const [wins, setWins] = useState<SnapWindow[]>(initialWins ?? []);
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [apps, setApps] = useState<AppEntry[]>([]);

  useEffect(() => {
    if (embedded && initialWins) {
      setWins(initialWins);
      setOpenCount((n) => n + 1);
    }
  }, [embedded, initialWins]);

  const refreshWorkspaces = useCallback(() => {
    return invoke<Workspace[]>("ws_list")
      .then(setWorkspaces)
      .catch(() => setWorkspaces([]));
  }, []);

  useEffect(() => {
    if (embedded) refreshWorkspaces();
  }, [embedded, refreshWorkspaces]);

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
      listen("workspaces-changed", () => {
        refreshWorkspaces();
      }),
    ];
    return () => {
      offs.forEach((p) => p.then((off) => off()));
    };
  }, [refreshWorkspaces]);

  return { visible, openCount, wins, workspaces, apps, refreshWorkspaces };
}
