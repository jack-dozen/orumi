import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import Icon from "./Icon";

export type CheatsheetRow = { keys: string; desc: string };
export type CheatsheetGroup = { title: string; rows: CheatsheetRow[] };

/** vscroll-wm keyboard shortcuts cheatsheet fetched live over IPC. */
export default function WmCheatsheet({
  visible,
  onClose,
  onBack,
  fromLauncher,
}: {
  visible: boolean;
  onClose: () => void;
  onBack: () => void;
  fromLauncher?: boolean;
}) {
  const [groups, setGroups] = useState<CheatsheetGroup[]>([]);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!visible) return;
    let active = true;
    invoke<CheatsheetGroup[]>("wm_cheatsheet_groups")
      .then((data) => {
        if (!active) return;
        setGroups(data);
        setErr(null);
      })
      .catch((e) => {
        if (!active) return;
        setErr(String(e));
      });
    return () => {
      active = false;
    };
  }, [visible]);

  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (fromLauncher) onBack();
        else onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible, fromLauncher, onBack, onClose]);

  return (
    <div className="wmcs">
      <div className="wmcs__head">
        <div className="wmcs__title-row">
          <Icon name="keyboard" size={20} />
          <span className="wmcs__title">WM Keyboard Shortcuts</span>
        </div>
        <span className="wmcs__hint">
          {fromLauncher ? "Esc to go back" : "Esc or Win + / to close"}
        </span>
      </div>

      {err ? (
        <div className="wmcs__empty">Unable to load shortcuts from vscroll-wm: {err}</div>
      ) : groups.length === 0 ? (
        <div className="wmcs__empty">Loading shortcuts…</div>
      ) : (
        <div className="wmcs__grid">
          {groups.map((g) => (
            <div key={g.title} className="wmcs__group">
              <div className="wmcs__group-title">{g.title}</div>
              <div className="wmcs__rows">
                {g.rows.map((r, idx) => (
                  <div key={`${r.keys}-${idx}`} className="wmcs__row">
                    <span className="wmcs__keys">{r.keys}</span>
                    <span className="wmcs__desc">{r.desc}</span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
