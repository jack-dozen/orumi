import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type MediaInfo = {
  title: string;
  artist: string;
  album: string;
  source: string;
  playing: boolean;
  position_ms: number;
  duration_ms: number;
  art_key: string;
  art: string | null;
};

export type MediaAction = "toggle" | "next" | "prev";

export type MediaState = {
  info: MediaInfo | null;
  art: string | null;
  control: (action: MediaAction) => void;
  seek: (ms: number) => void;
};

/** Ambil info media tiap detik, hanya saat `active` = true. */
export function useMedia(active: boolean): MediaState {
  const [info, setInfo] = useState<MediaInfo | null>(null);
  const [art, setArt] = useState<string | null>(null);
  const artKey = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const res = await invoke<MediaInfo | null>("get_media", {
        knownArtKey: artKey.current,
      });
      if (!res) {
        artKey.current = null;
        setArt(null);
      } else if (res.art_key !== artKey.current) {
        artKey.current = res.art_key;
        setArt(res.art);
      }
      setInfo(res);
    } catch {
      /* coba lagi di polling berikutnya */
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    refresh();
    const id = setInterval(refresh, 1000);
    return () => clearInterval(id);
  }, [active, refresh]);

  const control = useCallback(
    (action: MediaAction) => {
      invoke("media_control", { action }).finally(() => setTimeout(refresh, 250));
    },
    [refresh],
  );

  const seek = useCallback(
    (ms: number) => {
      invoke("media_seek", { positionMs: Math.max(0, Math.round(ms)) }).finally(() =>
        setTimeout(refresh, 250),
      );
    },
    [refresh],
  );

  return { info, art, control, seek };
}
