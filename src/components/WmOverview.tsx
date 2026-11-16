import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEdgeVisible } from "../useEdgeVisible";
import AppIcon from "./AppIcon";

type SpanKind = "full" | "left" | "right";

type OverviewItem = {
  hwnd: number;
  x: number;
  y: number;
  w: number;
  h: number;
  span: SpanKind;
  title: string;
  exe: string;
  thumb?: string | null;
  icon?: string | null;
};

type OverviewWorkspace = {
  index: number;
  active: boolean;
  width: number;
  height: number;
  view_y: number;
  view_h: number;
  focused: number;
  items: OverviewItem[];
};

type OverviewStateData = {
  paused: boolean;
  active: number;
  workspaces: OverviewWorkspace[];
};

type SlotKind = "split" | "beside" | "before" | "after" | "empty";

type DropSlot = {
  targetWs: number;
  kind: SlotKind;
  targetHwnd: number;
  first: boolean;
  mark: { x: number; y: number; w: number; h: number; isBar?: boolean };
};

type DragSession = {
  item: OverviewItem;
  startX: number;
  startY: number;
  x: number;
  y: number;
  active: boolean;
};

const closeOverview = () =>
  invoke("set_panel", { label: "overview", open: false }).catch(() => {});

function pointInItem(it: { x: number; y: number; w: number; h: number }, px: number, py: number) {
  return px >= it.x && px < it.x + it.w && py >= it.y && py < it.y + it.h;
}

function cleanFull(items: OverviewItem[], target: OverviewItem, stripWidth: number): boolean {
  const adj = Math.max(24, Math.round(stripWidth * 0.02));
  for (const it of items) {
    if (it.hwnd === target.hwnd || it.span === "full") continue;
    const above = Math.abs(it.y + it.h - target.y) <= adj;
    const below = Math.abs(target.y + target.h - it.y) <= adj;
    if (above || below) return false;
  }
  return true;
}

