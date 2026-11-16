import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

type WmConfigData = {
  connected: boolean;
  paused: boolean;
  config_path: string;
  ignore_path: string;
  gap: number;
  anim_ms: number;
  focus_border: string;
  focus_border_mode: "off" | "theme" | "custom";
  resize_step: number;
  height_mode: "preset" | "step" | string;
  height_step: number;
  mouse_drag: boolean;
  remember_layout: boolean;
  binds: Record<string, string>;
  ignore_text: string;
};

const HEX = /^#[0-9a-fA-F]{6}$/;

const activate = (enable: boolean) =>
  invoke("panel_activation", { label: getCurrentWindow().label, enable }).catch(() => {});

const BIND_DEFS: { key: string; label: string; def: string }[] = [
  { key: "overview", label: "Overview", def: "win+tab, win+alt+space" },
  { key: "cheatsheet", label: "Cheatsheet", def: "win+/" },
  { key: "close", label: "Close window", def: "win+q" },
  { key: "minimize", label: "Minimize", def: "win+m" },
  { key: "float", label: "Float / tile", def: "win+alt+f" },
  { key: "inspect", label: "Window inspector", def: "win+alt+i" },
  { key: "maximize", label: "Full width & height", def: "win+enter" },
  { key: "snap_left", label: "Move left column", def: "win+left" },
  { key: "snap_right", label: "Move right column", def: "win+right" },
  { key: "snap_up", label: "Height grow", def: "win+up" },
  { key: "snap_down", label: "Height shrink", def: "win+down" },
  { key: "split_left", label: "Split left", def: "win+[" },
  { key: "split_right", label: "Split right", def: "win+]" },
  { key: "split_reset", label: "Reset split 50/50", def: "win+\\" },
  { key: "focus_left", label: "Focus left", def: "win+shift+left" },
  { key: "focus_right", label: "Focus right", def: "win+shift+right" },
  { key: "focus_up", label: "Focus up", def: "win+shift+up" },
  { key: "focus_down", label: "Focus down", def: "win+shift+down" },
  { key: "swap_left", label: "Swap left", def: "win+alt+left" },
  { key: "swap_right", label: "Swap right", def: "win+alt+right" },
  { key: "swap_up", label: "Swap up", def: "win+alt+up" },
  { key: "swap_down", label: "Swap down", def: "win+alt+down" },
  { key: "workspace_prev", label: "Previous workspace", def: "win+ctrl+left" },
  { key: "workspace_next", label: "Next workspace", def: "win+ctrl+right" },
  { key: "workspace_goto", label: "Go to workspace 1-9", def: "win+ctrl+1-9" },
  { key: "workspace_send_prev", label: "Send to prev workspace", def: "win+ctrl+shift+left" },
  { key: "workspace_send_next", label: "Send to next workspace", def: "win+ctrl+shift+right" },
  { key: "workspace_send", label: "Send to workspace 1-9", def: "win+ctrl+shift+1-9" },
];

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="srow">
      <div className="srow__label">{label}</div>
      <div className="srow__ctl">{children}</div>
    </div>
  );
}

