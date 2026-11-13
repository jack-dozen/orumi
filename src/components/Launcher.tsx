import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useConfig } from "../useConfig";
import { useEdgeVisible } from "../useEdgeVisible";
import Icon from "./Icon";
import WallpaperPicker from "./WallpaperPicker";
import LauncherSettings from "./LauncherSettings";
import AppIcon from "./AppIcon";

type AppEntry = { name: string; id: string };

/** One result row, whether an app or an action. */
type Item = {
  key: string;
  name: string;
  hint?: string;
  glyph: string;
  /** App icon address ("aumid:..." or "path:..."); when present, it replaces the glyph. */
  icon?: string;
  /** true = glyph is a Material Symbols icon name, not a letter. */
  symbol?: boolean;
  dangerous?: boolean;
  run: () => void;
};

type ActionDef = {
  id: string;
  name: string;
  hint: string;
  glyph: string;
  dangerous?: boolean;
};

const ACTIONS: ActionDef[] = [
  { id: "lock", name: "Lock", hint: "Kunci layar", glyph: "lock" },
  { id: "sleep", name: "Sleep", hint: "Tidurkan PC", glyph: "bedtime" },
  { id: "hibernate", name: "Hibernate", hint: "Simpan sesi ke disk lalu mati", glyph: "schedule" },
  { id: "display_off", name: "Display off", hint: "Matikan layar saja", glyph: "desktop_access_disabled" },
  { id: "logout", name: "Sign out", hint: "Keluar dari akun", glyph: "logout", dangerous: true },
  { id: "restart", name: "Restart", hint: "Mulai ulang PC", glyph: "restart_alt", dangerous: true },
  { id: "shutdown", name: "Shutdown", hint: "Matikan PC", glyph: "power_settings_new", dangerous: true },
  { id: "reload_apps", name: "Reload apps", hint: "Muat ulang daftar aplikasi", glyph: "refresh" },
  { id: "switch_window", name: "Switch window", hint: "Pindah window terbuka (Alt+Tab)", glyph: "apps" },
  { id: "settings", name: "Settings", hint: "Pengaturan ORUMI", glyph: "tune" },
  { id: "wallpaper", name: "Wallpaper", hint: "Ganti wallpaper desktop", glyph: "wallpaper" },
];

const MAX_SHOWN = 8;
/** Panel width when displaying the list (design units). */
const LIST_WIDTH = 640;

/** Name-to-query match score. 0 = no match. */
function score(name: string, q: string): number {
  const n = name.toLowerCase();
  if (n === q) return 100;
  if (n.startsWith(q)) return 80;
  if (n.split(/[\s\-_.]+/).some((w) => w.startsWith(q))) return 60;
  if (n.includes(q)) return 40;
  let i = 0;
  for (const c of n) {
    if (c === q[i]) i++;
    if (i === q.length) return 20;
  }
  return 0;
}

const close = () => invoke("set_panel", { label: "launcher", open: false });

