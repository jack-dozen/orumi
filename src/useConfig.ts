import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Hotkeys = { bar: string; notch: string; launcher: string };

/** mode "auto" = from the wallpaper; "custom" = uses `value`. */
export type ColorSlot = { mode: "auto" | "custom"; value: string };

export type Colors = {
  bg0: ColorSlot; // panel background
  bg1: ColorSlot; // card background
  fg: ColorSlot; // text and icons
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
  alt_tab: boolean;
  taskbar_autohide: boolean;
  taskbar_hidden: boolean;
  bar_app_style: "icons" | "dots";
  search_placeholder: string;
  hotkeys: Hotkeys;
};

export type SaveResult = { config: Config; warnings: string[] };

/** Current config; updates when any window saves it. */
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
