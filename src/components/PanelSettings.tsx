import { useState } from "react";
import {
  saveConfig,
  useConfig,
  type PanelEdge,
  type PanelPref,
  type PanelsConfig,
} from "../useConfig";

type CustomPanelKey = "bar" | "notch" | "launcher" | "overlay";

type PanelMeta = {
  key: CustomPanelKey;
  title: string;
  desc: string;
  edges: PanelEdge[];
  required?: boolean;
};

const CUSTOM_PANELS: PanelMeta[] = [
  {
    key: "bar",
    title: "Bar",
    desc: "App list, workspaces, clock, tray",
    edges: ["left", "right", "top", "bottom"],
  },
  {
    key: "notch",
    title: "Notch",
    desc: "Dashboard, media, notifications, quick settings",
    edges: ["top", "bottom"],
  },
  {
    key: "launcher",
    title: "Launcher",
    desc: "App search, >settings, wallpaper picker",
    edges: ["top", "bottom"],
    required: true,
  },
  {
    key: "overlay",
    title: "Overlay",
    desc: "Volume/brightness OSD, mixer, power menu",
    edges: ["left", "right"],
  },
];

const EDGE_LABEL: Record<PanelEdge, string> = {
  left: "Left",
  right: "Right",
  top: "Top",
  bottom: "Bottom",
};

const ALL_EDGES: PanelEdge[] = ["left", "right", "top", "bottom"];

const DEFAULT_PANELS: PanelsConfig = {
  bar: { enabled: true, edge: "left", hotedge: true },
  notch: { enabled: true, edge: "top", hotedge: true },
  launcher: { enabled: true, edge: "bottom", hotedge: true },
  overlay: { enabled: true, edge: "right", hotedge: true },
  overview: true,
  switcher: true,
  activities: true,
};

function panelTitle(key: CustomPanelKey): string {
  return CUSTOM_PANELS.find((p) => p.key === key)?.title ?? key;
}

