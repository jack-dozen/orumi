import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { saveConfig, useConfig, type ColorSlot, type Config } from "../useConfig";
import Icon from "./Icon";
import WallpaperSettings from "./WallpaperSettings";

/** Notch normalnya tidak mengambil fokus; untuk mengetik ia perlu diizinkan sementara. */
const activate = (enable: boolean) =>
  invoke("panel_activation", { label: "notch", enable }).catch(() => {});

/** Batas waktu menunggu kombinasi tombol sebelum penangkapan dibatalkan. */
const CAPTURE_TIMEOUT_MS = 6000;

const HEX = /^#[0-9a-fA-F]{6}$/;

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="srow">
      <div className="srow__label">{label}</div>
      <div className="srow__ctl">{children}</div>
    </div>
  );
}

/**
 * Kolom hotkey: bisa diketik langsung (mis. Ctrl+Alt+K) atau ditangkap dari keyboard.
 * Kombinasi yang sudah dipakai app lain tidak bisa ditangkap (app itu menerimanya lebih dulu);
 * ketik manual supaya Windows memberi tahu bahwa kombinasi itu tidak tersedia.
 */
function HotkeyField({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const [capturing, setCapturing] = useState(false);
  const [text, setText] = useState(value);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => setText(value), [value]);

  useEffect(() => {
    if (!capturing) return;
    activate(true);
    const timeout = window.setTimeout(() => setCapturing(false), CAPTURE_TIMEOUT_MS);

    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      if (e.key === "Escape") {
        setCapturing(false);
        return;
      }
      const code = e.code;
      if (/^(Control|Alt|Shift|Meta)/.test(code)) return; // modifier saja: tunggu tombol utamanya

      let key: string | null = null;
      if (code.startsWith("Key")) key = code.slice(3);
      else if (code.startsWith("Digit")) key = code.slice(5);
      else if (/^F([1-9]|1[0-2])$/.test(code)) key = code;
      else if (code === "Space") key = "Space";
      if (!key) return;

      const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift"].filter(
        Boolean,
      ) as string[];
      if (mods.length === 0) return; // wajib ada modifier supaya tidak membajak ketikan biasa

      setCapturing(false);
      onChangeRef.current([...mods, key].join("+"));
    };
    const onBlur = () => setCapturing(false);

    window.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.clearTimeout(timeout);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", onBlur);
      activate(false);
    };
  }, [capturing]);

  const commit = () => {
    const t = text.trim();
    if (t && t !== value) onChange(t);
    else setText(value);
  };

  return (
    <div className="hotkeyfield">
      <input
        className="sinput"
        value={capturing ? "Tekan kombinasi…" : text}
        readOnly={capturing}
        spellCheck={false}
        onChange={(e) => setText(e.target.value)}
        onFocus={() => activate(true)}
        onBlur={() => {
          activate(false);
          if (!capturing) commit();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
        }}
      />
      <button
        className={`hotkey__cap ${capturing ? "hotkey__cap--on" : ""}`}
        title="Tangkap kombinasi dari keyboard"
        onClick={() => setCapturing((c) => !c)}
      >
        <Icon name="keyboard" size={18} />
      </button>
    </div>
  );
}

// ---------- Aplikasi: skala, kota, hotkey ----------

