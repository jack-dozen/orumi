import { useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  argbFromHex,
  hexFromArgb,
  sourceColorFromImage,
  themeFromSourceColor,
} from "@material/material-color-utilities";

/** Warna dasar kalau wallpaper tidak bisa dibaca (polos atau tidak ada). */
const FALLBACK_SEED = argbFromHex("#3b82f6");
const CHECK_EVERY_MS = 10_000;

type Tokens = Record<string, string>;

/** Token tema gelap M3, dari nada (tone) tiap palet sesuai spesifikasi Material 3. */
function buildTokens(seed: number): Tokens {
  const p = themeFromSourceColor(seed).palettes;
  const hex = hexFromArgb;
  return {
    "--surface": hex(p.neutral.tone(6)),
    "--surface-container": hex(p.neutral.tone(12)),
    "--surface-container-high": hex(p.neutral.tone(17)),
    "--surface-container-highest": hex(p.neutral.tone(22)),
    "--on-surface": hex(p.neutral.tone(90)),
    "--on-surface-variant": hex(p.neutralVariant.tone(80)),
    "--outline": hex(p.neutralVariant.tone(60)),
    "--outline-variant": hex(p.neutralVariant.tone(30)),
    "--primary": hex(p.primary.tone(80)),
    "--on-primary": hex(p.primary.tone(20)),
    "--primary-container": hex(p.primary.tone(30)),
    "--on-primary-container": hex(p.primary.tone(90)),
    "--secondary-container": hex(p.secondary.tone(30)),
    "--on-secondary-container": hex(p.secondary.tone(90)),
    "--error": hex(p.error.tone(80)),
    "--error-container": hex(p.error.tone(30)),
    "--on-error-container": hex(p.error.tone(90)),
  };
}

function applyTokens(tokens: Tokens) {
  const root = document.documentElement.style;
  for (const [name, value] of Object.entries(tokens)) root.setProperty(name, value);
}

async function seedFromWallpaper(): Promise<number> {
  try {
    const url = await invoke<string | null>("wallpaper_image");
    if (!url) return FALLBACK_SEED;
    const img = new Image();
    img.src = url;
    await img.decode();
    return await sourceColorFromImage(img);
  } catch {
    return FALLBACK_SEED;
  }
}

/**
 * Semua window mengikuti tema yang dihitung satu window sumber (notch).
 * Wallpaper diperiksa saat ada panel yang dibuka (murah: hanya membandingkan stempel),
 * bukan berkala, dan dihitung ulang paksa saat event "theme-refresh" diterima
 * (nanti dari wallpaper changer atau pengaturan warna).
 */
export function useTheme(isSource: boolean) {
  useEffect(() => {
    let cancelled = false;
    let stamp: string | null | undefined; // undefined = belum pernah dihitung
    let lastCheck = 0;
    const unlistens: Promise<() => void>[] = [];

    // Semua window: pakai tema yang sudah ada, lalu ikuti perubahannya.
    invoke<string | null>("get_theme")
      .then((json) => {
        if (json && !cancelled) applyTokens(JSON.parse(json));
      })
      .catch(() => {});
    unlistens.push(listen<string>("theme-changed", (e) => applyTokens(JSON.parse(e.payload))));

    const check = async (force: boolean) => {
      if (!force && Date.now() - lastCheck < 3000) return;
      lastCheck = Date.now();
      try {
        const now = await invoke<string | null>("wallpaper_stamp");
        if (!force && now === stamp) return;
        stamp = now;
        const tokens = buildTokens(await seedFromWallpaper());
        if (cancelled) return;
        applyTokens(tokens);
        await invoke("set_theme", { json: JSON.stringify(tokens) });
      } catch {
        /* coba lagi pada pemicu berikutnya */
      }
    };

    if (isSource) {
      check(true);
      for (const label of ["bar", "notch", "launcher", "overlay"]) {
        unlistens.push(
          listen<boolean>(`edge-visible:${label}`, (e) => {
            if (e.payload) check(false);
          }),
        );
      }
      unlistens.push(listen("theme-refresh", () => check(true)));
    }

    return () => {
      cancelled = true;
      unlistens.forEach((u) => u.then((fn) => fn()));
    };
  }, [isSource]);
}
