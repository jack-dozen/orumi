import { useCallback, useEffect, useRef, useState } from "react";
import type { MediaInfo } from "./useMedia";

export type LyricLine = {
  timeMs: number;
  text: string;
};

export type LyricsResult =
  | { kind: "synced"; lines: LyricLine[] }
  | { kind: "plain"; lines: string[] }
  | { kind: "instrumental" }
  | { kind: "none" };

export type LyricsState = {
  status: "idle" | "loading" | "ready" | "error";
  data: LyricsResult | null;
  activeIndex: number;
  retry: () => void;
};

type LrcLibRecord = {
  trackName?: string;
  artistName?: string;
  duration?: number;
  instrumental?: boolean;
  plainLyrics?: string | null;
  syncedLyrics?: string | null;
};

const CACHE_STORAGE_KEY = "orumi-lyrics-cache-v1";
const MAX_CACHE_ITEMS = 40;
const memCache = new Map<string, LyricsResult>();

function loadStorageCache(): Record<string, LyricsResult> {
  try {
    const raw = localStorage.getItem(CACHE_STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Record<string, LyricsResult>) : {};
  } catch {
    return {};
  }
}

function getCached(key: string): LyricsResult | null {
  const mem = memCache.get(key);
  if (mem) return mem;
  const stored = loadStorageCache()[key];
  if (stored) {
    memCache.set(key, stored);
    return stored;
  }
  return null;
}

function setCached(key: string, value: LyricsResult) {
  memCache.set(key, value);
  try {
    const store = loadStorageCache();
    delete store[key];
    store[key] = value;
    const keys = Object.keys(store);
    while (keys.length > MAX_CACHE_ITEMS) {
      const oldest = keys.shift();
      if (oldest) delete store[oldest];
    }
    localStorage.setItem(CACHE_STORAGE_KEY, JSON.stringify(store));
  } catch {
    /* ignore storage quota errors */
  }
}

/** Clean common video/channel noise from media metadata so LRCLIB matches better. */
function cleanMeta(rawTitle: string, rawArtist: string): { title: string; artist: string } {
  let title = rawTitle
    .replace(
      /[\(\[\【][^)\]\】]*(official|lyric|lyrics|audio|video|mv|hd|4k|visualizer|remaster|explicit)[^)\]\】]*[\)\]\】]/gi,
      "",
    )
    .replace(/\s+\|\s+.*$/, "")
    .trim();

  let artist = rawArtist
    .replace(/\s*-\s*Topic$/i, "")
    .split(/,|&|\bfeat\.|\bft\./i)[0]
    .trim();

  if (!artist || title.toLowerCase().startsWith(`${artist.toLowerCase()} - `)) {
    const dash = title.indexOf(" - ");
    if (dash > 0) {
      if (!artist) artist = title.slice(0, dash).trim();
      title = title.slice(dash + 3).trim();
    }
  }

  return { title: title || rawTitle.trim(), artist };
}

/** Parse LRC timestamped lyrics into sorted { timeMs, text } entries. */
export function parseLrc(lrc: string): LyricLine[] {
  const tagRe = /\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]/g;
  const raw: LyricLine[] = [];

  for (const line of lrc.split(/\r?\n/)) {
    tagRe.lastIndex = 0;
    const times: number[] = [];
    let match: RegExpExecArray | null;
    let lastEnd = 0;
    while ((match = tagRe.exec(line)) !== null) {
      const min = Number(match[1]);
      const sec = Number(match[2]);
      const fracStr = match[3] ?? "0";
      const fracMs =
        fracStr.length === 1
          ? Number(fracStr) * 100
          : fracStr.length === 2
            ? Number(fracStr) * 10
            : Number(fracStr);
      times.push(min * 60_000 + sec * 1000 + fracMs);
      lastEnd = tagRe.lastIndex;
    }
    if (times.length === 0) continue;
    const text = line.slice(lastEnd).trim();
    for (const timeMs of times) {
      raw.push({ timeMs, text });
    }
  }

  raw.sort((a, b) => a.timeMs - b.timeMs);

  // Collapse consecutive empty instrumental markers into a single "♪" break
  const cleaned: LyricLine[] = [];
  for (const item of raw) {
    if (!item.text) {
      if (cleaned.length > 0 && cleaned[cleaned.length - 1].text !== "♪") {
        cleaned.push({ timeMs: item.timeMs, text: "♪" });
      }
    } else {
      cleaned.push(item);
    }
  }
  if (cleaned.length > 0 && cleaned[cleaned.length - 1].text === "♪") {
    cleaned.pop();
  }
  return cleaned;
}

