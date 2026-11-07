import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Hotkeys = { bar: string; notch: string; launcher: string };

/** mode "auto" = dari wallpaper; "custom" = memakai `value`. */
export type ColorSlot = { mode: "auto" | "custom"; value: string };

export type Colors = {
  bg0: ColorSlot; // latar panel
  bg1: ColorSlot; // latar kartu
  fg: ColorSlot; // teks dan ikon
  accent: ColorSlot;
};

export type Config = {
  ui_scale: number;
  weather_city: string;
  bg_tint: number;
  colors: Colors;
  wallpaper_dir: string;
  wallpaper_switcher: "builtin" | "external";
  wallpaper_command: string;
  hotkeys: Hotkeys;
};

export type SaveResult = { config: Config; warnings: string[] };

/** Config terkini; ikut berubah saat window mana pun menyimpannya. */
export function useConfig(): Config | null {
  const [config, setConfig] = useState<Config | null>(null);

  useEffect(() => {
    let cancelled = false;
    invoke<Config>("get_config")
      .then((c) => {
        if (!cancelled) setConfig(c);
      })
      .catch(() => {});
    const unlisten = listen<Config>("config-changed", (e) => setConfig(e.payload));
    return () => {
      cancelled = true;
      unlisten.then((fn) => fn());
    };
  }, []);

  return config;
}

export function saveConfig(config: Config): Promise<SaveResult> {
  return invoke<SaveResult>("set_config", { config });
}
