import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

/**
 * True saat panel dengan label ini sedang ditampilkan oleh Rust.
 *
 * Saat berubah menjadi true, kita menunggu dua frame sungguhan dulu sebelum mengubah
 * tampilan. Tanpa ini, pada pembukaan pertama WebView2 bisa baru menggambar frame
 * setelah animasi selesai, sehingga panel terlihat "loncat" tanpa meluncur.
 * Kalau tidak ada frame dalam 500 ms (WebView sedang tidak menggambar), tetap ditampilkan.
 */
export function useEdgeVisible(label: string): boolean {
  const [requested, setRequested] = useState(false);
  const [shown, setShown] = useState(false);

  useEffect(() => {
    const unlisten = listen<boolean>(`edge-visible:${label}`, (e) => setRequested(e.payload));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [label]);

  useEffect(() => {
    if (!requested) {
      setShown(false);
      return;
    }
    let raf2 = 0;
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => setShown(true));
    });
    const fallback = window.setTimeout(() => setShown(true), 500);
    return () => {
      cancelAnimationFrame(raf1);
      cancelAnimationFrame(raf2);
      window.clearTimeout(fallback);
    };
  }, [requested]);

  return shown;
}
