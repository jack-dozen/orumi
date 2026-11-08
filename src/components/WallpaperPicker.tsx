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

/** Modulo yang selalu non-negatif, supaya indeks berputar di kedua arah. */
const mod = (n: number, m: number) => ((n % m) + m) % m;

/** Posisi tengah slot (px) relatif ke tengah panggung. */
function slotX(offset: number): number {
  if (offset === 0) return 0;
  const dir = offset > 0 ? 1 : -1;
  const n = Math.abs(offset);
  return dir * (CENTER_W / 2 + GAP + (n - 1) * (STRIP_W + GAP) + STRIP_W / 2);
}

/** Jumlah slot di tiap sisi untuk sejumlah gambar (folder kecil: lebih sedikit). */
const radiusFor = (count: number) => Math.min(RADIUS, Math.floor((count - 1) / 2));

/** Lebar panel (satuan desain) yang pas untuk sejumlah gambar. */
function panelWidth(count: number): number {
  const span = CENTER_W + 2 * radiusFor(count) * (STRIP_W + GAP);
  return Math.max(span + 64, 380);
}

/**
 * Pengalih wallpaper bergaya carousel yang berputar tanpa ujung. Thumbnail berupa file
 * di disk yang dimuat lewat asset protocol. Posisi disimpan sebagai angka "virtual" yang
 * bebas bertambah/berkurang, dan slot memakai posisi virtual itu sebagai key, jadi
 * saat berputar melewati ujung daftar slot tetap meluncur, bukan dibuat ulang.
 */
export default function WallpaperPicker({
  onBack,
  onDone,
  onWidth,
}: {
  onBack: () => void;
  onDone: () => void;
  /** Melaporkan lebar panel yang pas (null = pakai lebar daftar biasa). */
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
        setVirtual(r.start ?? 0); // mulai di wallpaper yang sedang aktif
      })
      .catch(() =>
        setList({ dir: "", items: [], start: null, error: "Gagal membaca folder wallpaper" }),
      );
    rootRef.current?.focus();
  }, []);

  const count = list?.items.length ?? 0;
  const active = count > 0 ? mod(virtual, count) : 0;


  // Lebar panel mengikuti jumlah slot yang tampil.
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

  // Muat lebih dulu thumbnail yang sebentar lagi masuk ke layar.
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

  // Folder kecil: batasi jumlah slot supaya gambar yang sama tidak tampil dua kali.
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
