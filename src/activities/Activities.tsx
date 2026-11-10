import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Icon from "../components/Icon";
import {
  closeActivities,
  useActivities,
  type AppEntry,
  type SnapWindow,
  type Workspace,
} from "./useActivities";
import "./activities.css";

type View = "ws" | "apps";

/** Ambang gulir (touchpad dua jari) untuk dianggap satu gestur. */
const SWIPE_THRESHOLD = 80;
/** Jeda setelah satu gestur dikenali, supaya inersia touchpad tidak memicu dua kali. */
const SWIPE_LOCK_MS = 450;
/** Klik di luar (blur) diabaikan sebentar setelah dibuka. */
const BLUR_GRACE_MS = 400;

const thumbSrc = (t: string) => `data:image/jpeg;base64,${t}`;
const letter = (s: string) => (s.trim().charAt(0) || "?").toUpperCase();

/** Skor kecocokan nama dengan query (sama dengan aturan launcher). 0 = tidak cocok. */
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

/** Jumlah kolom dan baris app grid menurut ukuran layar. */
function gridFor(w: number, h: number) {
  return {
    cols: Math.max(3, Math.min(8, Math.floor((w - 160) / 150))),
    rows: Math.max(2, Math.min(5, Math.floor((h - 300) / 150))),
  };
}

export default function Activities() {
  const { visible, openCount, wins, workspaces, apps, refreshWorkspaces } = useActivities();

  const [view, setView] = useState<View>("ws");
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);
  const [page, setPage] = useState(0);
  const [pickedWs, setPickedWs] = useState<string | null>(null);
  const [dropTarget, setDropTarget] = useState<string | null>(null);
  const [grid, setGrid] = useState(() => gridFor(window.innerWidth, window.innerHeight));

  const inputRef = useRef<HTMLInputElement>(null);
  const openedAt = useRef(0);
  const wheel = useRef({ x: 0, y: 0, last: 0, lock: 0 });

  // Reset tampilan setiap kali dibuka.
  useEffect(() => {
    if (openCount === 0) return;
    openedAt.current = Date.now();
    setView("ws");
    setQuery("");
    setSel(0);
    setPage(0);
    setPickedWs(null);
  }, [openCount]);

  useEffect(() => {
    const onResize = () => setGrid(gridFor(window.innerWidth, window.innerHeight));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // Klik di luar / window kehilangan fokus = tutup.
  useEffect(() => {
    const onBlur = () => {
      if (Date.now() - openedAt.current > BLUR_GRACE_MS) closeActivities();
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  // Fokuskan kotak cari saat masuk tampilan app.
  useEffect(() => {
    if (!visible || view !== "apps") return;
    const id = setTimeout(() => inputRef.current?.focus(), 30);
    return () => clearTimeout(id);
  }, [visible, view]);

  // Keyboard global: Esc bertahap, dan mengetik langsung membuka pencarian.
  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (query) {
          setQuery("");
          setSel(0);
          setPage(0);
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
        setPage(0);
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

  const list: AppEntry[] = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return apps;
    return apps
      .map((a) => ({ a, s: score(a.name, q) }))
      .filter((x) => x.s > 0)
      .sort((x, y) => y.s - x.s || x.a.name.localeCompare(y.a.name))
      .map((x) => x.a);
  }, [apps, query]);

  const perPage = grid.cols * grid.rows;
  const pageCount = Math.max(1, Math.ceil(list.length / perPage));
  const curPage = Math.min(page, pageCount - 1);

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
    setPage(Math.floor(next / perPage));
  };

  const onInputKey = (e: React.KeyboardEvent) => {
    if (list.length === 0) return;
    const step: Record<string, number> = {
      ArrowRight: 1,
      ArrowLeft: -1,
      ArrowDown: grid.cols,
      ArrowUp: -grid.cols,
    };
    if (e.key in step) {
      e.preventDefault();
      moveSel(step[e.key]);
    } else if (e.key === "Enter") {
      launch(list[sel]);
    }
  };

  // Gestur touchpad: gulir atas = buka app grid, gulir bawah di halaman pertama = kembali.
  const onWheel = (e: React.WheelEvent) => {
    const s = wheel.current;
    const now = Date.now();
    if (now < s.lock) return;
    if (now - s.last > 160) {
      s.x = 0;
      s.y = 0;
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
    if (Math.abs(s.x) > SWIPE_THRESHOLD && Math.abs(s.x) > Math.abs(s.y)) {
      setPage((p) => Math.max(0, Math.min(pageCount - 1, p + (s.x > 0 ? 1 : -1))));
      fire();
    } else if (s.y > SWIPE_THRESHOLD) {
      setPage((p) => Math.min(pageCount - 1, p + 1));
      fire();
    } else if (s.y < -SWIPE_THRESHOLD) {
      if (curPage > 0) setPage(curPage - 1);
      else if (!query) setView("ws");
      fire();
    }
  };

  const isBg = (e: React.PointerEvent) =>
    (e.target as HTMLElement).dataset.bg !== undefined;

  // ---------- Tampilan ----------

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
                        {letter(w.exe || w.title)}
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
                      <span className="act-card__blank">{letter(w.exe || w.title)}</span>
                    )}
                  </span>
                  <span className="act-card__meta">
                    <span className="act-card__badge">{letter(w.exe || w.title)}</span>
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
            placeholder="Cari aplikasi"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSel(0);
              setPage(0);
            }}
            onKeyDown={onInputKey}
            spellCheck={false}
          />

          <div className="act__viewport" data-bg>
            <div
              className="act__track"
              data-bg
              style={{ transform: `translateX(-${curPage * 100}%)` }}
            >
              {Array.from({ length: pageCount }, (_, p) => (
                <div
                  key={p}
                  className="act__page"
                  data-bg
                  style={{
                    gridTemplateColumns: `repeat(${grid.cols}, 1fr)`,
                    gridTemplateRows: `repeat(${grid.rows}, 1fr)`,
                  }}
                >
                  {list.slice(p * perPage, (p + 1) * perPage).map((a, i) => {
                    const idx = p * perPage + i;
                    return (
                      <button
                        key={a.id}
                        className={`act-app ${idx === sel && query ? "act-app--sel" : ""}`}
                        onMouseMove={() => idx !== sel && setSel(idx)}
                        onClick={() => launch(a)}
                      >
                        <span className="act-app__tile">{letter(a.name)}</span>
                        <span className="act-app__name">{a.name}</span>
                      </button>
                    );
                  })}
                </div>
              ))}
            </div>
          </div>

          {list.length === 0 && (
            <p className="act__empty">
              {apps.length === 0 ? "Memuat daftar aplikasi…" : "Tidak ada aplikasi yang cocok"}
            </p>
          )}

          {pageCount > 1 && (
            <div className="act__dots">
              {Array.from({ length: pageCount }, (_, p) => (
                <button
                  key={p}
                  className={`act__dot ${p === curPage ? "act__dot--on" : ""}`}
                  aria-label={`Halaman ${p + 1}`}
                  onClick={() => setPage(p)}
                />
              ))}
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
