import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { saveConfig, useConfig, type Config } from "../useConfig";

/** The notch normally does not take focus; temporarily allow it to receive focus for typing. */
const activate = (enable: boolean) =>
  invoke("panel_activation", { label: "notch", enable }).catch(() => {});

type Info = { dir: string; count: number; error: string | null };

export default function WallpaperSettings() {
  const cfg = useConfig();
  const [dir, setDir] = useState("");
  const [cmd, setCmd] = useState("");
  const [info, setInfo] = useState<Info | null>(null);

  useEffect(() => {
    if (!cfg) return;
    setDir(cfg.wallpaper_dir);
    setCmd(cfg.wallpaper_command);
  }, [cfg]);

  // Check the folder: how many images were found.
  useEffect(() => {
    if (!cfg || cfg.wallpaper_switcher !== "builtin") return;
    let cancelled = false;
    invoke<{ dir: string; items: unknown[]; error: string | null }>("list_wallpapers", {
        thumbs: false,
      })
      .then((r) => {
        if (!cancelled) setInfo({ dir: r.dir, count: r.items.length, error: r.error });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [cfg?.wallpaper_dir, cfg?.wallpaper_switcher]);

  if (!cfg) return null;

  const save = (patch: Partial<Config>) => saveConfig({ ...cfg, ...patch }).catch(() => {});
  const builtin = cfg.wallpaper_switcher === "builtin";

  return (
    <>
      <div className="srow">
        <div className="srow__label">Pengalih wallpaper (launcher: &gt; lalu Wallpaper)</div>
        <div className="srow__ctl">
          <div className="segmented segmented--small">
            <button
              className={builtin ? "segmented__on" : ""}
              onClick={() => save({ wallpaper_switcher: "builtin" })}
            >
              Bawaan
            </button>
            <button
              className={!builtin ? "segmented__on" : ""}
              onClick={() => save({ wallpaper_switcher: "external" })}
            >
              Perintah luar
            </button>
          </div>
        </div>
      </div>

      {builtin ? (
        <div className="srow">
          <div className="srow__label">Folder wallpaper (kosong = Pictures)</div>
          <div className="srow__ctl">
            <input
              className="sinput"
              value={dir}
              placeholder="D:\Wallpapers"
              spellCheck={false}
              onChange={(e) => setDir(e.target.value)}
              onFocus={() => activate(true)}
              onBlur={() => {
                activate(false);
                if (dir.trim() !== cfg.wallpaper_dir) save({ wallpaper_dir: dir.trim() });
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
              }}
            />
          </div>
          {info && (
            <div className={`hotkeynote ${info.error ? "settings__warn" : ""}`}>
              {info.error ?? `${info.count} gambar di ${info.dir}`}
            </div>
          )}
        </div>
      ) : (
        <div className="srow">
          <div className="srow__label">Perintah yang dijalankan</div>
          <div className="srow__ctl">
            <input
              className="sinput"
              value={cmd}
              placeholder='mis. "C:\Tools\wallswitch.exe"'
              spellCheck={false}
              onChange={(e) => setCmd(e.target.value)}
              onFocus={() => activate(true)}
              onBlur={() => {
                activate(false);
                if (cmd.trim() !== cfg.wallpaper_command) save({ wallpaper_command: cmd.trim() });
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
              }}
            />
          </div>
        </div>
      )}
    </>
  );
}
