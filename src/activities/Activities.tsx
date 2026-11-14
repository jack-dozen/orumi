import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import Icon from "../components/Icon";
import AppIcon from "../components/AppIcon";
import { useConfig } from "../useConfig";
import {
  closeActivities,
  iconTarget,
  useActivities,
  type AppEntry,
  type SnapWindow,
  type Workspace,
} from "./useActivities";
import "./activities.css";

type View = "ws" | "apps";

/** Scroll threshold (two-finger touchpad) for a gesture to be recognized. */
const SWIPE_THRESHOLD = 80;
/** Delay after recognizing a gesture so touchpad inertia does not trigger it twice. */
const SWIPE_LOCK_MS = 450;
/** Ignore outside clicks (blur) briefly after opening. */
const BLUR_GRACE_MS = 400;

const thumbSrc = (t: string) => convertFileSrc(t);
const letter = (s: string) => (s.trim().charAt(0) || "?").toUpperCase();

/** Name-to-query match score (same rules as the launcher). 0 = no match. */
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

/** Number of app grid columns based on panel width (some width is used by the letter rail on the left). */
const colsFor = (w: number) => Math.max(3, Math.min(9, Math.floor((w - 220) / 140)));

/** Letter rail: symbols/numbers (#), then A-Z = 27 dots. */
const RAIL = "#ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("");

/** Letter group for a name: A-Z (accents removed), otherwise "#". */
const groupOf = (name: string) => {
  const c = name.trim().charAt(0).normalize("NFD").charAt(0).toUpperCase();
  return c >= "A" && c <= "Z" ? c : "#";
};

const byName = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

/** List order: "#" group first, then A-Z, sorted by name within each group. */
function compareApps(a: AppEntry, b: AppEntry) {
  const ga = groupOf(a.name);
  const gb = groupOf(b.name);
  if (ga !== gb) return ga === "#" ? -1 : gb === "#" ? 1 : ga < gb ? -1 : 1;
  return byName.compare(a.name, b.name);
}

/**
 * Request new app icons from Rust as their tiles approach the visible area, so a long list
 * does not trigger hundreds of icon requests at once. Once loaded, the AppIcon cache is used.
 */
function LazyAppIcon({
  target,
  fallback,
  size,
}: {
  target: string;
  fallback: string;
  size: number;
}) {
  const ref = useRef<HTMLSpanElement>(null);
  const [seen, setSeen] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el || seen) return;
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setSeen(true);
          io.disconnect();
        }
      },
      { root: el.closest(".act__viewport"), rootMargin: "300px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [seen]);

  return (
    <span ref={ref} className="act-app__icon">
      <AppIcon target={seen ? target : null} fallback={fallback} size={size} />
    </span>
  );
}

