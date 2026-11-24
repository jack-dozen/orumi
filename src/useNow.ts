import { useEffect, useState } from "react";

/** Current time, aligned to minute boundaries and updated only while `active` = true. */
export function useNow(active = true): Date {
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    if (!active) return;
    setNow(new Date());
    let timer = 0;
    const schedule = () => {
      const delay = 60_000 - (Date.now() % 60_000) + 50;
      timer = window.setTimeout(() => {
        setNow(new Date());
        schedule();
      }, delay);
    };
    schedule();
    return () => window.clearTimeout(timer);
  }, [active]);

  return now;
}
