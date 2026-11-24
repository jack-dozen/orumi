import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";

/** target -> image address (null = none). Persists while the app runs. */
const cache = new Map<string, string | null>();
const pending = new Map<string, Promise<string | null>>();

function load(target: string): Promise<string | null> {
  const inflight = pending.get(target);
  if (inflight) return inflight;
  const p = invoke<string | null>("app_icon", { target })
    .then((path) => (path ? convertFileSrc(path) : null))
    .catch(() => null)
    .then((url) => {
      cache.set(target, url);
      return url;
    });
  pending.set(target, p);
  return p;
}

/** Shared IntersectionObserver per window so lazy icon lists don't allocate hundreds of observers. */
const lazyCallbacks = new WeakMap<Element, () => void>();
let sharedObserver: IntersectionObserver | null = null;

function observeLazy(el: Element, onVisible: () => void): () => void {
  if (typeof IntersectionObserver === "undefined") {
    onVisible();
    return () => {};
  }
  if (!sharedObserver) {
    sharedObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const cb = lazyCallbacks.get(entry.target);
          if (cb) {
            lazyCallbacks.delete(entry.target);
            sharedObserver?.unobserve(entry.target);
            cb();
          }
        }
      },
      { rootMargin: "200px 0px" },
    );
  }
  lazyCallbacks.set(el, onVisible);
  sharedObserver.observe(el);
  return () => {
    lazyCallbacks.delete(el);
    sharedObserver?.unobserve(el);
  };
}

/**
 * App icon. target: "aumid:<AppID>" or "path:<.exe path>".
 * Show the `fallback` letter until the icon is available (or if fetching it fails).
 */
export default function AppIcon({
  target,
  fallback,
  size,
  className,
  lazy = false,
}: {
  target?: string | null;
  fallback: string;
  size: number;
  className?: string;
  lazy?: boolean;
}) {
  const [url, setUrl] = useState<string | null | undefined>(
    target ? cache.get(target) : null,
  );
  const [inView, setInView] = useState(!lazy || (target ? cache.has(target) : true));
  const elRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!lazy || !target || cache.has(target)) {
      setInView(true);
      return;
    }
    const el = elRef.current;
    if (!el) {
      setInView(true);
      return;
    }
    setInView(false);
    return observeLazy(el, () => setInView(true));
  }, [lazy, target]);

  useEffect(() => {
    if (!target) {
      setUrl(null);
      return;
    }
    const known = cache.get(target);
    if (known !== undefined) {
      setUrl(known);
      return;
    }
    if (!inView) {
      setUrl(undefined);
      return;
    }
    let alive = true;
    load(target).then((u) => {
      if (alive) setUrl(u);
    });
    return () => {
      alive = false;
    };
  }, [target, inView]);

  const box = { width: size, height: size };
  if (url) {
    return (
      <img
        className={`appimg ${className ?? ""}`}
        src={url}
        alt=""
        decoding="async"
        draggable={false}
        style={box}
      />
    );
  }
  return (
    <div
      ref={elRef}
      className={`appimg appimg--letter ${className ?? ""}`}
      style={{ ...box, fontSize: size * 0.5 }}
    >
      {fallback}
    </div>
  );
}