export default function PanelSettings() {
  const cfg = useConfig();
  const [note, setNote] = useState("");

  if (!cfg) return <div className="card card--placeholder card--fill">loading…</div>;

  const panels: PanelsConfig = cfg.panels ?? DEFAULT_PANELS;

  const commit = async (next: PanelsConfig, info = "") => {
    try {
      const res = await saveConfig({ ...cfg, panels: next });
      setNote(res.warnings.length > 0 ? res.warnings.join(" ") : info);
    } catch (e) {
      setNote(String(e));
    }
  };

  const hotedgeOwner = (edge: PanelEdge, exclude?: CustomPanelKey): CustomPanelKey | null => {
    for (const m of CUSTOM_PANELS) {
      if (m.key === exclude) continue;
      const p = panels[m.key];
      if (p.enabled && p.hotedge && p.edge === edge) return m.key;
    }
    return null;
  };

  const setEnabled = (key: CustomPanelKey, enabled: boolean) => {
    if (key === "launcher") return;
    const cur = panels[key];
    const nextPref: PanelPref = { ...cur, enabled, hotedge: enabled ? cur.hotedge : false };
    void commit({ ...panels, [key]: nextPref });
  };

  const setEdge = (key: CustomPanelKey, edge: PanelEdge) => {
    const cur = panels[key];
    if (cur.edge === edge) return;
    const owner = hotedgeOwner(edge, key);
    let hotedge = cur.hotedge;
    let info = "";
    if (hotedge && owner) {
      hotedge = false;
      info = `${EDGE_LABEL[edge]} hotedge is owned by ${panelTitle(owner)}; turned off hotedge for ${panelTitle(key)}.`;
    }
    void commit({ ...panels, [key]: { ...cur, edge, hotedge } }, info);
  };

  const setHotedge = (key: CustomPanelKey, hotedge: boolean) => {
    const cur = panels[key];
    if (!cur.enabled && hotedge) return;
    const next: PanelsConfig = { ...panels, [key]: { ...cur, hotedge } };
    let info = "";
    if (hotedge) {
      const owner = hotedgeOwner(cur.edge, key);
      if (owner) {
        next[owner] = { ...panels[owner], hotedge: false };
        info = `${EDGE_LABEL[cur.edge]} hotedge moved from ${panelTitle(owner)} to ${panelTitle(key)}.`;
      }
    }
    void commit(next, info);
  };

  const setFixed = (key: "overview" | "switcher" | "activities", value: boolean) => {
    void commit({ ...panels, [key]: value });
  };

  return (
    <div className="card appset pset">
      <div className="appset__col pset__main">
        <div className="appset__title">Edge Panels</div>
        <div className="pset__list">
          {CUSTOM_PANELS.map((m) => {
            const pref = panels[m.key];
            const owner = hotedgeOwner(pref.edge, m.key);
            return (
              <div
                key={m.key}
                className={`pset__card ${pref.enabled ? "" : "pset__card--off"}`}
              >
                <div className="pset__head">
                  <div>
                    <div className="pset__name">{m.title}</div>
                    <div className="pset__desc">{m.desc}</div>
                  </div>
                  {m.required ? (
                    <span className="wmset__badge">Required</span>
                  ) : (
                    <div className="segmented segmented--small">
                      <button
                        type="button"
                        className={pref.enabled ? "segmented__on" : ""}
                        onClick={() => setEnabled(m.key, true)}
                      >
                        On
                      </button>
                      <button
                        type="button"
                        className={!pref.enabled ? "segmented__on" : ""}
                        onClick={() => setEnabled(m.key, false)}
                      >
                        Off
                      </button>
                    </div>
                  )}
                </div>

                <div className="pset__controls">
                  <div className="segmented segmented--small">
                    {m.edges.map((e) => (
                      <button
                        key={e}
                        type="button"
                        disabled={!pref.enabled}
                        className={pref.edge === e ? "segmented__on" : ""}
                        onClick={() => setEdge(m.key, e)}
                      >
                        {EDGE_LABEL[e]}
                      </button>
                    ))}
                  </div>
                  <button
                    type="button"
                    disabled={!pref.enabled}
                    className={`pset__hot ${pref.hotedge ? "pset__hot--on" : ""}`}
                    title={
                      owner && !pref.hotedge
                        ? `Take ${EDGE_LABEL[pref.edge]} hotedge from ${panelTitle(owner)}`
                        : `Trigger ${m.title} by hovering the ${EDGE_LABEL[pref.edge].toLowerCase()} screen edge`
                    }
                    onClick={() => setHotedge(m.key, !pref.hotedge)}
                  >
                    Hotedge: {pref.hotedge ? "On" : "Off"}
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      </div>

      <div className="appset__col pset__side">
        <div className="appset__title">Other Panels</div>
        <div className="srow">
          <div className="srow__label">WM Overview</div>
          <div className="srow__ctl">
            <div className="segmented segmented--small">
              <button
                type="button"
                className={panels.overview ? "segmented__on" : ""}
                onClick={() => setFixed("overview", true)}
              >
                On
              </button>
              <button
                type="button"
                className={!panels.overview ? "segmented__on" : ""}
                onClick={() => setFixed("overview", false)}
              >
                Off
              </button>
            </div>
          </div>
        </div>
        <div className="srow">
          <div className="srow__label">Alt+Tab Switcher</div>
          <div className="srow__ctl">
            <div className="segmented segmented--small">
              <button
                type="button"
                className={panels.switcher ? "segmented__on" : ""}
                onClick={() => setFixed("switcher", true)}
              >
                On
              </button>
              <button
                type="button"
                className={!panels.switcher ? "segmented__on" : ""}
                onClick={() => setFixed("switcher", false)}
              >
                Off
              </button>
            </div>
          </div>
        </div>
        <div className="srow">
          <div className="srow__label">Activities (Win key)</div>
          <div className="srow__ctl">
            <div className="segmented segmented--small">
              <button
                type="button"
                className={panels.activities ? "segmented__on" : ""}
                onClick={() => setFixed("activities", true)}
              >
                On
              </button>
              <button
                type="button"
                className={!panels.activities ? "segmented__on" : ""}
                onClick={() => setFixed("activities", false)}
              >
                Off
              </button>
            </div>
          </div>
        </div>

        <div className="appset__title" style={{ marginTop: 12 }}>
          Hotedge Map
        </div>
        <div className="pset__map">
          {ALL_EDGES.map((edge) => {
            const owner = hotedgeOwner(edge);
            return (
              <div key={edge} className="pset__map-row">
                <span className="pset__map-edge">{EDGE_LABEL[edge]}</span>
                <span className={owner ? "pset__map-owner" : "pset__map-none"}>
                  {owner ? panelTitle(owner) : "None"}
                </span>
              </div>
            );
          })}
        </div>

        <div className="pset__actions">
          <button
            type="button"
            className="wmset__btn"
            onClick={() => commit(DEFAULT_PANELS, "Panel positions reset to defaults.")}
          >
            Reset defaults
          </button>
        </div>

        {note && <div className="appset__note">{note}</div>}
      </div>
    </div>
  );
}