export default function AppSettings() {
  const cfg = useConfig();
  const [scale, setScale] = useState(80);
  const [city, setCity] = useState("");
  const [note, setNote] = useState("");
  const [ok, setOk] = useState("");
  const [resetKey, setResetKey] = useState(0); // memaksa kolom hotkey memuat ulang nilai tersimpan

  // Isian lokal mengikuti config yang tersimpan.
  useEffect(() => {
    if (!cfg) return;
    setScale(Math.round(cfg.ui_scale * 100));
    setCity(cfg.weather_city);
  }, [cfg]);

  if (!cfg) return <div className="card card--placeholder card--fill">memuat…</div>;

  const save = async (patch: Partial<Config>): Promise<string[]> => {
    setOk("");
    try {
      const res = await saveConfig({ ...cfg, ...patch });
      setNote(res.warnings.join(" "));
      // Ada yang ditolak: kembalikan tampilan kolom ke nilai yang sebenarnya dipakai.
      if (res.warnings.length > 0) setResetKey((k) => k + 1);
      return res.warnings;
    } catch (e) {
      setNote(String(e));
      return [String(e)];
    }
  };

  const setHotkey = (which: keyof Config["hotkeys"]) => async (v: string) => {
    const warnings = await save({ hotkeys: { ...cfg.hotkeys, [which]: v } });
    if (warnings.length === 0) setOk(`Hotkey ${v} terdaftar di Windows.`);
  };

  return (
    <div className="card appset">
      <div className="appset__col">
        <div className="appset__title">Tampilan</div>

        <Row label={`Skala UI ${scale}%`}>
          <input
            type="range"
            min={60}
            max={120}
            step={5}
            value={scale}
            onChange={(e) => setScale(Number(e.target.value))}
            onPointerUp={() => save({ ui_scale: scale / 100 })}
            onKeyUp={() => save({ ui_scale: scale / 100 })}
          />
        </Row>

        <Row label="Kota cuaca">
          <input
            className="sinput"
            value={city}
            spellCheck={false}
            onChange={(e) => setCity(e.target.value)}
            onFocus={() => activate(true)}
            onBlur={() => {
              activate(false);
              const c = city.trim();
              if (c && c !== cfg.weather_city) save({ weather_city: c });
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") (e.target as HTMLInputElement).blur();
            }}
          />
        </Row>
        <Row label="Alt+Tab">
          <div className="segmented segmented--small">
            <button
              className={cfg.alt_tab ? "segmented__on" : ""}
              onClick={() => save({ alt_tab: true })}
            >
              Kustom
            </button>
            <button
              className={!cfg.alt_tab ? "segmented__on" : ""}
              onClick={() => save({ alt_tab: false })}
            >
              Bawaan Windows
            </button>
          </div>
        </Row>
        <div className="appset__title">Wallpaper</div>
        <WallpaperSettings />
      </div>

      <div className="appset__col">
        <div className="appset__title">Pintasan keyboard</div>
        <Row label="Bar">
          <HotkeyField
            key={`bar-${resetKey}`}
            value={cfg.hotkeys.bar}
            onChange={setHotkey("bar")}
          />
        </Row>
        <Row label="Notch">
          <HotkeyField
            key={`notch-${resetKey}`}
            value={cfg.hotkeys.notch}
            onChange={setHotkey("notch")}
          />
        </Row>
        <Row label="Launcher">
          <HotkeyField
            key={`launcher-${resetKey}`}
            value={cfg.hotkeys.launcher}
            onChange={setHotkey("launcher")}
          />
        </Row>
        {note && <div className="settings__warn">{note}</div>}
        {ok && <div className="hotkeynote hotkeynote--ok">{ok}</div>}
        <div className="hotkeynote">
          Windows hanya menolak kombinasi yang dipakai app lewat pendaftaran hotkey biasa. App
          yang memantau keyboard sendiri (mis. Google Drive, AutoHotkey) tidak terdeteksi: kalau
          app lain ikut terbuka saat hotkey ditekan, ganti kombinasinya.
        </div>
      </div>
    </div>
  );
}

// ---------- Warna ----------

const SLOTS = [
  { key: "bg0", label: "Latar panel", note: "bg0", token: "--surface" },
  { key: "bg1", label: "Latar kartu", note: "bg1", token: "--surface-container-high" },
  { key: "fg", label: "Teks", note: "fg", token: "--on-surface" },
  { key: "accent", label: "Aksen", note: "accent", token: "--primary" },
] as const;

type SlotKey = (typeof SLOTS)[number]["key"];

