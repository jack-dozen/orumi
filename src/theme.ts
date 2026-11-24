import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow, primaryMonitor } from "@tauri-apps/api/window";
import {
  argbFromRgb,
  blueFromArgb,
  greenFromArgb,
  Hct,
  hexFromArgb,
  QuantizerCelebi,
  redFromArgb,
  sanitizeDegreesInt,
  TonalPalette,
} from "@material/material-color-utilities";
import type { ColorSlot, Config, PanelEdge, PanelsConfig } from "./useConfig";

/** Base colors when the wallpaper cannot be read (plain, grayscale, or missing). */
const FALLBACK_SEED = argbFromRgb(128, 128, 128);

type Tokens = Record<string, string>;
type RGB = [number, number, number];

// ---------- Color utilities ----------

const toRgb = (hex: string): RGB => [
  parseInt(hex.slice(1, 3), 16),
  parseInt(hex.slice(3, 5), 16),
  parseInt(hex.slice(5, 7), 16),
];

const toHex = ([r, g, b]: RGB) =>
  "#" +
  [r, g, b]
    .map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0"))
    .join("");

/** Blend two colors: t = 0 -> a, t = 1 -> b. */
function mix(a: string, b: string, t: number): string {
  const x = toRgb(a);
  const y = toRgb(b);
  return toHex([x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]);
}

