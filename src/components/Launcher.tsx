import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useConfig } from "../useConfig";
import { useEdgeVisible } from "../useEdgeVisible";
import { usePanelEdge } from "../theme";
import Icon from "./Icon";
import WallpaperPicker from "./WallpaperPicker";
import LauncherSettings from "./LauncherSettings";
import AppIcon from "./AppIcon";
import WmCheatsheet from "./WmCheatsheet";

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
  { id: "wallpaper", name: "Wallpaper", hint: "Change desktop wallpaper", glyph: "wallpaper" },
  { id: "switch_window", name: "Switch window", hint: "Switch open windows (Alt+Tab)", glyph: "apps" },
  { id: "cheatsheet", name: "WM shortcuts", hint: "Window manager keyboard shortcuts (Win + /)", glyph: "keyboard" },
  { id: "settings", name: "Settings", hint: "ORUMI settings", glyph: "tune" },
  { id: "lock", name: "Lock", hint: "Lock the screen", glyph: "lock" },
  { id: "sleep", name: "Sleep", hint: "Put PC to sleep", glyph: "bedtime" },
  { id: "hibernate", name: "Hibernate", hint: "Save session to disk and power off", glyph: "schedule" },
  { id: "restart", name: "Restart", hint: "Restart the PC", glyph: "restart_alt", dangerous: true },
  { id: "shutdown", name: "Shutdown", hint: "Shut down the PC", glyph: "power_settings_new", dangerous: true },
  { id: "display_off", name: "Display off", hint: "Turn off the display only", glyph: "desktop_access_disabled" },
  { id: "logout", name: "Sign out", hint: "Sign out of your account", glyph: "logout", dangerous: true },
  { id: "reload_apps", name: "Reload apps", hint: "Reload the application list", glyph: "refresh" },
];

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
  const edge = usePanelEdge();
  const isTop = edge === "top";
  const [apps, setApps] = useState<AppEntry[]>([]);
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<"apps" | "actions">("apps");
  const [compact, setCompact] = useState(false);
  const [index, setIndex] = useState(0);
  const [armed, setArmed] = useState<string | null>(null);
  const [view, setView] = useState<"list" | "wallpaper" | "settings" | "cheatsheet">("list");
  const [listMounted, setListMounted] = useState(false);
  const [renderLimit, setRenderLimit] = useState(36);
  /** A view requested from outside (tray menu / hotkey) before the launcher finished opening. */
  const pendingView = useRef<"settings" | "cheatsheet" | null>(null);
  const [csFromLauncher, setCsFromLauncher] = useState(false);
  const visibleRef = useRef(false);
  visibleRef.current = visible;
  const openRef = useRef(false);
  const viewRef = useRef(view);
  viewRef.current = view;
  const [pickerWidth, setPickerWidth] = useState<number | null>(null);
  const [settingsWidth, setSettingsWidth] = useState<number | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const scrollOnIndex = useRef(false);
  const openedAt = useRef(0);
  const activatedAt = useRef(0);
  const focusOnOpen = useRef(true);

  const activateLauncher = useCallback(() => {
    focusOnOpen.current = true;
    activatedAt.current = Date.now();
    invoke("panel_activation", { label: "launcher", enable: true }).catch(() => {});
    window.setTimeout(() => inputRef.current?.focus({ preventScroll: true }), 20);
  }, []);

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

  // Track whether this open should immediately focus the search box (hotkey/tray) or wait for click (hover).
  useEffect(() => {
    const unlisten = listen<boolean>("launcher-focus", (e) => {
      focusOnOpen.current = e.payload;
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Prepare clean initial state as soon as Rust requests open (2 frames before `visible` flips)
  // so height/compact layout settles off-screen before the CSS slide-up begins.
  useEffect(() => {
    const unlisten = listen<boolean>("edge-visible:launcher", (e) => {
      openRef.current = e.payload;
      if (!e.payload) return;
      setListMounted(true);
      setRenderLimit(36);
      openedAt.current = Date.now();
      setQuery("");
      setMode("apps");
      setIndex(0);
      const nextView = pendingView.current ?? "list";
      pendingView.current = null;
      if (nextView === "cheatsheet") setCsFromLauncher(false);
      viewRef.current = nextView;
      setView(nextView);
      if (focusOnOpen.current && nextView === "list") {
        inputRef.current?.focus({ preventScroll: true });
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Suppress WebView2's Alt menu-bar beep/mnemonic handling and capture keystrokes typed while
  // Alt from an Alt+Space hotkey is still held or before DOM focus settles on the input.
  useEffect(() => {
    const applyChar = (ch: string) => {
      if (ch === ">") {
        setMode((m) => (m === "apps" ? "actions" : "apps"));
        setIndex(0);
      } else {
        setQuery((q) => q + ch);
        setIndex(0);
      }
    };

    const onKeyDownCapture = (e: KeyboardEvent) => {
      if (!openRef.current || viewRef.current !== "list") return;
      if (e.key === "Alt") {
        e.preventDefault();
        return;
      }
      if (e.altKey && !e.ctrlKey && !e.metaKey) {
        if ((e.code === "Space" || e.key === " ") && Date.now() - openedAt.current < 400) {
          e.preventDefault();
          e.stopPropagation();
          return;
        }
        if (e.key.length === 1 && !e.isComposing) {
          e.preventDefault();
          e.stopPropagation();
          inputRef.current?.focus({ preventScroll: true });
          applyChar(e.key);
        }
        return;
      }
      if (
        !e.altKey &&
        !e.ctrlKey &&
        !e.metaKey &&
        e.key.length === 1 &&
        !e.isComposing &&
        document.activeElement !== inputRef.current
      ) {
        e.preventDefault();
        e.stopPropagation();
        inputRef.current?.focus({ preventScroll: true });
        applyChar(e.key);
      }
    };

    const onKeyUpCapture = (e: KeyboardEvent) => {
      if (!openRef.current || viewRef.current !== "list") return;
      if (e.key === "Alt") {
        e.preventDefault();
      }
    };

    window.addEventListener("keydown", onKeyDownCapture, true);
    window.addEventListener("keyup", onKeyUpCapture, true);
    return () => {
      window.removeEventListener("keydown", onKeyDownCapture, true);
      window.removeEventListener("keyup", onKeyUpCapture, true);
    };
  }, []);

  // Focus the search input as soon as the panel becomes visible (if opened with focus),
  // and reset back to the default view after the close slide finishes.
  useEffect(() => {
    if (!visible) {
      const id = window.setTimeout(() => {
        setListMounted(false);
        setView("list");
        setPickerWidth(null);
        setSettingsWidth(null);
      }, 360);
      return () => window.clearTimeout(id);
    }
    setListMounted(true);
    if (!focusOnOpen.current) {
      inputRef.current?.blur();
      return;
    }
    inputRef.current?.focus({ preventScroll: true });
  }, [visible]);

  // Keep Rust's launcher window size override synced when switching to expanded views.
  const prevView = useRef(view);
  useEffect(() => {
    if (!visible) {
      prevView.current = view;
      return;
    }
    if (view !== "list" || prevView.current !== "list") {
      invoke("launcher_view", { view }).catch(() => {});
    }
    prevView.current = view;
  }, [visible, view]);

  // Tray menu / WM hotkey can request settings or cheatsheet before or after the panel opens.
  useEffect(() => {
    const unlisten = listen<string>("launcher-view", (e) => {
      if (e.payload !== "settings" && e.payload !== "cheatsheet") return;
      if (!visibleRef.current) pendingView.current = e.payload;
      if (e.payload === "cheatsheet") setCsFromLauncher(false);
      setView(e.payload);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Compact mode when the active window behind the launcher is maximized/fullscreen.
  useEffect(() => {
    const unlisten = listen<boolean>("launcher-compact", (e) => setCompact(e.payload));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Returning from the wallpaper carousel remounts the search box, so focus it again if active.
  useEffect(() => {
    if (!visible || view !== "list" || !document.hasFocus()) return;
    const id = setTimeout(() => inputRef.current?.focus({ preventScroll: true }), 30);
    return () => clearTimeout(id);
  }, [visible, view]);

  // Outside click on the launcher (window loses focus) = close.
  useEffect(() => {
    if (!visible) return;
    const onBlur = () => {
      // The native color dialog makes the window lose focus; keep the launcher open during it.
      const el = document.activeElement as HTMLInputElement | null;
      if (el?.tagName === "INPUT" && el.type === "color") return;
      const since = Date.now() - Math.max(openedAt.current, activatedAt.current);
      if (since > 250) close();
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [visible]);

  // Cancel confirmation if the selection, mode, or text changes.
  useEffect(() => {
    setArmed(null);
  }, [query, index, visible, mode]);

  const results: Item[] = useMemo(() => {
    if (mode === "actions") {
      const q = query.trim().toLowerCase();
      const list = q
        ? ACTIONS.map((a) => ({ a, s: Math.max(score(a.name, q), score(a.id, q)) }))
            .filter((x) => x.s > 0)
            .sort((x, y) => y.s - x.s)
            .map((x) => x.a)
        : ACTIONS;
      return list.map((a) => ({
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
          .sort(
            (x, y) =>
              y.s - x.s ||
              x.a.name.localeCompare(y.a.name, undefined, { sensitivity: "base" }),
          )
          .map((x) => x.a)
      : [...apps].sort((a, b) =>
          a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
        );
    return list.map((a) => ({
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
    // Settings & WM shortcuts: open inside the launcher.
    if (mode === "actions" && item.key === "settings") {
      setView("settings");
      return;
    }
    if (mode === "actions" && item.key === "cheatsheet") {
      setCsFromLauncher(true);
      setView("cheatsheet");
      return;
    }
    close();
    item.run();
  };

  const showList = !compact || mode === "actions" || query.trim().length > 0;
  // Browsing all apps shows A-Z top-to-bottom; searching & actions keep best match closest to the search box.
  // When anchored at the top edge, the search bar sits at the top, so normal order already keeps the best match closest.
  const reversed = !isTop && (mode === "actions" || query.trim().length > 0);
  const effectiveLimit = Math.max(renderLimit, index + 12);
  const shownResults = useMemo(
    () => (listMounted ? results.slice(0, effectiveLimit) : []),
    [listMounted, results, effectiveLimit],
  );

  // Reset scroll position and render window when opening or switching query/mode.
  useEffect(() => {
    setRenderLimit(36);
    if (listRef.current) listRef.current.scrollTop = 0;
  }, [visible, query, mode]);

  const onListScroll = () => {
    const el = listRef.current;
    if (!el || effectiveLimit >= results.length) return;
    // In column-reverse, scrollTop becomes negative as the user scrolls toward older items at the top.
    const nearEnd = reversed
      ? Math.abs(el.scrollTop) + el.clientHeight >= el.scrollHeight - 220
      : el.scrollTop + el.clientHeight >= el.scrollHeight - 220;
    if (nearEnd) {
      setRenderLimit((n) => Math.max(n, effectiveLimit) + 40);
    }
  };

  // Keep keyboard-selected item visible inside the scrollable list.
  useEffect(() => {
    if (!scrollOnIndex.current) return;
    scrollOnIndex.current = false;
    const active = listRef.current?.querySelector<HTMLElement>(".litem--active");
    active?.scrollIntoView({ block: "nearest" });
  }, [index]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === ">") {
      // Consume '>' so it toggles between apps and system actions without entering the query.
      e.preventDefault();
      setMode((m) => (m === "apps" ? "actions" : "apps"));
      setIndex(0);
    } else if (e.key === "Backspace" && query === "" && mode === "actions") {
      e.preventDefault();
      setMode("apps");
      setIndex(0);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (showList && results.length > 0) {
        scrollOnIndex.current = true;
        setIndex((i) => (reversed ? Math.min(i + 1, results.length - 1) : Math.max(i - 1, 0)));
      }
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      if (showList && results.length > 0) {
        scrollOnIndex.current = true;
        setIndex((i) => (reversed ? Math.max(i - 1, 0) : Math.min(i + 1, results.length - 1)));
      }
    } else if (e.key === "Enter") {
      if (showList) activate(results[index]);
    } else if (e.key === "Escape") {
      close();
    }
  };

  const emptyText =
    mode === "actions"
      ? "No matching actions"
      : apps.length === 0
        ? "Loading applications…"
        : "No results";

  const width =
    view === "wallpaper" && pickerWidth
      ? pickerWidth
      : view === "settings"
        ? (settingsWidth ?? 780)
        : view === "cheatsheet"
          ? 1040
          : LIST_WIDTH;

  useLayoutEffect(() => {
    if (height === null) return;
    invoke("launcher_set_size", { width, height }).catch(() => {});
  }, [width, height]);

  const resultList = showList && listMounted && (
    <div
      ref={listRef}
      className={`launcher__list ${reversed ? "launcher__list--reverse" : ""}`}
      onScroll={onListScroll}
    >
      {results.length === 0 && <div className="launcher__empty">{emptyText}</div>}
      {shownResults.map((item, i) => (
        <div
          key={item.key}
          className={`litem ${i === index ? "litem--active" : ""}`}
          onMouseMove={() => {
            if (i !== index) setIndex(i);
          }}
          onClick={() => activate(item)}
        >
          {item.icon ? (
            <AppIcon target={item.icon} fallback={item.glyph} size={36} lazy />
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
                {armed === item.key ? "Press Enter again to confirm" : item.hint}
              </span>
            )}
          </div>
        </div>
      ))}
    </div>
  );

  const searchBar = (
    <div className="lsearchbar" onPointerDown={activateLauncher}>
      <button
        type="button"
        className={`lsearch__mode ${mode === "actions" ? "lsearch__mode--actions" : ""}`}
        title={mode === "actions" ? "Back to application list" : "System actions (>)"}
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => {
          setMode((m) => (m === "apps" ? "actions" : "apps"));
          setIndex(0);
          inputRef.current?.focus();
        }}
      >
        {mode === "actions" ? (
          <Icon name="chevron_right" size={22} />
        ) : (
          <svg
            width="20"
            height="20"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <circle cx="11" cy="11" r="7" />
            <line x1="20" y1="20" x2="16.2" y2="16.2" />
          </svg>
        )}
      </button>
      <input
        ref={inputRef}
        className="lsearch"
        placeholder={
          mode === "actions"
            ? "Search system actions…"
            : cfg?.search_placeholder || "Search apps…  (> system actions)"
        }
        value={query}
        onChange={(e) => {
          const v = e.target.value;
          if (v.startsWith(">")) {
            setMode((m) => (m === "apps" ? "actions" : "apps"));
            setQuery(v.slice(1));
          } else {
            setQuery(v);
          }
          setIndex(0);
        }}
        onKeyDown={onKeyDown}
        spellCheck={false}
      />
    </div>
  );

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
        <div className="lpanel__clip">
          <div className="lpanel__inner" ref={innerRef} style={{ width }}>
            <div key={view} className="lpanel__stage">
              {view === "settings" ? (
            <LauncherSettings onBack={() => setView("list")} onWidth={setSettingsWidth} />
          ) : view === "cheatsheet" ? (
            <WmCheatsheet
              visible={visible}
              fromLauncher={csFromLauncher}
              onBack={() => setView("list")}
              onClose={close}
            />
          ) : view === "wallpaper" ? (
            <WallpaperPicker
              onBack={() => setView("list")}
              onDone={close}
              onWidth={setPickerWidth}
            />
          ) : isTop ? (
            <>
              {searchBar}
              {resultList}
            </>
          ) : (
            <>
              {resultList}
              {searchBar}
            </>
          )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