/** vscroll-wm settings card shown inside LauncherSettings. */
export default function WmSettings() {
  const [cfg, setCfg] = useState<WmConfigData | null>(null);
  const [borderHex, setBorderHex] = useState("#4fa8f5");
  const [bindFilter, setBindFilter] = useState("");
  const [statusMsg, setStatusMsg] = useState("");
  const [errMsg, setErrMsg] = useState("");
  const colorTimer = useRef<number | undefined>(undefined);

  const load = () => {
    invoke<WmConfigData>("wm_config_get")
      .then((d) => {
        setCfg(d);
        if (HEX.test(d.focus_border)) setBorderHex(d.focus_border);
      })
      .catch((e) => setErrMsg(String(e)));
  };

  useEffect(() => {
    load();
    const unlistenWs = listen("workspaces-changed", load);
    const unlistenTheme = listen("theme-changed", () => {
      window.setTimeout(load, 80);
    });
    return () => {
      unlistenWs.then((fn) => fn());
      unlistenTheme.then((fn) => fn());
      window.clearTimeout(colorTimer.current);
    };
  }, []);

  if (!cfg) {
    return <div className="card card--placeholder card--fill">loading WM settings…</div>;
  }

  const save = async (next: WmConfigData, restart = false, msg = "Saved & applied.") => {
    setErrMsg("");
    setStatusMsg("");
    try {
      const saved = await invoke<WmConfigData>("wm_config_save", { data: next, restart });
      setCfg(saved);
      if (HEX.test(saved.focus_border)) setBorderHex(saved.focus_border);
      setStatusMsg(msg);
    } catch (e) {
      setErrMsg(String(e));
    }
  };

  const patch = (partial: Partial<WmConfigData>, msg?: string) => {
    const next = { ...cfg, ...partial };
    setCfg(next);
    return save(next, false, msg);
  };

  const togglePause = async () => {
    setErrMsg("");
    try {
      const paused = await invoke<boolean>("wm_toggle_pause_cmd");
      setCfg((c) => (c ? { ...c, paused } : c));
      setStatusMsg(paused ? "Tiling WM paused." : "Tiling WM resumed.");
    } catch (e) {
      setErrMsg(String(e));
    }
  };

  const reapply = async () => {
    setErrMsg("");
    try {
      await invoke("wm_reapply_cmd");
      setStatusMsg("Windows re-tiled.");
    } catch (e) {
      setErrMsg(String(e));
    }
  };

  const pickBorderColor = (hex: string) => {
    setBorderHex(hex);
    window.clearTimeout(colorTimer.current);
    colorTimer.current = window.setTimeout(() => {
      patch({ focus_border_mode: "custom", focus_border: hex.toLowerCase() }, "Border updated.");
    }, 200);
  };

  const filteredBinds = bindFilter.trim()
    ? BIND_DEFS.filter(
        (b) =>
          b.label.toLowerCase().includes(bindFilter.toLowerCase()) ||
          b.key.toLowerCase().includes(bindFilter.toLowerCase()) ||
          (cfg.binds[b.key] ?? b.def).toLowerCase().includes(bindFilter.toLowerCase()),
      )
    : BIND_DEFS;

  const borderMode = cfg.focus_border_mode ?? (HEX.test(cfg.focus_border) ? "custom" : "off");
  const borderOn = borderMode === "custom";

  return (
    <div className="card appset">
      <div className="appset__col">
        <div className="wmset__head">
          <div className="appset__title">Tiling & Behavior</div>
          <span
            className={`wmset__badge ${
              !cfg.connected
                ? "wmset__badge--off"
                : cfg.paused
                  ? "wmset__badge--paused"
                  : "wmset__badge--ok"
            }`}
          >
            {!cfg.connected ? "Offline" : cfg.paused ? "Paused" : "Active"}
          </span>
        </div>

        <Row label="Window manager">
          <div className="wmset__actions">
            <button type="button" className="wmset__btn" onClick={togglePause}>
              {cfg.paused ? "Resume tiling" : "Pause tiling"}
            </button>
            <button type="button" className="wmset__btn" onClick={reapply}>
              Re-tile now
            </button>
          </div>
        </Row>

        <Row label={`Window gap ${cfg.gap} px`}>
          <input
            type="range"
            min={0}
            max={64}
            step={2}
            value={Math.min(64, cfg.gap)}
            onChange={(e) => setCfg({ ...cfg, gap: Number(e.target.value) })}
            onPointerUp={() => save(cfg)}
            onKeyUp={() => save(cfg)}
          />
        </Row>

        <Row label={cfg.anim_ms === 0 ? "Animation Off" : `Animation ${cfg.anim_ms} ms`}>
          <input
            type="range"
            min={0}
            max={400}
            step={20}
            value={Math.min(400, cfg.anim_ms)}
            onChange={(e) => setCfg({ ...cfg, anim_ms: Number(e.target.value) })}
            onPointerUp={() => save(cfg)}
            onKeyUp={() => save(cfg)}
          />
        </Row>

        <Row label="Focus border">
          <div className="wmset__borderctl">
            <div className="segmented segmented--small">
              <button
                type="button"
                className={borderMode === "off" ? "segmented__on" : ""}
                onClick={() => patch({ focus_border_mode: "off", focus_border: "" })}
              >
                Off
              </button>
              <button
                type="button"
                className={borderMode === "theme" ? "segmented__on" : ""}
                onClick={() =>
                  patch({ focus_border_mode: "theme" }, "Focus border follows theme.")
                }
              >
                Theme
              </button>
              <button
                type="button"
                className={borderMode === "custom" ? "segmented__on" : ""}
                onClick={() =>
                  patch({
                    focus_border_mode: "custom",
                    focus_border: HEX.test(borderHex) ? borderHex : "#4fa8f5",
                  })
                }
              >
                Custom
              </button>
            </div>
            {borderMode === "theme" && (
              <span
                className="swatch"
                title={cfg.focus_border || "Theme accent"}
                style={{ background: "var(--primary)" }}
              />
            )}
            {borderOn && (
              <>
                <input
                  type="color"
                  className="scolor"
                  value={HEX.test(borderHex) ? borderHex : "#4fa8f5"}
                  onFocus={() => activate(true)}
                  onBlur={() => activate(false)}
                  onChange={(e) => pickBorderColor(e.target.value)}
                />
                <input
                  className="sinput hexinput"
                  value={borderHex}
                  maxLength={7}
                  spellCheck={false}
                  onChange={(e) => setBorderHex(e.target.value)}
                  onFocus={() => activate(true)}
                  onBlur={() => {
                    activate(false);
                    const v = borderHex.trim().toLowerCase();
                    if (v === "off" || v === "") {
                      patch({ focus_border_mode: "off", focus_border: "" });
                    } else if (HEX.test(v)) {
                      patch({ focus_border_mode: "custom", focus_border: v });
                    } else {
                      setBorderHex(HEX.test(cfg.focus_border) ? cfg.focus_border : "#4fa8f5");
                    }
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                />
              </>
            )}
          </div>
        </Row>

        <Row label={`Split step ${cfg.resize_step} px`}>
          <input
            type="range"
            min={20}
            max={240}
            step={10}
            value={Math.max(20, Math.min(240, cfg.resize_step))}
            onChange={(e) => setCfg({ ...cfg, resize_step: Number(e.target.value) })}
            onPointerUp={() => save(cfg)}
            onKeyUp={() => save(cfg)}
          />
        </Row>

        <Row label="Height mode">
          <div className="segmented segmented--small">
            <button
              type="button"
              className={cfg.height_mode !== "step" ? "segmented__on" : ""}
              onClick={() => patch({ height_mode: "preset" })}
            >
              Preset
            </button>
            <button
              type="button"
              className={cfg.height_mode === "step" ? "segmented__on" : ""}
              onClick={() => patch({ height_mode: "step" })}
            >
              Step
            </button>
          </div>
        </Row>

        {cfg.height_mode === "step" && (
          <Row label={`Height step ${cfg.height_step} px`}>
            <input
              type="range"
              min={20}
              max={240}
              step={10}
              value={Math.max(20, Math.min(240, cfg.height_step))}
              onChange={(e) => setCfg({ ...cfg, height_step: Number(e.target.value) })}
              onPointerUp={() => save(cfg)}
              onKeyUp={() => save(cfg)}
            />
          </Row>
        )}

        <Row label="Mouse drag">
          <div className="segmented segmented--small">
            <button
              type="button"
              className={cfg.mouse_drag ? "segmented__on" : ""}
              onClick={() => patch({ mouse_drag: true })}
            >
              On
            </button>
            <button
              type="button"
              className={!cfg.mouse_drag ? "segmented__on" : ""}
              onClick={() => patch({ mouse_drag: false })}
            >
              Off
            </button>
          </div>
        </Row>

        <Row label="Remember layout">
          <div className="segmented segmented--small">
            <button
              type="button"
              className={cfg.remember_layout ? "segmented__on" : ""}
              onClick={() => patch({ remember_layout: true })}
            >
              On
            </button>
            <button
              type="button"
              className={!cfg.remember_layout ? "segmented__on" : ""}
              onClick={() => patch({ remember_layout: false })}
            >
              Off
            </button>
          </div>
        </Row>
      </div>

      <div className="appset__col">
        <div className="wmset__head">
          <div className="appset__title">Shortcuts (vscroll-wm)</div>
          <button
            type="button"
            className="linkbtn"
            onClick={() => patch({ binds: {} }, "Reset WM shortcuts to defaults.")}
          >
            Reset defaults
          </button>
        </div>

        <input
          className="sinput"
          value={bindFilter}
          placeholder="Filter shortcuts (e.g. close, focus, win+q)…"
          spellCheck={false}
          onChange={(e) => setBindFilter(e.target.value)}
          onFocus={() => activate(true)}
          onBlur={() => activate(false)}
        />

        <div className="wmset__binds">
          {filteredBinds.map((b) => {
            const val = cfg.binds[b.key] ?? "";
            return (
              <div key={b.key} className="wmset__bind-row">
                <span className="wmset__bind-label">{b.label}</span>
                <input
                  className="sinput wmset__bind-input"
                  value={val}
                  placeholder={b.def}
                  spellCheck={false}
                  onChange={(e) =>
                    setCfg({
                      ...cfg,
                      binds: { ...cfg.binds, [b.key]: e.target.value },
                    })
                  }
                  onFocus={() => activate(true)}
                  onBlur={() => {
                    activate(false);
                    save(cfg);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                  }}
                />
              </div>
            );
          })}
        </div>

        <div className="wmset__head">
          <div className="appset__title">Ignore rules</div>
          <button
            type="button"
            className="wmset__btn"
            onClick={() => save(cfg, true, "Ignore rules saved & WM restarted.")}
          >
            Apply & restart WM
          </button>
        </div>

        <textarea
          className="sinput wmset__ignore"
          rows={3}
          value={cfg.ignore_text}
          placeholder="exe:notepad.exe&#10;class:Dialog&#10;title:Picture-in-Picture"
          spellCheck={false}
          onChange={(e) => setCfg({ ...cfg, ignore_text: e.target.value })}
          onFocus={() => activate(true)}
          onBlur={() => activate(false)}
        />

        {statusMsg && <div className="appset__ok">{statusMsg}</div>}
        {errMsg && <div className="appset__warn">{errMsg}</div>}
      </div>
    </div>
  );
}
