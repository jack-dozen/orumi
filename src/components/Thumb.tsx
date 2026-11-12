import { useEffect, useState } from "react";

/** Shows a thumbnail; a changed file is preloaded first so the image never blinks blank. */
export default function Thumb({ src }: { src: string }) {
  const [shown, setShown] = useState(src);

  useEffect(() => {
    if (src === shown) return;
    let cancelled = false;
    const img = new Image();
    img.onload = () => {
      if (!cancelled) setShown(src);
    };
    img.src = src;
    return () => {
      cancelled = true;
    };
  }, [src, shown]);

  return <img src={shown} alt="" draggable={false} />;
}
