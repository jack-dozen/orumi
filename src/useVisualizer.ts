import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type AudioVizFrame = {
  bands: number[];
  beat: number;
};

/** Tell the Rust WASAPI loopback worker whether any visualizer UI is currently visible. */
export function useVisualizerSource(active: boolean) {
  useEffect(() => {
    invoke("set_visualizer_active", { active }).catch(() => {});
  }, [active]);

  useEffect(() => {
    return () => {
      invoke("set_visualizer_active", { active: false }).catch(() => {});
    };
  }, []);
}

export type VisualizerBarOptions = {
  minScale: number;
  baseOpacity?: number;
  beatOpacityBoost?: number;
  beatLift?: number;
};

/**
 * Drive an array of bar elements directly via requestAnimationFrame from live `audio-viz` events.
 * Resamples the 24 backend frequency bands to `barCount` and optionally pulses `containerRef` opacity on beat.
 */
export function useVisualizerBars(
  active: boolean,
  barCount: number,
  options: VisualizerBarOptions,
) {
  const barRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const targetBands = useRef<number[]>(new Array(barCount).fill(0));
  const currentBands = useRef<number[]>(new Array(barCount).fill(0));
  const lastRendered = useRef<string[]>(new Array(barCount).fill(""));
  const targetBeat = useRef(0);
  const currentBeat = useRef(0);
  const lastOpacity = useRef("");
  const optsRef = useRef(options);
  optsRef.current = options;

  useEffect(() => {
    if (!active) {
      targetBands.current.fill(0);
      currentBands.current.fill(0);
      lastRendered.current.fill("");
      targetBeat.current = 0;
      currentBeat.current = 0;
      lastOpacity.current = "";
      for (let i = 0; i < barCount; i++) {
        const el = barRefs.current[i];
        if (el) el.style.transform = `scaleY(${optsRef.current.minScale})`;
      }
      if (containerRef.current) {
        containerRef.current.style.opacity = "";
      }
      return;
    }

    const unlistenPromise = listen<AudioVizFrame>("audio-viz", (e) => {
      const src = e.payload.bands;
      if (!src || src.length === 0) return;
      const srcLast = src.length - 1;
      const dstLast = Math.max(1, barCount - 1);

      for (let i = 0; i < barCount; i++) {
        const pos = barCount === src.length ? i : (i / dstLast) * srcLast;
        const i0 = Math.floor(pos);
        const i1 = Math.min(srcLast, i0 + 1);
        const frac = pos - i0;
        targetBands.current[i] = src[i0] * (1 - frac) + src[i1] * frac;
      }
      targetBeat.current = e.payload.beat ?? 0;
    });

    let rafId = 0;
    let lastTs = performance.now();

    const tick = (now: number) => {
      const dt = Math.min(0.05, Math.max(0.001, (now - lastTs) / 1000));
      lastTs = now;

      const attackAlpha = 1 - Math.exp(-36 * dt);
      const releaseAlpha = 1 - Math.exp(-16 * dt);

      const tBeat = targetBeat.current;
      currentBeat.current +=
        (tBeat - currentBeat.current) * (tBeat > currentBeat.current ? attackAlpha : releaseAlpha);
      if (currentBeat.current < 0.002 && tBeat === 0) {
        currentBeat.current = 0;
      }

      const { minScale, baseOpacity, beatOpacityBoost = 0, beatLift = 0 } = optsRef.current;
      const span = 1 - minScale;

      for (let i = 0; i < barCount; i++) {
        const t = targetBands.current[i];
        const c = currentBands.current[i];
        const next = c + (t - c) * (t > c ? attackAlpha : releaseAlpha);
        currentBands.current[i] = next < 0.002 && t === 0 ? 0 : next;

        const el = barRefs.current[i];
        if (el) {
          const val = Math.min(1, currentBands.current[i] + currentBeat.current * beatLift);
          const scaleStr = (minScale + val * span).toFixed(3);
          if (lastRendered.current[i] !== scaleStr) {
            lastRendered.current[i] = scaleStr;
            el.style.transform = `scaleY(${scaleStr})`;
          }
        }
      }

      if (containerRef.current && baseOpacity !== undefined) {
        const opStr = (baseOpacity + beatOpacityBoost * currentBeat.current).toFixed(3);
        if (lastOpacity.current !== opStr) {
          lastOpacity.current = opStr;
          containerRef.current.style.opacity = opStr;
        }
      }

      rafId = requestAnimationFrame(tick);
    };

    rafId = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(rafId);
      unlistenPromise.then((fn) => fn());
    };
  }, [active, barCount]);

  return { barRefs, containerRef };
}
