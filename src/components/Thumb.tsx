import { useEffect, useState } from "react";

/**
 * Shows a thumbnail. The file is decoded off-screen first (also on first mount), so the image is
 * ready when it is painted, and a changed file replaces the old one only once it is decoded.
 */
export default function Thumb({ src }: { src: string }) {
  const [shown, setShown] = useState(src);

  useEffect(() => {
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
  }, [src]);

  return <img src={shown} alt="" draggable={false} />;
}
