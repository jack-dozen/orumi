import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

type Item = { name: string; path: string; thumb: string };
type List = { dir: string; items: Item[]; start: number | null; error: string | null };

// Geometri carousel (satuan desain): kartu tengah lebar, strip sempit di kiri-kanan.
const RADIUS = 5;
const CENTER_W = 320;
const STRIP_W = 56;
const GAP = 8;
const WHEEL_COOLDOWN_MS = 90;
const PRELOAD = 3;

/** Posisi tengah slot (px) relatif ke tengah panggung. */
function slotX(offset: number): number {
  if (offset === 0) return 0;
  const dir = offset > 0 ? 1 : -1;
  const n = Math.abs(offset);
  return dir * (CENTER_W / 2 + GAP + (n - 1) * (STRIP_W + GAP) + STRIP_W / 2);
}

/**
 * Pengalih wallpaper bergaya carousel. Thumbnail berupa file di disk yang dimuat lewat
 * asset protocol (bukan base64 lewat IPC), dan slot memakai key indeks mutlak sehingga
 * berpindah dengan meluncur, bukan dibuat ulang.
 */
export default function WallpaperPicker({
  onBack,
  onDone,
}: {
  onBack: () => void;
  onDone: () => void;
}) {
  const [list, setList] = useState<List | null>(null);
  const [index, setIndex] = useState(0);
  const lastWheel = useRef(0);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    invoke<List>("list_wallpapers", { thumbs: true })
      .then((r) => {
        setList(r);
        setIndex(r.start ?? 0); // mulai di wallpaper yang sedang aktif
      })
      .catch(() =>
        setList({ dir: "", items: [], start: null, error: "Gagal membaca folder wallpaper" }),
      );
    rootRef.current?.focus();
  }, []);

  const count = list?.items.length ?? 0;
  const clamp = (i: number) => Math.max(0, Math.min(count - 1, i));

  const apply = (i: number) => {
    const item = list?.items[i];
    if (!item) return;
    invoke("set_wallpaper", { path: item.path }).catch(() => {});
    onDone();
  };

  // Muat lebih dulu thumbnail yang sebentar lagi masuk ke layar.
  useEffect(() => {
    if (!list) return;
    for (let d = RADIUS + 1; d <= RADIUS + PRELOAD; d++) {
      for (const i of [index - d, index + d]) {
        const t = list.items[i]?.thumb;
        if (t) new Image().src = convertFileSrc(t);
      }
    }
  }, [list, index]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onBack();
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        setIndex((i) => clamp(i + 1));
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        setIndex((i) => clamp(i - 1));
      } else if (e.key === "PageDown") {
        e.preventDefault();
        setIndex((i) => clamp(i + RADIUS));
      } else if (e.key === "PageUp") {
        e.preventDefault();
        setIndex((i) => clamp(i - RADIUS));
      } else if (e.key === "Home") {
        setIndex(0);
      } else if (e.key === "End") {
        setIndex(Math.max(count - 1, 0));
      } else if (e.key === "Enter") {
        e.preventDefault();
        apply(index);
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

  const slots = [];
  for (let o = -RADIUS; o <= RADIUS; o++) {
    const i = index + o;
    if (i < 0 || i >= count) continue;
    const item = list.items[i];
    slots.push(
      <div
        key={i}
        className={`cw-slot ${o === 0 ? "cw-slot--active" : ""}`}
        style={{
          width: o === 0 ? CENTER_W : STRIP_W,
          transform: `translate3d(calc(-50% + ${slotX(o)}px), -50%, 0)`,
          zIndex: o === 0 ? 30 : 20 - Math.abs(o),
        }}
        onClick={() => (o === 0 ? apply(i) : setIndex(i))}
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
        setIndex((i) => clamp(i + (dy > 0 ? 1 : -1)));
      }}
    >
      <div className="cw-stage">{slots}</div>
      <div className="wpick__title">
        {list.items[index].name}
        {list.start === index && " · wallpaper saat ini"}
      </div>
      <div className="wpick__hint">
        {index + 1} / {count} · ← → pilih · Enter terapkan · Esc kembali
      </div>
    </div>
  );
}
