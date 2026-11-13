import { useEffect, useState } from "react";
import AppSettings, { ColorSettings } from "./Settings";

type Section = "app" | "color";

const SECTIONS: [Section, string][] = [
  ["app", "Aplikasi"],
  ["color", "Warna"],
];

/** ORUMI's own settings, shown inside the launcher (`>setting`). Esc goes back to the list. */
export default function LauncherSettings({
  onBack,
  onWidth,
}: {
  onBack: () => void;
  onWidth?: (w: number | null) => void;
}) {
  const [section, setSection] = useState<Section>("app");

  useEffect(() => {
    onWidth?.(section === "color" ? 720 : 780);
  }, [section, onWidth]);
  useEffect(() => () => onWidth?.(null), [onWidth]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const el = document.activeElement as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA")) {
        el.blur(); // first Esc leaves the field, the next one goes back
        return;
      }
      onBack();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onBack]);

  return (
    <div className="lsettings">
      <div className="segmented">
        {SECTIONS.map(([id, label]) => (
          <button
            key={id}
            className={section === id ? "segmented__on" : ""}
            onClick={() => setSection(id)}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="lsettings__body">
        {section === "app" && <AppSettings />}
        {section === "color" && <ColorSettings />}
      </div>
      <div className="lsettings__hint">Esc untuk kembali</div>
    </div>
  );
}