export default function Launcher() {
  const visible = useEdgeVisible("launcher");
  const cfg = useConfig();
  const [apps, setApps] = useState<AppEntry[]>([]);
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [armed, setArmed] = useState<string | null>(null);
  const [view, setView] = useState<"list" | "wallpaper" | "settings">("list");
  /** A view requested from outside (tray menu) before the launcher finished opening. */
  const pendingView = useRef<"settings" | null>(null);
  const visibleRef = useRef(false);
  visibleRef.current = visible;
  const [pickerWidth, setPickerWidth] = useState<number | null>(null);
  const [settingsWidth, setSettingsWidth] = useState<number | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const openedAt = useRef(0);

  // The panel fits its contents: height is measured from content; width is set by the view.
  const innerRef = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const measure = () => setHeight(el.offsetHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Load the app list once when the app starts (takes 1-2 seconds).
  useEffect(() => {
    invoke<AppEntry[]>("list_apps", { refresh: false })
      .then(setApps)
      .catch(() => {});
  }, []);

  // Reset and focus the search box each time the launcher opens.
  useEffect(() => {
    if (!visible) return;
    openedAt.current = Date.now();
    setQuery("");
    setIndex(0);
    setView(pendingView.current ?? "list");
    pendingView.current = null;
    const id = setTimeout(() => inputRef.current?.focus(), 60);
    return () => clearTimeout(id);
  }, [visible]);

  // The tray menu asks for the settings view; it may arrive before or after the panel is shown.
  useEffect(() => {
    const unlisten = listen<string>("launcher-view", (e) => {
      if (e.payload !== "settings") return;
      if (!visibleRef.current) pendingView.current = "settings"; // applied when it opens
      setView("settings");
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Returning from the wallpaper carousel remounts the search box, so focus it again.
  useEffect(() => {
    if (view !== "list") return;
    const id = setTimeout(() => inputRef.current?.focus(), 30);
    return () => clearTimeout(id);
  }, [view]);

  // Outside click on the launcher (window loses focus) = close.
  useEffect(() => {
    const onBlur = () => {
      // The native color dialog makes the window lose focus; keep the launcher open during it.
      const el = document.activeElement as HTMLInputElement | null;
      if (el?.tagName === "INPUT" && el.type === "color") return;
      if (Date.now() - openedAt.current > 400) close();
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Cancel confirmation if the selection or text changes.
  useEffect(() => {
    setArmed(null);
  }, [query, index, visible]);

  const mode = query.startsWith(">") ? "actions" : "apps";

  const results: Item[] = useMemo(() => {
    if (mode === "actions") {
      const q = query.slice(1).trim().toLowerCase();
      const list = q
        ? ACTIONS.map((a) => ({ a, s: Math.max(score(a.name, q), score(a.id, q)) }))
            .filter((x) => x.s > 0)
            .sort((x, y) => y.s - x.s)
            .map((x) => x.a)
        : ACTIONS;
      return list.slice(0, MAX_SHOWN).map((a) => ({
        key: a.id,
        name: a.name,
        hint: a.hint,
        glyph: a.glyph,
        symbol: true,
        dangerous: a.dangerous,
        run: () => {
          if (a.id === "reload_apps") {
            invoke<AppEntry[]>("list_apps", { refresh: true })
              .then(setApps)
              .catch(() => {});
          } else if (a.id === "switch_window") {
            invoke("switcher_open").catch(() => {});
          } else if (a.id === "settings") {
            setView("settings");
          } else {
            invoke("run_action", { id: a.id }).catch(() => {});
          }
        },
      }));
    }

    const q = query.trim().toLowerCase();
    const list = q
      ? apps
          .map((a) => ({ a, s: score(a.name, q) }))
          .filter((x) => x.s > 0)
          .sort((x, y) => y.s - x.s || x.a.name.localeCompare(y.a.name))
          .map((x) => x.a)
      : apps;
    return list.slice(0, MAX_SHOWN).map((a) => ({
      key: a.id,
      name: a.name,
      glyph: a.name.charAt(0).toUpperCase(),
      icon: `aumid:${a.id}`,
      run: () => {
        invoke("launch_app", { id: a.id }).catch(() => {});
      },
    }));
  }, [apps, query, mode]);

  /** Run an item; dangerous actions require two confirmations. */
  const activate = (item?: Item) => {
    if (!item) return;
    if (item.dangerous && armed !== item.key) {
      setArmed(item.key);
      return;
    }
    // Wallpaper: the built-in picker opens a carousel inside the launcher;
    // "External command" mode runs the command from config, then closes the launcher.
    if (mode === "actions" && item.key === "wallpaper") {
      if (cfg?.wallpaper_switcher === "external") {
        close();
        invoke("run_wallpaper_command").catch(() => {});
      } else {
        setView("wallpaper");
      }
      return;
    }
    // Settings: ORUMI's own settings open inside the launcher.
    if (mode === "actions" && item.key === "settings") {
      setView("settings");
      return;
    }
    close();
    item.run();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowUp") {
      // The list is displayed in reverse: moving up selects less relevant results.
      e.preventDefault();
      setIndex((i) => Math.min(i + 1, results.length - 1));
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      activate(results[index]);
    } else if (e.key === "Escape") {
      close();
    }
  };

  const emptyText =
    mode === "actions"
      ? "Tidak ada aksi yang cocok"
      : apps.length === 0
        ? "Memuat daftar aplikasi…"
        : "Tidak ada hasil";

  const width =
    view === "wallpaper" && pickerWidth
      ? pickerWidth
      : view === "settings"
        ? (settingsWidth ?? 780)
        : LIST_WIDTH;

  return (
    <div
      className="lroot"
      onPointerDown={(e) => {
        // Clicking the transparent area outside the panel counts as an outside click: close.
        if (e.target === e.currentTarget) close();
      }}
    >
      <div
        className={`lpanel ${visible ? "lpanel--visible" : ""}`}
        style={{ width, height: height ?? undefined }}
      >
        <div className="lpanel__inner" ref={innerRef}>
          {view === "settings" ? (
            <LauncherSettings onBack={() => setView("list")} onWidth={setSettingsWidth} />
          ) : view === "wallpaper" ? (
            <WallpaperPicker
              onBack={() => setView("list")}
              onDone={close}
              onWidth={setPickerWidth}
            />
          ) : (
            <>
              <div className="launcher__list">
                {results.length === 0 && <div className="launcher__empty">{emptyText}</div>}
                {results.map((item, i) => (
                  <div
                    key={item.key}
                    className={`litem ${i === index ? "litem--active" : ""}`}
                    onMouseMove={() => setIndex(i)}
                    onClick={() => activate(item)}
                  >
                    {item.icon ? (
                      <AppIcon target={item.icon} fallback={item.glyph} size={36} />
                    ) : (
                      <div
                        className={`litem__tile ${item.dangerous ? "litem__tile--danger" : ""}`}
                      >
                        {item.symbol ? <Icon name={item.glyph} size={22} /> : item.glyph}
                      </div>
                    )}
                    <div className="litem__text">
                      <span className="litem__name">{item.name}</span>
                      {(item.hint || armed === item.key) && (
                        <span
                          className={`litem__hint ${armed === item.key ? "litem__hint--armed" : ""}`}
                        >
                          {armed === item.key ? "Tekan Enter lagi untuk konfirmasi" : item.hint}
                        </span>
                      )}
                    </div>
                  </div>
                ))}
              </div>

              <input
                ref={inputRef}
                className="lsearch"
                placeholder="Cari aplikasi…  (> aksi sistem)"
                value={query}
                onChange={(e) => {
                  setQuery(e.target.value);
                  setIndex(0);
                }}
                onKeyDown={onKeyDown}
                spellCheck={false}
              />
            </>
          )}
        </div>
      </div>
    </div>
  );
}