export function ColorSettings() {
  const cfg = useConfig();
  const [tint, setTint] = useState(35);
  const [hex, setHex] = useState<Record<SlotKey, string>>({
    bg0: "",
    bg1: "",
    fg: "",
    accent: "",
  });
  const timers = useRef<Partial<Record<SlotKey, number>>>({});

  useEffect(() => {
    if (!cfg) return;
    setTint(cfg.bg_tint);
    setHex({
      bg0: cfg.colors.bg0.value,
      bg1: cfg.colors.bg1.value,
      fg: cfg.colors.fg.value,
      accent: cfg.colors.accent.value,
    });
  }, [cfg]);

  if (!cfg) return <div className="card card--placeholder card--fill">memuat…</div>;

  const patchSlot = (key: SlotKey, patch: Partial<ColorSlot>) =>
    saveConfig({
      ...cfg,
      colors: { ...cfg.colors, [key]: { ...cfg.colors[key], ...patch } },
    }).catch(() => {});

  /** Memilih warna otomatis menjadikan slot itu custom. Disimpan setelah jeda singkat. */
  const pick = (key: SlotKey, value: string) => {
    setHex((h) => ({ ...h, [key]: value }));
    window.clearTimeout(timers.current[key]);
    timers.current[key] = window.setTimeout(
      () => patchSlot(key, { mode: "custom", value }),
      250,
    );
  };

  const resetAll = () =>
    saveConfig({
      ...cfg,
      colors: {
        bg0: { ...cfg.colors.bg0, mode: "auto" },
        bg1: { ...cfg.colors.bg1, mode: "auto" },
        fg: { ...cfg.colors.fg, mode: "auto" },
        accent: { ...cfg.colors.accent, mode: "auto" },
      },
    }).catch(() => {});

  const tintMatters = [cfg.colors.bg0, cfg.colors.bg1, cfg.colors.fg].some(
    (s) => s.mode === "auto",
  );

  return (
    <div className="card colorset">
      <div className="colorgrid">
        {SLOTS.map((s) => {
          const slot = cfg.colors[s.key];
          return (
            <div className="colorcard" key={s.key}>
              <div className="colorcard__head">
                <span className="swatch" style={{ background: `var(${s.token})` }} />
                <span>{s.label}</span>
                <span className="colorcard__note">{s.note}</span>
              </div>
              <div className="colorcard__ctl">
                <div className="segmented segmented--small">
                  <button
                    className={slot.mode === "auto" ? "segmented__on" : ""}
                    onClick={() => patchSlot(s.key, { mode: "auto" })}
                  >
                    Wallpaper
                  </button>
                  <button
                    className={slot.mode === "custom" ? "segmented__on" : ""}
                    onClick={() => patchSlot(s.key, { mode: "custom" })}
                  >
                    Custom
                  </button>
                </div>
                <input
                  type="color"
                  className="scolor"
                  value={HEX.test(hex[s.key]) ? hex[s.key] : slot.value}
                  onFocus={() => activate(true)}
                  onBlur={() => activate(false)}
                  onChange={(e) => pick(s.key, e.target.value)}
                />
                <input
                  className="sinput hexinput"
                  value={hex[s.key]}
                  spellCheck={false}
                  maxLength={7}
                  onChange={(e) => setHex((h) => ({ ...h, [s.key]: e.target.value }))}
                  onFocus={() => activate(true)}
                  onBlur={() => {
                    activate(false);
                    if (HEX.test(hex[s.key])) pick(s.key, hex[s.key].toLowerCase());
                    else setHex((h) => ({ ...h, [s.key]: slot.value }));
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") e.currentTarget.blur();
                  }}
                />
              </div>
            </div>
          );
        })}
      </div>

      <div className="colorfoot">
        <span className={tintMatters ? "" : "colorfoot--off"}>Warna wallpaper di latar {tint}%</span>
        <input
          type="range"
          min={0}
          max={100}
          step={5}
          value={tint}
          disabled={!tintMatters}
          onChange={(e) => setTint(Number(e.target.value))}
          onPointerUp={() => saveConfig({ ...cfg, bg_tint: tint }).catch(() => {})}
          onKeyUp={() => saveConfig({ ...cfg, bg_tint: tint }).catch(() => {})}
        />
        <button className="linkbtn" onClick={resetAll}>
          Semua ke wallpaper
        </button>
      </div>
    </div>
  );
}