export default function Activities() {
  const { visible, openCount, wins, workspaces, apps, refreshWorkspaces } = useActivities();
  const cfg = useConfig();

  const [view, setView] = useState<View>("ws");
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);
  const [pickedWs, setPickedWs] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [cols, setCols] = useState(() => colsFor(window.innerWidth));
  const [activeLetter, setActiveLetter] = useState("#");

  const inputRef = useRef<HTMLInputElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const openedAt = useRef(0);
  const wheel = useRef({ x: 0, y: 0, last: 0, lock: 0, fresh: true });

  // Reset the display each time it opens.
  useEffect(() => {
    if (openCount === 0) return;
    openedAt.current = Date.now();
    setView("ws");
    setQuery("");
    setSel(0);
    setPickedWs(null);
  }, [openCount]);

  useEffect(() => {
    const onResize = () => setCols(colsFor(window.innerWidth));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Outside click / window loses focus = close.
  useEffect(() => {
    const onBlur = () => {
      if (Date.now() - openedAt.current > BLUR_GRACE_MS) closeActivities();
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Focus the search box when entering the app view.
  useEffect(() => {
    if (!visible || view !== "apps") return;
    const id = setTimeout(() => inputRef.current?.focus(), 30);
    return () => clearTimeout(id);
  }, [visible, view]);

  // Global keyboard: Esc backs out step by step, and typing opens search immediately.
  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (query) {
          setQuery("");
          setSel(0);
        } else if (view === "apps") {
          setView("ws");
        } else {
          closeActivities();
        }
        return;
      }
      if (view === "ws" && e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
        e.preventDefault();
        setView("apps");
        setQuery(e.key);
        setSel(0);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible, view, query]);

  // ---------- Data turunan ----------

  const activeWs: Workspace | undefined =
    workspaces.find((w) => w.id === pickedWs) ??
    workspaces.find((w) => w.active) ??
    workspaces[0];

  const shownWins: SnapWindow[] = useMemo(
    () => (activeWs ? wins.filter((w) => activeWs.windows.includes(w.hwnd)) : wins),
    [wins, activeWs],
  );

  const sorted = useMemo(() => [...apps].sort(compareApps), [apps]);
  const present = useMemo(() => new Set(sorted.map((a) => groupOf(a.name))), [sorted]);
  const searching = query.trim().length > 0;

  const list: AppEntry[] = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return sorted;
    return apps
      .map((a) => ({ a, s: score(a.name, q) }))
      .filter((x) => x.s > 0)
      .sort((x, y) => y.s - x.s || x.a.name.localeCompare(y.a.name))
      .map((x) => x.a);
  }, [apps, sorted, query]);

  // ---------- List scrolling and letter rail ----------

  /** Active letter = the group on the topmost visible row. */
  const updateActive = useCallback(() => {
    const vp = viewportRef.current;
    if (!vp) return;
    const firsts = vp.querySelectorAll<HTMLElement>("[data-first]");
    if (firsts.length === 0) return;
    let cur = firsts[0].dataset.first as string;
    const atEnd = vp.scrollHeight > vp.clientHeight + 2 && vp.scrollTop + vp.clientHeight >= vp.scrollHeight - 2;
    if (atEnd) {
      cur = firsts[firsts.length - 1].dataset.first as string;
    } else {
      for (const el of firsts) {
        if (el.offsetTop <= vp.scrollTop + 4) cur = el.dataset.first as string;
        else break;
      }
    }
    setActiveLetter(cur);
  }, []);

  const rafRef = useRef(0);
  const onListScroll = () => {
    cancelAnimationFrame(rafRef.current);
    rafRef.current = requestAnimationFrame(updateActive);
  };

  // Return to the top when opened or when the search changes; recalculate the active letter.
  useEffect(() => {
    viewportRef.current?.scrollTo({ top: 0 });
    requestAnimationFrame(updateActive);
  }, [openCount, query, updateActive]);

  useEffect(() => {
    requestAnimationFrame(updateActive);
  }, [list, cols, updateActive]);

  const jump = (l: string) => {
    const vp = viewportRef.current;
    const el = vp?.querySelector<HTMLElement>(`[data-first="${l}"]`);
    if (vp && el) vp.scrollTo({ top: Math.max(0, el.offsetTop - 4), behavior: "smooth" });
  };

  /** Ensure tile i is visible (without scrollIntoView, which could shift the panel). */
  const reveal = (i: number) => {
    const vp = viewportRef.current;
    const el = vp?.querySelector<HTMLElement>(`[data-idx="${i}"]`);
    if (!vp || !el) return;
    const top = el.offsetTop;
    const bottom = top + el.offsetHeight;
    if (top < vp.scrollTop) vp.scrollTo({ top: Math.max(0, top - 8) });
    else if (bottom > vp.scrollTop + vp.clientHeight) {
      vp.scrollTo({ top: bottom - vp.clientHeight + 8 });
    }
  };

  // ---------- Aksi ----------

  const openWindow = (w: SnapWindow) => {
    invoke("focus_window", { hwnd: w.hwnd }).catch(() => {});
    closeActivities();
  };

  const launch = (a?: AppEntry) => {
    if (!a) return;
    invoke("launch_app", { id: a.id }).catch(() => {});
    closeActivities();
  };

  const pickWorkspace = (id: string) => {
    setPickedWs(id);
    invoke("ws_switch", { id })
      .then(refreshWorkspaces)
      .catch(() => {});
  };

  const dropOn = (id: string, data: string) => {
    const hwnd = Number(data);
    if (!hwnd) return;
    invoke("ws_move_window", { hwnd, id })
      .then(refreshWorkspaces)
      .catch(() => {});
  };

  const moveSel = (d: number) => {
    const next = Math.max(0, Math.min(list.length - 1, sel + d));
    setSel(next);
    requestAnimationFrame(() => reveal(next));
  };

  const onInputKey = (e: React.KeyboardEvent) => {
    // Arrow keys and Enter apply only to search results (a flat list); the regular list scrolls.
    if (!searching || list.length === 0) return;
    const step: Record<string, number> = {
      ArrowRight: 1,
      ArrowLeft: -1,
      ArrowDown: cols,
      ArrowUp: -cols,
    };
    if (e.key in step) {
      e.preventDefault();
      moveSel(step[e.key]);
    } else if (e.key === "Enter") {
      launch(list[sel]);
    }
  };

  // Touchpad gestures: scroll up = open app grid; scroll down at the top of the list = go back.
  const onWheel = (e: React.WheelEvent) => {
    const s = wheel.current;
    const now = Date.now();
    if (now < s.lock) return;
    if (now - s.last > 160) {
      s.x = 0;
      s.y = 0;
      // A new gesture is valid for "back" only if the list was already at the top.
      s.fresh = (viewportRef.current?.scrollTop ?? 0) <= 0;
    }
    s.last = now;
    s.x += e.deltaX;
    s.y += e.deltaY;

    const fire = () => {
      s.x = 0;
      s.y = 0;
      s.lock = now + SWIPE_LOCK_MS;
    };

    if (view === "ws") {
      if (s.y > SWIPE_THRESHOLD) {
        setView("apps");
        fire();
      }
      return;
    }
    // The app list scrolls independently; scroll down (finger moves down) at the top = go back.
    const top = viewportRef.current?.scrollTop ?? 0;
    if (s.fresh && top <= 0 && !searching && s.y < -SWIPE_THRESHOLD) {
      setView("ws");
      fire();
    }
  };

  const isBg = (e: React.PointerEvent) =>
    (e.target as HTMLElement).dataset.bg !== undefined;

  // ---------- View ----------

  return (
    <div
      className={`act ${visible ? "act--visible" : ""} act--${view}`}
      data-bg
      onPointerDown={(e) => isBg(e) && closeActivities()}
      onWheel={onWheel}
    >
      <div className="act__top" data-bg inert={view !== "ws"}>
        <div className="act__strip" data-bg>
          {workspaces.map((ws) => {
            const thumbs = wins.filter((w) => ws.windows.includes(w.hwnd)).slice(0, 4);
            return (
              <button
                key={ws.id}
                className={[
                  "act-ws",
                  ws.id === activeWs?.id ? "act-ws--active" : "",
                  dropTarget === ws.id ? "act-ws--drop" : "",
                ].join(" ")}
                onClick={() => pickWorkspace(ws.id)}
                onDragOver={(e) => {
                  e.preventDefault();
                  setDropTarget(ws.id);
                }}
                onDragLeave={() => setDropTarget((t) => (t === ws.id ? null : t))}
                onDrop={(e) => {
                  e.preventDefault();
                  setDropTarget(null);
                  dropOn(ws.id, e.dataTransfer.getData("text/plain"));
                }}
              >
                <span className="act-ws__preview">
                  {thumbs.map((w) =>
                    w.thumb ? (
                      <img key={w.hwnd} src={thumbSrc(w.thumb)} alt="" draggable={false} />
                    ) : (
                      <span key={w.hwnd} className="act-ws__blank">
                        <AppIcon
                          target={iconTarget(w)}
                          fallback={letter(w.exe || w.title)}
                          size={28}
                        />
                      </span>
                    ),
                  )}
                </span>
                <span className="act-ws__name">{ws.name}</span>
                <span className="act-ws__count">{ws.windows.length}</span>
              </button>
            );
          })}
        </div>
      </div>

      <div className="act__stage" data-bg>
        <section className="act__wins" data-bg inert={view !== "ws"}>
          {shownWins.length === 0 ? (
            <p className="act__empty">Workspace ini belum punya window</p>
          ) : (
            <div className="act__cards" data-bg>
              {shownWins.map((w) => (
                <button
                  key={w.hwnd}
                  className={`act-card ${w.focused ? "act-card--focused" : ""}`}
                  draggable
                  onDragStart={(e) => {
                    e.dataTransfer.setData("text/plain", String(w.hwnd));
                    e.dataTransfer.effectAllowed = "move";
                  }}
                  onClick={() => openWindow(w)}
                  title={w.title}
                >
                  <span className="act-card__shot">
                    {w.thumb ? (
                      <img src={thumbSrc(w.thumb)} alt="" draggable={false} />
                    ) : (
                      <span className="act-card__blank">
                        <AppIcon
                          target={iconTarget(w)}
                          fallback={letter(w.exe || w.title)}
                          size={72}
                        />
                      </span>
                    )}
                  </span>
                  <span className="act-card__meta">
                    <AppIcon
                      target={iconTarget(w)}
                      fallback={letter(w.exe || w.title)}
                      size={24}
                    />
                    <span className="act-card__title">{w.title || w.exe}</span>
                    {w.minimized && <span className="act-card__tag">diperkecil</span>}
                  </span>
                </button>
              ))}
            </div>
          )}
        </section>

        <section className="act__apps" data-bg inert={view !== "apps"}>
          <input
            ref={inputRef}
            className="act__search"
            placeholder={cfg?.search_placeholder || "Cari aplikasi"}
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSel(0);
            }}
            onKeyDown={onInputKey}
            spellCheck={false}
          />

          <div className="act__viewport" data-bg ref={viewportRef} onScroll={onListScroll}>
            <div
              className="act__grid"
              data-bg
              style={{ gridTemplateColumns: `repeat(${cols}, 1fr)` }}
            >
              {list.map((a, idx) => {
                const g = groupOf(a.name);
                // Start each letter group on a new row so the rail can jump to it precisely.
                const first = !searching && (idx === 0 || groupOf(list[idx - 1].name) !== g);
                return (
                  <button
                    key={a.id}
                    data-idx={idx}
                    data-first={first ? g : undefined}
                    className={`act-app ${idx === sel && searching ? "act-app--sel" : ""}`}
                    style={first ? { gridColumnStart: 1 } : undefined}
                    onMouseMove={() => idx !== sel && setSel(idx)}
                    onClick={() => launch(a)}
                  >
                    <LazyAppIcon target={`aumid:${a.id}`} fallback={letter(a.name)} size={64} />
                    <span className="act-app__name">{a.name}</span>
                  </button>
                );
              })}
            </div>
          </div>

          {list.length === 0 && (
            <p className="act__empty">
              {apps.length === 0 ? "Memuat daftar aplikasi…" : "Tidak ada aplikasi yang cocok"}
            </p>
          )}

          {!searching && (
            <div className="act-rail" role="navigation" aria-label="Lompat ke huruf">
              {RAIL.map((l) => {
                const has = present.has(l);
                return (
                  <button
                    key={l}
                    className={[
                      "act-rail__dot",
                      l === activeLetter ? "act-rail__dot--on" : "",
                      has ? "" : "act-rail__dot--empty",
                    ].join(" ")}
                    data-letter={l}
                    disabled={!has}
                    aria-label={l === "#" ? "Simbol dan angka" : l}
                    onClick={() => jump(l)}
                  />
                );
              })}
            </div>
          )}
        </section>
      </div>

      <button className="act__switch" onClick={() => setView(view === "ws" ? "apps" : "ws")}>
        <Icon name={view === "ws" ? "apps" : "dashboard"} size={20} />
        {view === "ws" ? "Semua aplikasi" : "Workspace"}
      </button>
    </div>
  );
}