function recordToResult(rec: LrcLibRecord): LyricsResult {
  if (rec.syncedLyrics) {
    const lines = parseLrc(rec.syncedLyrics);
    if (lines.length > 0) return { kind: "synced", lines };
  }
  if (rec.plainLyrics) {
    const lines = rec.plainLyrics
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter((l, i, arr) => l.length > 0 || (i > 0 && arr[i - 1].length > 0));
    if (lines.length > 0) return { kind: "plain", lines };
  }
  if (rec.instrumental) return { kind: "instrumental" };
  return { kind: "none" };
}

function pickBestSearchMatch(list: LrcLibRecord[], durationSec: number): LrcLibRecord | null {
  let best: LrcLibRecord | null = null;
  let bestScore = -Infinity;

  for (const item of list) {
    const hasSynced = Boolean(item.syncedLyrics?.trim());
    const hasPlain = Boolean(item.plainLyrics?.trim());
    if (!hasSynced && !hasPlain && !item.instrumental) continue;

    let diff = 0;
    if (durationSec > 0 && typeof item.duration === "number" && item.duration > 0) {
      diff = Math.abs(item.duration - durationSec);
      if (diff > 18) continue;
    }

    const score = (hasSynced ? 100 : hasPlain ? 40 : 10) - diff;
    if (score > bestScore) {
      bestScore = score;
      best = item;
    }
  }

  return best;
}

async function fetchLyrics(info: MediaInfo, signal: AbortSignal): Promise<LyricsResult> {
  const { title, artist } = cleanMeta(info.title, info.artist);
  if (!title) return { kind: "none" };

  const durationSec = info.duration_ms > 0 ? Math.round(info.duration_ms / 1000) : 0;

  // 1. Exact lookup when artist, album, and duration are available
  if (artist && info.album && durationSec > 0) {
    const params = new URLSearchParams({
      track_name: title,
      artist_name: artist,
      album_name: info.album,
      duration: String(durationSec),
    });
    const res = await fetch(`https://lrclib.net/api/get?${params.toString()}`, { signal });
    if (res.ok) {
      const data = (await res.json()) as LrcLibRecord;
      const parsed = recordToResult(data);
      if (parsed.kind !== "none") return parsed;
    } else if (res.status !== 404) {
      throw new Error(`lrclib ${res.status}`);
    }
  }

  // 2. Structured search fallback (handles slight album/duration mismatches)
  if (artist) {
    const params = new URLSearchParams({ track_name: title, artist_name: artist });
    const res = await fetch(`https://lrclib.net/api/search?${params.toString()}`, { signal });
    if (res.ok) {
      const list = (await res.json()) as LrcLibRecord[];
      const match = Array.isArray(list) ? pickBestSearchMatch(list, durationSec) : null;
      if (match) return recordToResult(match);
    } else {
      throw new Error(`lrclib ${res.status}`);
    }
  }

  // 3. Broad query fallback (useful for browser/YouTube titles without clean artist metadata)
  const q = artist ? `${artist} ${title}` : title;
  const res = await fetch(`https://lrclib.net/api/search?q=${encodeURIComponent(q)}`, { signal });
  if (!res.ok) throw new Error(`lrclib ${res.status}`);
  const list = (await res.json()) as LrcLibRecord[];
  const match = Array.isArray(list) ? pickBestSearchMatch(list, durationSec) : null;
  return match ? recordToResult(match) : { kind: "none" };
}