/** Luminansi relatif (WCAG). */
function luminance(hex: string): number {
  const [r, g, b] = toRgb(hex).map((v) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** Choose the more legible of two candidates on top of the `on` color. */
const pickReadable = (on: string, a: string, b: string) => (contrast(on, a) >= contrast(on, b) ? a : b);

// ---------- Token ----------

/** Four colors (automatic from the wallpaper or custom) -> all CSS tokens. */
function buildTokens(cfg: Pick<Config, "colors" | "bg_tint">, seed: number): Tokens {
  const r = redFromArgb(seed);
  const g = greenFromArgb(seed);
  const b = blueFromArgb(seed);
  const spread = Math.max(r, g, b) - Math.min(r, g, b);
  const seedHct = Hct.fromInt(seed);

  const pick = (slot: ColorSlot, auto: () => string) =>
    slot.mode === "custom" ? slot.value : auto();

  let bg0: string;
  let bg1: string;
  let fg: string;
  let accent: string;

  if (spread < 6) {
    // Pure neutral grays so CAM16's 209.5 deg neutral hue never tints grayscale wallpapers blue.
    bg0 = pick(cfg.colors.bg0, () => "#111111");
    bg1 = pick(cfg.colors.bg1, () => "#282828");
    fg = pick(cfg.colors.fg, () => "#e3e3e3");
    accent = pick(cfg.colors.accent, () => "#d4d4d4");
  } else {
    const hue = seedHct.hue;
    const baseChroma = Math.max(seedHct.chroma, spread * 0.6);
    const accentChroma =
      baseChroma >= 28 ? Math.max(48, seedHct.chroma) : Math.max(12, baseChroma * 1.6);
    const tintMax = Math.min(36, baseChroma * 1.4);
    // bg_tint 0 = nearly neutral gray; 100 = a background tinted up to the wallpaper's chroma.
    const bgChroma = 2 + (cfg.bg_tint / 100) * Math.max(0, tintMax - 2);
    const tinted = TonalPalette.fromHueAndChroma(hue, bgChroma);
    const soft = TonalPalette.fromHueAndChroma(hue, Math.min(6, baseChroma * 0.5));
    const accentPalette = TonalPalette.fromHueAndChroma(hue, accentChroma);

    bg0 = pick(cfg.colors.bg0, () => hexFromArgb(tinted.tone(6)));
    bg1 = pick(cfg.colors.bg1, () => hexFromArgb(tinted.tone(17)));
    fg = pick(cfg.colors.fg, () => hexFromArgb(soft.tone(92)));
    accent = pick(cfg.colors.accent, () => hexFromArgb(accentPalette.tone(80)));
  }

  // Light backgrounds need a darker error color for legibility.
  const light = luminance(bg0) > 0.5;

  return {
    "--surface": bg0,
    "--surface-container": mix(bg0, bg1, 0.5),
    "--surface-container-high": bg1,
    "--surface-container-highest": mix(bg1, fg, 0.1),
    "--on-surface": fg,
    "--on-surface-variant": mix(fg, bg0, 0.25),
    "--outline": mix(bg0, fg, 0.5),
    "--outline-variant": mix(bg1, fg, 0.2),
    "--primary": accent,
    "--on-primary": pickReadable(accent, bg0, fg),
    "--primary-container": mix(bg1, accent, 0.4),
    "--on-primary-container": fg,
    "--secondary-container": mix(bg1, accent, 0.2),
    "--on-secondary-container": fg,
    "--error": light ? "#ba1a1a" : "#ffb4ab",
    "--error-container": light ? "#ffdad6" : "#93000a",
    "--on-error-container": light ? "#410002" : "#ffdad6",
  };
}

export type UiStyle = "material" | "fluent";

let currentUiStyle: UiStyle = "material";
let currentMica = false;
const styleListeners = new Set<(style: UiStyle) => void>();

function normalizeUiStyle(raw: unknown): UiStyle {
  return raw === "fluent" ? "fluent" : "material";
}

function applyUiStyle(style: UiStyle) {
  currentUiStyle = style;
  document.documentElement.dataset.style = style;
  for (const fn of styleListeners) fn(style);
}

function applyMicaEnabled(enabled: boolean) {
  currentMica = enabled;
  document.documentElement.dataset.mica = enabled ? "on" : "off";
}

function applyScreenBorder(enabled: boolean, size: number) {
  const clamped = Math.max(6, Math.min(16, Math.round(size || 10)));
  document.documentElement.dataset.border = enabled ? "on" : "off";
  document.documentElement.style.setProperty("--border-size", `${clamped}px`);
  document.documentElement.style.setProperty("--border-radius", "24px");
}

let currentPanelEdge: PanelEdge = "left";
const edgeListeners = new Set<(e: PanelEdge) => void>();

function resolvePanelEdge(label: string, panels?: PanelsConfig): PanelEdge {
  if (label === "bar" || label === "preview") return panels?.bar?.edge ?? "left";
  if (label === "notch") return panels?.notch?.edge ?? "top";
  if (label === "launcher") return panels?.launcher?.edge ?? "bottom";
  if (label === "overlay") return panels?.overlay?.edge ?? "right";
  if (label === "overview") return "left";
  return "bottom";
}

function applyPanelEdge(label: string, panels?: PanelsConfig) {
  const edge = resolvePanelEdge(label, panels);
  currentPanelEdge = edge;
  document.documentElement.dataset.edge = edge;
  for (const fn of edgeListeners) fn(edge);
}

/** Reactive screen edge for the current window, synced from config.panels by useTheme. */
export function usePanelEdge(): PanelEdge {
  const [edge, setEdge] = useState<PanelEdge>(currentPanelEdge);
  useEffect(() => {
    setEdge(currentPanelEdge);
    edgeListeners.add(setEdge);
    return () => {
      edgeListeners.delete(setEdge);
    };
  }, []);
  return edge;
}

/** Reactive UI style ("material" | "fluent") kept in sync across all windows by useTheme. */
export function useUiStyle(): UiStyle {
  const [style, setStyle] = useState<UiStyle>(currentUiStyle);
  useEffect(() => {
    setStyle(currentUiStyle);
    styleListeners.add(setStyle);
    return () => {
      styleListeners.delete(setStyle);
    };
  }, []);
  return style;
}

function applyTokens(tokens: Tokens) {
  const root = document.documentElement.style;
  for (const [name, value] of Object.entries(tokens)) root.setProperty(name, value);
}

function applyMicaImage(path: string | null) {
  const root = document.documentElement.style;
  if (path) {
    const url = convertFileSrc(path);
    root.setProperty("--mica-image", `url("${url.replace(/"/g, '\\"')}")`);
  } else {
    root.removeProperty("--mica-image");
  }
}

const MICA_SUBPANELS = ".bar, .wmov, .notch, .lpanel, .swpanel, .overlay, .pvcard";

let micaOffsetRaf = 0;

function syncMicaOffsetsNow() {
  if (!currentMica) return;
  for (const el of document.querySelectorAll<HTMLElement>(MICA_SUBPANELS)) {
    const x = el.offsetLeft;
    let y = el.offsetTop;
    if (el.classList.contains("overlay")) {
      y -= el.offsetHeight / 2;
    }
    el.style.setProperty("--mica-el-x", `${x}px`);
    el.style.setProperty("--mica-el-y", `${y}px`);
    el.style.setProperty("--mica-el-w", `${el.offsetWidth}px`);
    el.style.setProperty("--mica-el-h", `${el.offsetHeight}px`);
  }
}

function scheduleMicaOffsets() {
  if (!currentMica || micaOffsetRaf !== 0) return;
  micaOffsetRaf = window.requestAnimationFrame(() => {
    micaOffsetRaf = 0;
    syncMicaOffsetsNow();
  });
}

type CachedMetrics = {
  monX: number;
  monY: number;
  monW: number;
  monH: number;
  scaleFactor: number;
  winX: number;
  winY: number;
};

let cachedMetrics: CachedMetrics | null = null;

function applyWindowMicaFromCache(uiScale: number) {
  if (!currentMica || !cachedMetrics) return;
  const factor = (cachedMetrics.scaleFactor || 1) * (uiScale || 1);
  const root = document.documentElement.style;
  root.setProperty("--mica-screen-w", `${cachedMetrics.monW / factor}px`);
  root.setProperty("--mica-screen-h", `${cachedMetrics.monH / factor}px`);
  root.setProperty("--mica-win-x", `${(cachedMetrics.winX - cachedMetrics.monX) / factor}px`);
  root.setProperty("--mica-win-y", `${(cachedMetrics.winY - cachedMetrics.monY) / factor}px`);
  scheduleMicaOffsets();
}

async function syncWindowMica(uiScale: number, forceQuery = true) {
  if (!currentMica) return;
  if (!forceQuery && cachedMetrics) {
    applyWindowMicaFromCache(uiScale);
    return;
  }
  try {
    const win = getCurrentWindow();
    const [mon, pos] = await Promise.all([primaryMonitor(), win.outerPosition()]);
    if (!mon) return;
    cachedMetrics = {
      monX: mon.position.x,
      monY: mon.position.y,
      monW: mon.size.width,
      monH: mon.size.height,
      scaleFactor: mon.scaleFactor || 1,
      winX: pos.x,
      winY: pos.y,
    };
    applyWindowMicaFromCache(uiScale);
  } catch {
    /* ignore transient window metric errors */
  }
}

/**
 * Pick a representative seed color from RGBA pixel bytes without letting neutral pixels
 * inflate CAM16's ~209.5 deg hue or falling back to Google Blue.
 */
function extractSeedFromBytes(imageBytes: Uint8ClampedArray): number {
  const pixels: number[] = [];
  for (let i = 0; i < imageBytes.length; i += 4) {
    if (imageBytes[i + 3] < 255) continue;
    pixels.push(argbFromRgb(imageBytes[i], imageBytes[i + 1], imageBytes[i + 2]));
  }
  if (pixels.length === 0) return FALLBACK_SEED;

  const clusters = QuantizerCelebi.quantize(pixels, 128);
  let totalPop = 0;
  const candidates: { argb: number; pop: number; spread: number; hct: Hct }[] = [];
  for (const [argb, pop] of clusters.entries()) {
    totalPop += pop;
    const r = redFromArgb(argb);
    const g = greenFromArgb(argb);
    const b = blueFromArgb(argb);
    const spread = Math.max(r, g, b) - Math.min(r, g, b);
    const hct = Hct.fromInt(argb);
    // Use sRGB channel spread instead of HCT chroma so CAM16's D65 blue offset doesn't bias against warm tones.
    if (spread >= 6 && hct.tone >= 5 && hct.tone <= 95) {
      candidates.push({ argb, pop, spread, hct });
    }
  }

  const candPop = candidates.reduce((s, it) => s + it.pop, 0);
  if (totalPop === 0 || candidates.length === 0 || candPop < totalPop * 0.005) {
    return FALLBACK_SEED;
  }

  // Accumulate hue weights from chromatic/tinted candidates only (excluding neutral pixels).
  const hueWeight = new Array<number>(360).fill(0);
  let totalWeight = 0;
  for (const it of candidates) {
    const h = sanitizeDegreesInt(Math.floor(it.hct.hue));
    const w = it.pop * it.spread;
    hueWeight[h] += w;
    totalWeight += w;
  }
  if (totalWeight === 0) return FALLBACK_SEED;

  const excited = new Array<number>(360).fill(0);
  for (let h = 0; h < 360; h++) {
    const prop = hueWeight[h] / totalWeight;
    for (let i = h - 14; i < h + 16; i++) {
      excited[sanitizeDegreesInt(i)] += prop;
    }
  }

  let best: { argb: number; score: number } | null = null;
  for (const it of candidates) {
    const h = sanitizeDegreesInt(Math.round(it.hct.hue));
    const prop = excited[h];
    if (prop <= 0.01 && candidates.length > 1) continue;
    const propScore = prop * 100 * 0.7;
    const cw = it.hct.chroma < 48 ? 0.2 : 0.3;
    const chromaScore = (it.hct.chroma - 48) * cw;
    const score = propScore + chromaScore;
    if (!best || score > best.score) {
      best = { argb: it.argb, score };
    }
  }
  return best ? best.argb : candidates[0].argb;
}

async function seedFromWallpaper(): Promise<number | null> {
  try {
    const px = await invoke<{ width: number; height: number; data: string } | null>(
      "wallpaper_pixels",
    );
    if (!px) return null;
    const bin = atob(px.data);
    const bytes = new Uint8ClampedArray(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return extractSeedFromBytes(bytes);
  } catch {
    return null;
  }
}

/**
 * All windows follow the theme calculated by a single source window (the launcher).
 * The wallpaper is checked when a panel opens (inexpensive: only its stamp is compared),
 * not periodically. Wallpaper colors are cached by stamp, so moving a slider or
 * changing a custom color does not reread the image.
 */
export function useTheme(isSource: boolean) {
  useEffect(() => {
    let cancelled = false;
    let lastSig = "";
    let lastMicaStamp: string | null | undefined = undefined;
    let cfgSig = "";
    let lastCheck = 0;
    let currentScale = 0.8;
    let hasTheme = false;
    let hasMica = false;
    let retryTimer: number | null = null;
    let checkTimer: number | null = null;
    let windowMoveRaf = 0;
    let seedCache: { stamp: string | null; seed: number } | null = null;
    const unlistenFns: (() => void)[] = [];
    const win = getCurrentWindow();
    const label = win.label;

    const addListener = async (p: Promise<() => void>): Promise<void> => {
      try {
        const fn = await p;
        if (cancelled) {
          fn();
        } else {
          unlistenFns.push(fn);
        }
      } catch {
        /* ignore listener registration failure */
      }
    };

    const syncCachedTheme = () => {
      if (!hasTheme) {
        invoke<string | null>("get_theme")
          .then((json) => {
            if (json && !cancelled) {
              hasTheme = true;
              applyTokens(JSON.parse(json));
            }
          })
          .catch(() => {});
      }
      if (currentMica && !hasMica) {
        invoke<string | null>("get_mica")
          .then((path) => {
            if (path && !cancelled) {
              hasMica = true;
              applyMicaImage(path);
            }
          })
          .catch(() => {});
      }
    };

    const ro = new ResizeObserver(() => scheduleMicaOffsets());
    const syncMicaObservers = (enabled: boolean) => {
      ro.disconnect();
      if (!enabled) return;
      for (const el of document.querySelectorAll<HTMLElement>(MICA_SUBPANELS)) {
        ro.observe(el);
      }
      scheduleMicaOffsets();
    };

    /** force: recalculate even if nothing changed. rewall: reread the wallpaper image. */
    const check = async (force: boolean, rewall = false) => {
      if (!force && Date.now() - lastCheck < 3000) return;
      lastCheck = Date.now();
      try {
        const cfg = await invoke<Config>("get_config");
        cfgSig = JSON.stringify([cfg.colors, cfg.bg_tint]);
        const needsWallpaper = Object.values(cfg.colors).some((s) => s.mode === "auto");
        const needsMica = cfg.mica !== false;
        const stamp =
          needsWallpaper || needsMica ? await invoke<string | null>("wallpaper_stamp") : null;

        if (!needsMica) {
          lastMicaStamp = undefined;
        } else if (rewall || stamp !== lastMicaStamp) {
          lastMicaStamp = stamp;
          invoke<string | null>("mica_backdrop")
            .then((p) => {
              if (!cancelled) {
                hasMica = Boolean(p);
                applyMicaImage(p);
              }
            })
            .catch(() => {});
        }

        const sig = `${cfgSig}|${stamp}`;
        if (!force && sig === lastSig) return;

        let seed = FALLBACK_SEED;
        let readFailed = false;
        if (needsWallpaper && stamp !== null) {
          if (!rewall && seedCache && seedCache.stamp === stamp) {
            seed = seedCache.seed;
          } else {
            const extracted = await seedFromWallpaper();
            if (extracted === null) {
              readFailed = true;
            } else {
              seed = extracted;
              seedCache = { stamp, seed };
            }
          }
        }

        const tokens = buildTokens(cfg, seed);
        if (cancelled) return;
        if (!readFailed) {
          lastSig = sig;
        } else if (retryTimer === null) {
          // Wallpaper file may still be settling right after boot; retry once shortly.
          retryTimer = window.setTimeout(() => {
            retryTimer = null;
            if (!cancelled) void check(true, true);
          }, 2000);
        }
        hasTheme = true;
        applyTokens(tokens);
        await invoke("set_theme", { json: JSON.stringify(tokens) });
      } catch {
        /* try again on the next trigger */
      }
    };

    // Register all event listeners BEFORE fetching initial cached state so no startup
    // broadcast from the source window can be missed between get_theme() and listen().
    const listenerPromises: Promise<void>[] = [
      addListener(
        listen<string>("theme-changed", (e) => {
          if (cancelled) return;
          hasTheme = true;
          applyTokens(JSON.parse(e.payload));
        }),
      ),
      addListener(
        listen<string | null>("mica-changed", (e) => {
          if (cancelled) return;
          hasMica = Boolean(e.payload);
          applyMicaImage(e.payload);
        }),
      ),
      addListener(
        listen<Config>("config-changed", (e) => {
          if (cancelled) return;
          currentScale = e.payload.ui_scale || 0.8;
          const nextStyle = normalizeUiStyle(e.payload.ui_style);
          const nextMica = e.payload.mica !== false;
          applyUiStyle(nextStyle);
          applyMicaEnabled(nextMica);
          syncMicaObservers(nextMica);
          applyScreenBorder(Boolean(e.payload.screen_border), e.payload.screen_border_size ?? 10);
          applyPanelEdge(label, e.payload.panels);
          if (nextMica) {
            void syncWindowMica(currentScale);
            if (isSource) {
              invoke<string | null>("mica_backdrop")
                .then((p) => {
                  if (!cancelled) {
                    hasMica = Boolean(p);
                    applyMicaImage(p);
                  }
                })
                .catch(() => {});
            } else if (!hasMica) {
              syncCachedTheme();
            }
          } else {
            hasMica = false;
            lastMicaStamp = undefined;
          }
          if (isSource && JSON.stringify([e.payload.colors, e.payload.bg_tint]) !== cfgSig) {
            void check(true);
          }
        }),
      ),
      addListener(
        win.onMoved(() => {
          if (!currentMica || windowMoveRaf !== 0) return;
          windowMoveRaf = window.requestAnimationFrame(() => {
            windowMoveRaf = 0;
            void syncWindowMica(currentScale);
          });
        }),
      ),
      addListener(
        win.onResized(() => {
          if (!currentMica || windowMoveRaf !== 0) return;
          windowMoveRaf = window.requestAnimationFrame(() => {
            windowMoveRaf = 0;
            void syncWindowMica(currentScale);
          });
        }),
      ),
      addListener(
        listen<boolean>(`edge-visible:${label}`, (e) => {
          if (!e.payload || cancelled) return;
          if (currentMica) {
            if (cachedMetrics) {
              applyWindowMicaFromCache(currentScale);
            } else {
              void syncWindowMica(currentScale);
            }
          }
          syncCachedTheme();
        }),
      ),
    ];

    if (label === "preview") {
      listenerPromises.push(
        addListener(
          listen("preview-data", () => {
            if (!cancelled && currentMica) scheduleMicaOffsets();
          }),
        ),
      );
    }

    if (label === "activities") {
      listenerPromises.push(
        addListener(
          listen("activities-open", () => {
            if (cancelled) return;
            if (currentMica) {
              if (cachedMetrics) {
                applyWindowMicaFromCache(currentScale);
              } else {
                void syncWindowMica(currentScale);
              }
            }
            syncCachedTheme();
          }),
        ),
      );
    }

    if (label === "border") {
      listenerPromises.push(
        addListener(
          listen<boolean>("border-visible", (e) => {
            if (!e.payload || cancelled) return;
            if (currentMica) void syncWindowMica(currentScale);
            syncCachedTheme();
          }),
        ),
        addListener(
          listen("border-spread", () => {
            if (cancelled) return;
            if (currentMica) void syncWindowMica(currentScale);
            syncCachedTheme();
          }),
        ),
      );
    }

    if (isSource) {
      for (const panel of ["bar", "notch", "launcher", "overlay"]) {
        listenerPromises.push(
          addListener(
            listen<boolean>(`edge-visible:${panel}`, (e) => {
              if (!e.payload || cancelled) return;
              if (checkTimer !== null) window.clearTimeout(checkTimer);
              checkTimer = window.setTimeout(() => {
                checkTimer = null;
                if (!cancelled) void check(false);
              }, 500);
            }),
          ),
        );
      }
      listenerPromises.push(
        addListener(
          listen("theme-refresh", () => {
            if (!cancelled) void check(true, true);
          }),
        ),
      );
    }

    void Promise.all(listenerPromises).then(() => {
      if (cancelled) return;

      // Listeners are now active: pull any theme/mica already broadcast before we mounted.
      invoke<string | null>("get_theme")
        .then((json) => {
          if (json && !cancelled) {
            hasTheme = true;
            applyTokens(JSON.parse(json));
          }
        })
        .catch(() => {});
      invoke<string | null>("get_mica")
        .then((path) => {
          if (path && !cancelled) {
            hasMica = true;
            applyMicaImage(path);
          }
        })
        .catch(() => {});
      invoke<Config>("get_config")
        .then((cfg) => {
          if (cancelled) return;
          currentScale = cfg.ui_scale || 0.8;
          const nextMica = cfg.mica !== false;
          applyUiStyle(normalizeUiStyle(cfg.ui_style));
          applyMicaEnabled(nextMica);
          syncMicaObservers(nextMica);
          applyScreenBorder(Boolean(cfg.screen_border), cfg.screen_border_size ?? 10);
          applyPanelEdge(label, cfg.panels);
          if (nextMica) {
            void syncWindowMica(currentScale);
            if (!isSource && !hasMica) {
              syncCachedTheme();
            }
          }
        })
        .catch(() => {});

      if (isSource) {
        void check(true);
      }
    });

    return () => {
      cancelled = true;
      if (retryTimer !== null) {
        window.clearTimeout(retryTimer);
        retryTimer = null;
      }
      if (checkTimer !== null) {
        window.clearTimeout(checkTimer);
        checkTimer = null;
      }
      if (windowMoveRaf !== 0) {
        window.cancelAnimationFrame(windowMoveRaf);
        windowMoveRaf = 0;
      }
      if (micaOffsetRaf !== 0) {
        window.cancelAnimationFrame(micaOffsetRaf);
        micaOffsetRaf = 0;
      }
      ro.disconnect();
      for (const fn of unlistenFns) fn();
    };
  }, [isSource]);
}
