import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

type Item = { name: string; path: string; thumb: string };
type List = { dir: string; items: Item[]; start: number | null; error: string | null };

// Carousel geometry (design units): wide center card, narrow strips on either side.
const RADIUS = 5;
const CENTER_W = 320;
const STRIP_W = 56;
const GAP = 8;
const WHEEL_COOLDOWN_MS = 90;
const PRELOAD = 3;

/** Always-nonnegative modulo, so the index wraps in both directions. */
const mod = (n: number, m: number) => ((n % m) + m) % m;

/** Slot center position (px) relative to the center of the stage. */
function slotX(offset: number): number {
  if (offset === 0) return 0;
  const dir = offset > 0 ? 1 : -1;
  const n = Math.abs(offset);
  return dir * (CENTER_W / 2 + GAP + (n - 1) * (STRIP_W + GAP) + STRIP_W / 2);
}

/** Number of slots on each side for a given image count (fewer for small folders). */
const radiusFor = (count: number) => Math.min(RADIUS, Math.floor((count - 1) / 2));

/** Panel width (design units) that fits a given image count. */
function panelWidth(count: number): number {
  const span = CENTER_W + 2 * radiusFor(count) * (STRIP_W + GAP);
  return Math.max(span + 64, 380);
}

/**
 * An endless, rotating carousel-style wallpaper picker. Thumbnails are files
 * on disk loaded through the asset protocol. Position is stored as an unbounded
 * "virtual" number, and slots use that virtual position as their key, so
 * slots keep sliding across the list boundary instead of being recreated.
 */
export default function WallpaperPicker({
  onBack,
  onDone,
  onWidth,
}: {
  onBack: () => void;
  onDone: () => void;
  /** Report the fitted panel width (null = use the regular list width). */
  onWidth?: (w: number | null) => void;
}) {
  const [list, setList] = useState<List | null>(null);
  const [virtual, setVirtual] = useState(0);
  const lastWheel = useRef(0);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    invoke<List>("list_wallpapers", { thumbs: true })
      .then((r) => {
        setList(r);
        setVirtual(r.start ?? 0); // start at the currently active wallpaper
      })
      .catch(() =>
        setList({ dir: "", items: [], start: null, error: "Gagal membaca folder wallpaper" }),
      );
    rootRef.current?.focus();
  }, []);

  const count = list?.items.length ?? 0;
  const active = count > 0 ? mod(virtual, count) : 0;


  // Panel width follows the number of visible slots.
  useEffect(() => {
    onWidth?.(list && !list.error && count > 0 ? panelWidth(count) : null);
  }, [list, count, onWidth]);
  useEffect(() => () => onWidth?.(null), [onWidth]);

  const apply = (i: number) => {
    const item = list?.items[i];
    if (!item) return;
    invoke("set_wallpaper", { path: item.path }).catch(() => {});
    onDone();
  };

  // Preload thumbnails that are about to enter the screen.
  useEffect(() => {
    if (!list || count === 0) return;
    for (let d = RADIUS + 1; d <= RADIUS + PRELOAD; d++) {
      for (const v of [virtual - d, virtual + d]) {
        const t = list.items[mod(v, count)]?.thumb;
        if (t) new Image().src = convertFileSrc(t);
      }
    }
  }, [list, count, virtual]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onBack();
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        setVirtual((v) => v + 1);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        setVirtual((v) => v - 1);
      } else if (e.key === "PageDown") {
        e.preventDefault();
        setVirtual((v) => v + RADIUS);
      } else if (e.key === "PageUp") {
        e.preventDefault();
        setVirtual((v) => v - RADIUS);
      } else if (e.key === "Home") {
        e.preventDefault();
        setVirtual(0);
      } else if (e.key === "End") {
        e.preventDefault();
        setVirtual(Math.max(count - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        apply(active);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (!list) {
    return (
      <div className="wpick wpick__msg">Menyiapkan thumbnail (hanya sekali per gambar)…</div>
    );
  }

  if (list.error || count === 0) {
    return (
      <div className="wpick" ref={rootRef} tabIndex={-1}>
        <div className="wpick__msg">
          {list.error ?? "Tidak ada gambar di folder ini"}
          {list.dir && <div className="wpick__dir">{list.dir}</div>}
          <div className="wpick__dir">Atur folder di Settings → Aplikasi. · Esc untuk kembali</div>
        </div>
      </div>
    );
  }

  // Small folder: limit the number of slots so the same image is not shown twice.
  const radius = radiusFor(count);

  const slots = [];
  for (let o = -radius; o <= radius; o++) {
    const v = virtual + o;
    const actual = mod(v, count);
    const item = list.items[actual];
    slots.push(
      <div
        key={v}
        className={`cw-slot ${o === 0 ? "cw-slot--active" : ""}`}
        style={{
          width: o === 0 ? CENTER_W : STRIP_W,
          transform: `translate3d(calc(-50% + ${slotX(o)}px), -50%, 0)`,
          zIndex: o === 0 ? 30 : 20 - Math.abs(o),
        }}
        onClick={() => (o === 0 ? apply(actual) : setVirtual(v))}
      >
        {item.thumb && (
          <img src={convertFileSrc(item.thumb)} alt="" decoding="async" draggable={false} />
        )}
      </div>,
    );
  }

  return (
    <div
      className="wpick"
      ref={rootRef}
      tabIndex={-1}
      onWheel={(e) => {
        const now = performance.now();
        if (now - lastWheel.current < WHEEL_COOLDOWN_MS) return;
        const dy = Math.abs(e.deltaY) > Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
        if (Math.abs(dy) < 10) return;
        lastWheel.current = now;
        setVirtual((v) => v + (dy > 0 ? 1 : -1));
      }}
    >
      <div className="cw-stage">{slots}</div>
      <div className="wpick__title">
        {list.items[active].name}
        {list.start === active && " · wallpaper saat ini"}
      </div>
    </div>
  );
}