function findActiveLine(lines: LyricLine[], posMs: number): number {
  const target = posMs + 150;
  let lo = 0;
  let hi = lines.length - 1;
  let ans = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (lines[mid].timeMs <= target) {
      ans = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return ans;
}

/** Fetch and synchronize lyrics with the active media track when `enabled` is true. */
export function useLyrics(info: MediaInfo | null, enabled: boolean): LyricsState {
  const [status, setStatus] = useState<LyricsState["status"]>("idle");
  const [data, setData] = useState<LyricsResult | null>(null);
  const [activeIndex, setActiveIndex] = useState(-1);
  const [attempt, setAttempt] = useState(0);
  const skipCacheRef = useRef(false);

  const trackKey = info
    ? `${info.title}|${info.artist}|${info.album}|${Math.round(info.duration_ms / 1000)}`
    : "";

  const clockRef = useRef({
    trackKey: "",
    baseMs: 0,
    at: performance.now(),
    playing: false,
    behindCount: 0,
  });

  const syncRef = useRef<(() => void) | null>(null);

  // Keep a monotonic local playback clock so small SMTC position jitter never bounces lyrics backward.
  useEffect(() => {
    const now = performance.now();
    const pos = info?.position_ms ?? 0;
    const playing = info?.playing ?? false;
    const c = clockRef.current;

    if (!info || c.trackKey !== trackKey) {
      clockRef.current = { trackKey, baseMs: pos, at: now, playing, behindCount: 0 };
      syncRef.current?.();
      return;
    }

    const estimated = c.playing ? c.baseMs + (now - c.at) : c.baseMs;
    const drift = pos - estimated;

    if (!playing) {
      clockRef.current = { trackKey, baseMs: pos, at: now, playing: false, behindCount: 0 };
      syncRef.current?.();
      return;
    }

    if (!c.playing) {
      // Just resumed playing
      clockRef.current = { trackKey, baseMs: pos, at: now, playing: true, behindCount: 0 };
      syncRef.current?.();
      return;
    }

    if (Math.abs(drift) > 1200) {
      // Genuine seek / skip jump
      clockRef.current = { trackKey, baseMs: pos, at: now, playing: true, behindCount: 0 };
      syncRef.current?.();
    } else if (drift >= 0) {
      // Forward catchup never bounces backward
      clockRef.current = { trackKey, baseMs: pos, at: now, playing: true, behindCount: 0 };
      syncRef.current?.();
    } else if (drift < -350 && c.behindCount >= 1) {
      // Consistently behind for 2+ polls (e.g. player buffered or small backward seek)
      clockRef.current = { trackKey, baseMs: pos, at: now, playing: true, behindCount: 0 };
      syncRef.current?.();
    } else {
      // Small backward SMTC jitter: hold monotonic clock steady
      clockRef.current = {
        trackKey,
        baseMs: estimated,
        at: now,
        playing: true,
        behindCount: drift < -350 ? c.behindCount + 1 : 0,
      };
    }
  }, [trackKey, info?.position_ms, info?.playing]);

  const retry = useCallback(() => {
    skipCacheRef.current = true;
    setAttempt((n) => n + 1);
  }, []);

  useEffect(() => {
    if (!enabled || !info || !trackKey) {
      if (!info) {
        setStatus("idle");
        setData(null);
        setActiveIndex(-1);
      }
      return;
    }

    const skipCache = skipCacheRef.current;
    skipCacheRef.current = false;
    const cached = !skipCache ? getCached(trackKey) : null;
    if (cached) {
      setData(cached);
      setStatus("ready");
      return;
    }

    const ctrl = new AbortController();
    setStatus("loading");
    setData(null);
    setActiveIndex(-1);

    fetchLyrics(info, ctrl.signal)
      .then((res) => {
        if (ctrl.signal.aborted) return;
        setCached(trackKey, res);
        setData(res);
        setStatus("ready");
      })
      .catch((err) => {
        if (ctrl.signal.aborted || err?.name === "AbortError") return;
        setStatus("error");
      });

    return () => ctrl.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, trackKey, attempt]);

  // Keep active lyric index synced smoothly between 1s media polls
  useEffect(() => {
    if (!enabled || data?.kind !== "synced" || data.lines.length === 0) {
      syncRef.current = null;
      setActiveIndex(-1);
      return;
    }

    const lines = data.lines;
    const sync = () => {
      const { baseMs, at, playing } = clockRef.current;
      const pos = playing ? baseMs + (performance.now() - at) : baseMs;
      const idx = findActiveLine(lines, pos);
      setActiveIndex((prev) => (prev === idx ? prev : idx));
    };

    syncRef.current = sync;
    sync();
    if (!info?.playing) return;

    const id = window.setInterval(sync, 100);
    return () => {
      syncRef.current = null;
      window.clearInterval(id);
    };
  }, [enabled, data, info?.playing, trackKey]);

  return { status, data, activeIndex, retry };
}
