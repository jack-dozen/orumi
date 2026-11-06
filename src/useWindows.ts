import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type WinEntry = {
  hwnd: number;
  title: string;
  exe: string;
  minimized: boolean;
  focused: boolean;
};

/** Satu aplikasi = satu titik di bar, berapa pun jumlah window-nya. */
export type AppGroup = {
  key: string;
  name: string;
  windows: WinEntry[];
  focused: boolean;
  allMinimized: boolean;
};

/** App yang sedang terbuka. Diperbarui saat Windows melaporkan perubahan window. */
export function useWindows(active: boolean): AppGroup[] {
  const [groups, setGroups] = useState<AppGroup[]>([]);
  const order = useRef<string[]>([]); // urutan stabil: app baru ditambah di belakang
  const lastFocused = useRef<number | null>(null);

  useEffect(() => {
    if (!active) return;
    let cancelled = false;

    const refresh = async () => {
      try {
        const list = await invoke<WinEntry[]>("list_windows");
        if (cancelled) return;

        // Kalau window kita sendiri yang di depan (launcher, bar), tidak ada yang "focused"
        // di daftar: pertahankan app terakhir yang fokus selama window-nya masih ada.
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
              windows,
              focused: windows.some((w) => w.focused),
              allMinimized: windows.every((w) => w.minimized),
            };
          }),
        );
      } catch {
        /* coba lagi pada event berikutnya */
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