/** Port of vscroll-wm's overview::slot_at in strip coordinates. */
function computeDropSlot(
  ws: OverviewWorkspace,
  px: number,
  py: number,
  srcHwnd: number,
): DropSlot | null {
  if (px < 0 || px >= ws.width || py < -60 || py >= ws.height + 60) {
    return null;
  }

  const remaining = ws.items.filter((it) => it.hwnd !== srcHwnd);
  if (remaining.length === 0) {
    if (ws.items.length === 0) {
      return {
        targetWs: ws.index,
        kind: "empty",
        targetHwnd: 0,
        first: false,
        mark: { x: 0, y: 0, w: ws.width, h: ws.height },
      };
    }
    return null;
  }

  // Hovering over the dragged tile's own original rect cancels drop.
  const selfItem = ws.items.find((it) => it.hwnd === srcHwnd);
  if (selfItem && pointInItem(selfItem, px, py)) {
    return null;
  }

  let topItem = remaining[0];
  let bottomItem = remaining[0];
  for (const it of remaining) {
    if (it.y < topItem.y || (it.y === topItem.y && it.x < topItem.x)) topItem = it;
    const b = it.y + it.h;
    const curB = bottomItem.y + bottomItem.h;
    if (b > curB || (b === curB && it.x < bottomItem.x)) bottomItem = it;
  }

  const gap = Math.max(8, Math.round(ws.width * 0.008));
  const edgeOf = (it: OverviewItem) => Math.max(40, Math.round(it.h * 0.22));
  const barH = Math.max(18, Math.round(ws.height * 0.018));
  const makeBar = (yy: number) => ({
    x: 0,
    y: Math.max(0, yy - barH / 2),
    w: ws.width,
    h: barH,
    isBar: true,
  });

  if (py < topItem.y + edgeOf(topItem)) {
    return {
      targetWs: ws.index,
      kind: "before",
      targetHwnd: topItem.hwnd,
      first: false,
      mark: makeBar(Math.max(gap, topItem.y - gap)),
    };
  }
  if (py >= bottomItem.y + bottomItem.h - edgeOf(bottomItem)) {
    return {
      targetWs: ws.index,
      kind: "after",
      targetHwnd: bottomItem.hwnd,
      first: false,
      mark: makeBar(Math.min(ws.height - gap, bottomItem.y + bottomItem.h + gap)),
    };
  }

  // Find item containing py (nearest in x), else vertically closest.
  let near: OverviewItem | null = null;
  let bestDx = Infinity;
  for (const it of remaining) {
    if (py >= it.y && py < it.y + it.h) {
      const dx = px < it.x ? it.x - px : px >= it.x + it.w ? px - (it.x + it.w) + 1 : 0;
      if (dx < bestDx) {
        bestDx = dx;
        near = it;
      }
    }
  }
  if (!near) {
    let bestDy = Infinity;
    for (const it of remaining) {
      const dy = py < it.y ? it.y - py : py - (it.y + it.h) + 1;
      if (dy < bestDy) {
        bestDy = dy;
        near = it;
      }
    }
  }
  if (!near) return null;

  const inside = pointInItem(near, px, py);
  if (near.span === "full") {
    if (inside && cleanFull(remaining, near, ws.width)) {
      const midX = near.x + near.w / 2;
      const first = px < midX;
      return {
        targetWs: ws.index,
        kind: "split",
        targetHwnd: near.hwnd,
        first,
        mark: {
          x: first ? near.x : midX,
          y: near.y,
          w: near.w / 2,
          h: near.h,
        },
      };
    }
    if (py < near.y + near.h / 2) {
      return {
        targetWs: ws.index,
        kind: "before",
        targetHwnd: near.hwnd,
        first: false,
        mark: makeBar(Math.max(gap, near.y - gap)),
      };
    }
    return {
      targetWs: ws.index,
      kind: "after",
      targetHwnd: near.hwnd,
      first: false,
      mark: makeBar(Math.min(ws.height - gap, near.y + near.h + gap)),
    };
  }

  if (inside) {
    const midY = near.y + near.h / 2;
    const first = py < midY;
    return {
      targetWs: ws.index,
      kind: "split",
      targetHwnd: near.hwnd,
      first,
      mark: {
        x: near.x,
        y: first ? near.y : midY,
        w: near.w,
        h: near.h / 2,
      },
    };
  }

  const besideX = near.span === "right" ? gap : near.x + near.w + gap;
  const besideW =
    near.span === "right" ? Math.max(20, near.x - 2 * gap) : Math.max(20, ws.width - besideX - gap);
  return {
    targetWs: ws.index,
    kind: "beside",
    targetHwnd: near.hwnd,
    first: false,
    mark: {
      x: besideX,
      y: near.y,
      w: besideW,
      h: near.h,
    },
  };
}

function tileLabel(item: OverviewItem): string {
  const exe = item.exe.replace(/\.exe$/i, "");
  return item.title.trim() || exe || "Window";
}

function tileGlyph(item: OverviewItem): string {
  return tileLabel(item).charAt(0).toUpperCase() || "?";
}

