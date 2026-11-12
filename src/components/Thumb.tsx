import { useEffect, useState } from "react";

/**
 * Shows a thumbnail. Initial mount uses async image decoding directly; when `src` changes later,
 * the replacement is decoded off-screen first so the image never blinks blank.
 */
export default function Thumb({ src }: { src: string }) {
  const [shown, setShown] = useState(src);

  useEffect(() => {
    if (src === shown) return;
    let cancelled = false;
    const img = new Image();
    img.src = src;
    img.decode().then(
      () => {
        if (!cancelled) setShown(src);
      },
      () => {}, // file missing or unreadable: keep what is shown
    );
    return () => {
      cancelled = true;
    };
  }, [src, shown]);

  return <img src={shown} alt="" decoding="async" draggable={false} />;
}
