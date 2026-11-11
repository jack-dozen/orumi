import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type WinEntry = {
  hwnd: number;
  title: string;
  exe: string;
  minimized: boolean;
  focused: boolean;
  path: string;
  aumid: string;
};

/** Icon address for a window: AppUserModelID (Store app) or program path. */
export function windowIconTarget(w: { aumid: string; path: string }): string | null {
  if (w.aumid) return `aumid:${w.aumid}`;
  if (w.path) return `path:${w.path}`;
  return null;
}

/** One app = one dot in the bar, regardless of the number of windows. */
export type AppGroup = {
  key: string;
  name: string;
  windows: WinEntry[];
  focused: boolean;
  allMinimized: boolean;
  icon: string | null;
};

/** Open apps. Updated when Windows reports a window change. */
export function useWindows(active: boolean): AppGroup[] {
  const [groups, setGroups] = useState<AppGroup[]>([]);
  const order = useRef<string[]>([]); // stable order: new apps are appended to the end
  const lastFocused = useRef<number | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const refresh = async () => {
      try {
        const list = await invoke<WinEntry[]>("list_windows");
        if (cancelled) return;

        // If one of our windows is in front (launcher, bar), none are "focused" in the list:
        // keep the last focused app as long as its window still exists.
        const fg = list.find((w) => w.focused)?.hwnd;
        if (fg !== undefined) lastFocused.current = fg;
        else if (!list.some((w) => w.hwnd === lastFocused.current)) lastFocused.current = null;

        const byKey = new Map<string, WinEntry[]>();
        for (const w of list) {
          const entry = { ...w, focused: w.hwnd === lastFocused.current };
          const key = w.exe ? w.exe.toLowerCase() : `uwp:${w.title}`;
          const arr = byKey.get(key);
          if (arr) arr.push(entry);
          else byKey.set(key, [entry]);
        }

        order.current = order.current
          .filter((k) => byKey.has(k))
          .concat([...byKey.keys()].filter((k) => !order.current.includes(k)));

        setGroups(
          order.current.map((key) => {
            const windows = byKey.get(key)!;
            return {
              key,
              name: windows[0].exe || windows[0].title,
              icon: windowIconTarget(windows[0]),
              windows,
              focused: windows.some((w) => w.focused),
              allMinimized: windows.every((w) => w.minimized),
            };
          }),
        );
      } catch {
        /* try again on the next event */
      }
    };

    refresh();
    const unlisten = listen("windows-changed", refresh);
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, [active]);

  return groups;
}