export default function WmOverview() {
  const visible = useEdgeVisible("overview");
  const [fromBar, setFromBar] = useState(false);
  const [data, setData] = useState<OverviewStateData | null>(null);
  const [viewedWs, setViewedWs] = useState(0);
  const [selectedHwnd, setSelectedHwnd] = useState<number>(0);
  const [thumbs, setThumbs] = useState<Record<number, string>>({});
  const [stageWidth, setStageWidth] = useState(380);
  const [drag, setDrag] = useState<DragSession | null>(null);
  const [dropSlot, setDropSlot] = useState<DropSlot | null>(null);
  const [dotHover, setDotHover] = useState<number | null>(null);

  const stageRef = useRef<HTMLDivElement>(null);
  const railRef = useRef<HTMLDivElement>(null);
  const openedAt = useRef(0);
  const dotDwellTimer = useRef<number | undefined>(undefined);
  const dotHoverRef = useRef<number | null>(null);
  dotHoverRef.current = dotHover;

  const dataRef = useRef(data);
  dataRef.current = data;
  const viewedWsRef = useRef(viewedWs);
  viewedWsRef.current = viewedWs;
  const stageWidthRef = useRef(stageWidth);
  stageWidthRef.current = stageWidth;
  const dragRef = useRef(drag);
  dragRef.current = drag;
  const dropSlotRef = useRef(dropSlot);
  dropSlotRef.current = dropSlot;
  const railWheel = useRef({ acc: 0, lastAt: 0, locked: false, lockedAt: 0 });

  const loadState = useCallback((resetToActive: boolean) => {
    invoke<OverviewStateData>("wm_overview_state")
      .then((next) => {
        setData(next);
        setThumbs((prev) => {
          const merged = { ...prev };
          for (const ws of next.workspaces) {
            for (const it of ws.items) {
              if (it.thumb) merged[it.hwnd] = it.thumb;
            }
          }
          return merged;
        });
        if (resetToActive) {
          setViewedWs(next.active);
          const cur = next.workspaces[next.active];
          setSelectedHwnd(cur?.focused || cur?.items[0]?.hwnd || 0);
        } else {
          setViewedWs((curIdx) => Math.max(0, Math.min(curIdx, next.workspaces.length - 1)));
        }
      })
      .catch(() => {});
  }, []);

  useLayoutEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const measure = () => {
      if (el.clientWidth > 0) setStageWidth(el.clientWidth);
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [visible, viewedWs]);

  useEffect(() => {
    const unlistenSrc = listen<string>("overview-source", (e) => {
      setFromBar(e.payload === "bar");
    });
    const unlistenThumbs = listen<Record<string, string>>("overview-thumbs", (e) => {
      setThumbs((prev) => {
        const next = { ...prev };
        for (const [k, v] of Object.entries(e.payload)) {
          next[Number(k)] = v;
        }
        return next;
      });
    });
    return () => {
      unlistenSrc.then((fn) => fn());
      unlistenThumbs.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    if (!visible) {
      setDrag(null);
      setDropSlot(null);
      setDotHover(null);
      window.clearTimeout(dotDwellTimer.current);
      return;
    }
    openedAt.current = Date.now();
    loadState(true);
    const unlistenRefresh = listen("overview-refresh", () => loadState(false));
    const unlistenWs = listen("workspaces-changed", () => loadState(false));
    return () => {
      unlistenRefresh.then((fn) => fn());
      unlistenWs.then((fn) => fn());
    };
  }, [visible, loadState]);

  // Outside click / focus loss closes overview (unless mid-drag).
  useEffect(() => {
    const onBlur = () => {
      if (dragRef.current?.active) return;
      if (Date.now() - openedAt.current > 350) closeOverview();
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, []);

  const curWs = data?.workspaces[viewedWs] ?? null;

  // Global pointer tracking for smooth drag-and-drop across tiles & workspace dots.
  useEffect(() => {
    const onMove = (e: PointerEvent) => {
      const d = dragRef.current;
      if (!d) return;
      const dist = Math.hypot(e.clientX - d.startX, e.clientY - d.startY);
      const active = d.active || dist >= 6;
      const nextDrag = { ...d, x: e.clientX, y: e.clientY, active };
      dragRef.current = nextDrag;
      setDrag(nextDrag);
      if (!active) return;

      // 1. Check workspace dot hover on the left rail -> dwell switches viewed workspace live.
      let hoveredDot: number | null = null;
      const railEl = railRef.current;
      if (railEl) {
        const dots = railEl.querySelectorAll<HTMLElement>("[data-ws-dot]");
        for (const dot of dots) {
          const r = dot.getBoundingClientRect();
          if (
            e.clientX >= r.left - 4 &&
            e.clientX <= r.right + 4 &&
            e.clientY >= r.top - 3 &&
            e.clientY <= r.bottom + 3
          ) {
            hoveredDot = Number(dot.dataset.wsDot);
            break;
          }
        }
      }

      if (hoveredDot !== dotHoverRef.current) {
        dotHoverRef.current = hoveredDot;
        setDotHover(hoveredDot);
        window.clearTimeout(dotDwellTimer.current);
        if (hoveredDot !== null && hoveredDot !== viewedWsRef.current) {
          const targetIdx = hoveredDot;
          dotDwellTimer.current = window.setTimeout(() => {
            setViewedWs(targetIdx);
            setDropSlot(null);
          }, 130);
        }
      }

      // 2. Compute exact drop slot inside currently displayed workspace stage.
      const ws = dataRef.current?.workspaces[viewedWsRef.current];
      const stageEl = stageRef.current;
      if (!ws || !stageEl) {
        setDropSlot(null);
        return;
      }
      const rect = stageEl.getBoundingClientRect();
      const sw = stageWidthRef.current || rect.width || 1;
      const scale = sw / ws.width;
      const relX = e.clientX - rect.left;
      const relY = e.clientY - rect.top;
      if (relX < -16 || relX > rect.width + 16 || relY < -24 || relY > rect.height + 24) {
        setDropSlot(null);
        return;
      }
      const slot = computeDropSlot(ws, relX / scale, relY / scale, d.item.hwnd);
      setDropSlot(slot);
    };

    const onUp = () => {
      const d = dragRef.current;
      if (!d) return;
      const slot = dropSlotRef.current;
      const hoveredDot = dotHoverRef.current;
      window.clearTimeout(dotDwellTimer.current);
      dragRef.current = null;
      setDrag(null);
      setDropSlot(null);
      setDotHover(null);

      if (!d.active) {
        // Short click on tile: focus that window (and switch workspace if needed).
        invoke("wm_overview_pick", { hwnd: d.item.hwnd }).catch(() => {});
        return;
      }

      if (slot) {
        invoke("wm_overview_drop", {
          srcHwnd: d.item.hwnd,
          targetWs: slot.targetWs,
          kind: slot.kind,
          targetHwnd: slot.targetHwnd,
          first: slot.first,
        }).catch(() => {});
      } else if (hoveredDot !== null) {
        const ws = dataRef.current?.workspaces[hoveredDot];
        if (ws && !ws.items.some((it) => it.hwnd === d.item.hwnd)) {
          invoke("wm_overview_drop", {
            srcHwnd: d.item.hwnd,
            targetWs: hoveredDot,
            kind: "empty",
            targetHwnd: 0,
            first: false,
          }).catch(() => {});
          setViewedWs(hoveredDot);
        }
      }
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onUp);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
      window.clearTimeout(dotDwellTimer.current);
    };
  }, []);

  // Keyboard navigation inside the overview (Esc, Enter, Arrows, 1-9).
  useEffect(() => {
    if (!visible) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        if (dragRef.current?.active) {
          dragRef.current = null;
          setDrag(null);
          setDropSlot(null);
          setDotHover(null);
        } else {
          closeOverview();
        }
        return;
      }
      const ws = dataRef.current?.workspaces[viewedWsRef.current];
      if (!ws) return;
      if (e.key === "Enter" && selectedHwnd) {
        e.preventDefault();
        invoke("wm_overview_pick", { hwnd: selectedHwnd }).catch(() => {});
        return;
      }
      if (e.key >= "1" && e.key <= "9") {
        const idx = Number(e.key) - 1;
        if (dataRef.current && idx < dataRef.current.workspaces.length) {
          e.preventDefault();
          setViewedWs(idx);
          const target = dataRef.current.workspaces[idx];
          setSelectedHwnd(target.focused || target.items[0]?.hwnd || 0);
        }
        return;
      }
      if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"].includes(e.key)) {
        e.preventDefault();
        if (ws.items.length === 0) return;
        const cur = ws.items.find((it) => it.hwnd === selectedHwnd) ?? ws.items[0];
        const cx = cur.x + cur.w / 2;
        const cy = cur.y + cur.h / 2;
        let best: OverviewItem | null = null;
        let bestScore = Infinity;
        for (const it of ws.items) {
          if (it.hwnd === cur.hwnd) continue;
          const ix = it.x + it.w / 2;
          const iy = it.y + it.h / 2;
          const dx = ix - cx;
          const dy = iy - cy;
          const ok =
            (e.key === "ArrowLeft" && dx < -10) ||
            (e.key === "ArrowRight" && dx > 10) ||
            (e.key === "ArrowUp" && dy < -10) ||
            (e.key === "ArrowDown" && dy > 10);
          if (!ok) continue;
          const dist = Math.hypot(dx, dy);
          if (dist < bestScore) {
            bestScore = dist;
            best = it;
          }
        }
        if (best) setSelectedHwnd(best.hwnd);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [visible, selectedHwnd]);

  const onRailWheel = (e: React.WheelEvent<HTMLDivElement>) => {
    if (!data || data.workspaces.length <= 1) return;
    e.preventDefault();
    const now = performance.now();
    const s = railWheel.current;
    const gap = now - s.lastAt;
    s.lastAt = now;
    if (s.locked) {
      if (gap > 110 && now >= s.lockedAt + 160) {
        s.locked = false;
        s.acc = 0;
      } else {
        return;
      }
    }
    if (gap > 110) s.acc = 0;
    s.acc += e.deltaY;
    if (Math.abs(s.acc) < 40) return;
    const dir = s.acc > 0 ? 1 : -1;
    s.acc = 0;
    s.locked = true;
    s.lockedAt = now;
    const nextIdx = Math.max(0, Math.min(data.workspaces.length - 1, viewedWs + dir));
    setViewedWs(nextIdx);
    const target = data.workspaces[nextIdx];
    setSelectedHwnd(target?.focused || target?.items[0]?.hwnd || 0);
  };

  const scale = curWs && curWs.width > 0 ? stageWidth / curWs.width : 0.2;
  const stripHeight = curWs
    ? Math.max(curWs.height * scale, stageWidth * 0.5625)
    : stageWidth * 0.5625;

  return (
    <div
      className={`wmov ${visible ? "wmov--visible" : ""} ${fromBar ? "wmov--from-bar" : ""}`}
    >
      {/* Left vertical workspace dots rail */}
      <div className="wmov__rail" ref={railRef} onWheel={onRailWheel}>
        <div className="wmov__dots">
          {(data?.workspaces ?? []).map((ws) => {
            const isViewed = ws.index === viewedWs;
            const isActiveOs = ws.active;
            const isEmpty = ws.items.length === 0;
            const isHover = dotHover === ws.index;
            return (
              <button
                key={ws.index}
                type="button"
                data-ws-dot={ws.index}
                className={`wmov__dot-hit ${isHover ? "wmov__dot-hit--drop" : ""}`}
                title={`Workspace ${ws.index + 1}${isEmpty ? " (empty)" : ` (${ws.items.length})`}`}
                onClick={() => {
                  setViewedWs(ws.index);
                  setSelectedHwnd(ws.focused || ws.items[0]?.hwnd || 0);
                }}
              >
                <span
                  className={[
                    "wmov__dot",
                    isViewed ? "wmov__dot--viewed" : "",
                    isActiveOs && !isViewed ? "wmov__dot--active-os" : "",
                    isEmpty ? "wmov__dot--empty" : "",
                  ]
                    .filter(Boolean)
                    .join(" ")}
                />
              </button>
            );
          })}
        </div>
      </div>

      {/* Main workspace tile layout panel */}
      <div className="wmov__main">
        <div className="wmov__header">
          <div className="wmov__title-wrap">
            <span className="wmov__title">Workspace {viewedWs + 1}</span>
            {curWs?.active && <span className="wmov__badge">Current</span>}
          </div>
          <span className="wmov__count">
            {curWs
              ? curWs.items.length === 0
                ? "Empty"
                : `${curWs.items.length} ${curWs.items.length === 1 ? "window" : "windows"}`
              : ""}
          </span>
        </div>

        <div className="wmov__canvas">
          <div
            ref={stageRef}
            className={`wmov__stage ${curWs && curWs.items.length === 0 ? "wmov__stage--empty" : ""}`}
            style={{ height: stripHeight }}
          >
            {curWs && curWs.items.length === 0 ? (
              <div
                className={`wmov__empty ${dropSlot?.kind === "empty" ? "wmov__empty--drop" : ""}`}
              >
                {drag?.active ? (
                  <span>Drop here to move to Workspace {curWs.index + 1}</span>
                ) : (
                  <span>Workspace {curWs.index + 1} is empty</span>
                )}
              </div>
            ) : (
              <>
                {curWs && curWs.height > curWs.view_h + 4 && (
                  <div
                    className="wmov__viewport"
                    style={{
                      top: curWs.view_y * scale,
                      height: curWs.view_h * scale,
                    }}
                  />
                )}

                {curWs?.items.map((item) => {
                  const thumbPath = thumbs[item.hwnd] ?? item.thumb;
                  const isDragged = drag?.active && drag.item.hwnd === item.hwnd;
                  const isSelected = selectedHwnd === item.hwnd;
                  const isFocused = curWs.focused === item.hwnd;
                  const label = tileLabel(item);
                  return (
                    <div
                      key={item.hwnd}
                      className={[
                        "wmov__tile",
                        isSelected || isFocused ? "wmov__tile--selected" : "",
                        isDragged ? "wmov__tile--dragging" : "",
                      ]
                        .filter(Boolean)
                        .join(" ")}
                      style={{
                        left: item.x * scale,
                        top: item.y * scale,
                        width: Math.max(24, item.w * scale),
                        height: Math.max(24, item.h * scale),
                      }}
                      title={label}
                      onPointerDown={(e) => {
                        if (e.button !== 0) return;
                        e.preventDefault();
                        setSelectedHwnd(item.hwnd);
                        const session: DragSession = {
                          item,
                          startX: e.clientX,
                          startY: e.clientY,
                          x: e.clientX,
                          y: e.clientY,
                          active: false,
                        };
                        dragRef.current = session;
                        setDrag(session);
                      }}
                    >
                      <div className="wmov__tile-thumb">
                        {thumbPath ? (
                          <img src={convertFileSrc(thumbPath)} alt="" draggable={false} />
                        ) : (
                          <AppIcon target={item.icon} fallback={tileGlyph(item)} size={32} />
                        )}
                      </div>
                      <div className="wmov__tile-bar">
                        <AppIcon target={item.icon} fallback={tileGlyph(item)} size={16} />
                        <span className="wmov__tile-title">{label}</span>
                      </div>
                    </div>
                  );
                })}

                {dropSlot && dropSlot.kind !== "empty" && (
                  <div
                    className={`wmov__dropmark ${dropSlot.mark.isBar ? "wmov__dropmark--bar" : ""}`}
                    style={{
                      left: dropSlot.mark.x * scale,
                      top: dropSlot.mark.y * scale,
                      width: Math.max(12, dropSlot.mark.w * scale),
                      height: Math.max(4, dropSlot.mark.h * scale),
                    }}
                  />
                )}
              </>
            )}
          </div>
        </div>

        <div className="wmov__footer">
          <span>Click window to switch · Drag tile to rearrange or onto a dot</span>
        </div>
      </div>

      {drag?.active && (
        <div className="wmov__ghost" style={{ left: drag.x, top: drag.y }}>
          <AppIcon target={drag.item.icon} fallback={tileGlyph(drag.item)} size={18} />
          <span>{tileLabel(drag.item)}</span>
        </div>
      )}
    </div>
  );
}
