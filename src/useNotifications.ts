import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type NotifEntry = {
  id: number;
  app: string;
  aumid: string;
  title: string;
  body: string;
  arrived_ms: number;
  image: string | null;
  icon_target: string | null;
};

const TOAST_DURATION_MS = 5000;
const TOAST_EXIT_MS = 320;
const MAX_QUEUE = 4;

export function useNotifications(enabled: boolean, popupEnabled: boolean, notchVisible: boolean) {
  const [items, setItems] = useState<NotifEntry[]>([]);
  const [activeToast, setActiveToast] = useState<NotifEntry | null>(null);
  const [toastOpen, setToastOpen] = useState(false);

  const queueRef = useRef<NotifEntry[]>([]);
  const activeRef = useRef<NotifEntry | null>(null);
  const hoveredRef = useRef(false);
  const timerRef = useRef<number | undefined>(undefined);
  const exitTimerRef = useRef<number | undefined>(undefined);
  const notchVisibleRef = useRef(notchVisible);
  notchVisibleRef.current = notchVisible;

  const clearTimers = useCallback(() => {
    window.clearTimeout(timerRef.current);
    window.clearTimeout(exitTimerRef.current);
  }, []);

  const showNext = useCallback(() => {
    clearTimers();
    const next = queueRef.current.shift() ?? null;
    activeRef.current = next;
    setActiveToast(next);
    if (!next) {
      setToastOpen(false);
      return;
    }
    setToastOpen(false);
    window.requestAnimationFrame(() => {
      if (activeRef.current?.id === next.id && !notchVisibleRef.current) {
        setToastOpen(true);
      }
    });
    if (!hoveredRef.current) {
      timerRef.current = window.setTimeout(() => {
        setToastOpen(false);
        exitTimerRef.current = window.setTimeout(() => {
          showNext();
        }, TOAST_EXIT_MS);
      }, TOAST_DURATION_MS);
    }
  }, [clearTimers]);

  const hideToastNow = useCallback(() => {
    clearTimers();
    queueRef.current = [];
    activeRef.current = null;
    setToastOpen(false);
    setActiveToast(null);
  }, [clearTimers]);

  // Hide popup immediately if full notch opens or popups are turned off.
  useEffect(() => {
    if (notchVisible || !enabled || !popupEnabled) {
      hideToastNow();
    }
  }, [notchVisible, enabled, popupEnabled, hideToastNow]);

  useEffect(() => {
    if (!enabled) {
      setItems([]);
      return;
    }
    let alive = true;
    invoke<NotifEntry[]>("notif_list")
      .then((list) => {
        if (alive) setItems(list);
      })
      .catch(() => {});

    const unlistenChanged = listen<NotifEntry[]>("notif-changed", (e) => {
      if (!alive) return;
      setItems(e.payload);
      // Drop any queued/active toast that was already marked read/expired.
      const unreadIds = new Set(e.payload.map((x) => x.id));
      queueRef.current = queueRef.current.filter((x) => unreadIds.has(x.id));
      if (activeRef.current && !unreadIds.has(activeRef.current.id)) {
        showNext();
      }
    });

    const unlistenToast = listen<NotifEntry>("notif-toast", (e) => {
      if (!alive || !popupEnabled || notchVisibleRef.current) return;
      const incoming = e.payload;
      if (activeRef.current?.id === incoming.id) return;
      queueRef.current = queueRef.current.filter((x) => x.id !== incoming.id);
      queueRef.current.push(incoming);
      if (queueRef.current.length > MAX_QUEUE) {
        queueRef.current.shift();
      }
      if (!activeRef.current) {
        showNext();
      }
    });

    return () => {
      alive = false;
      clearTimers();
      unlistenChanged.then((fn) => fn());
      unlistenToast.then((fn) => fn());
    };
  }, [enabled, popupEnabled, clearTimers, showNext]);

  const setToastHovered = useCallback(
    (hovered: boolean) => {
      hoveredRef.current = hovered;
      if (!activeRef.current) return;
      if (hovered) {
        clearTimers();
        setToastOpen(true);
      } else {
        clearTimers();
        timerRef.current = window.setTimeout(() => {
          setToastOpen(false);
          exitTimerRef.current = window.setTimeout(() => {
            showNext();
          }, TOAST_EXIT_MS);
        }, 2500);
      }
    },
    [clearTimers, showNext],
  );

  const dismissToast = useCallback(
    (markAsRead: boolean) => {
      const cur = activeRef.current;
      if (!cur) return;
      if (markAsRead) {
        invoke("notif_mark_read", { id: cur.id }).catch(() => {});
      }
      setToastOpen(false);
      clearTimers();
      exitTimerRef.current = window.setTimeout(() => {
        showNext();
      }, TOAST_EXIT_MS);
    },
    [clearTimers, showNext],
  );

  const markRead = useCallback((id: number) => {
    invoke("notif_mark_read", { id }).catch(() => {});
  }, []);

  const clearAll = useCallback(() => {
    invoke("notif_clear_all").catch(() => {});
  }, []);

  const openNotif = useCallback(
    (id: number) => {
      if (activeRef.current?.id === id) {
        hideToastNow();
      }
      invoke("notif_open", { id }).catch(() => {});
    },
    [hideToastNow],
  );

  return {
    items,
    unreadCount: items.length,
    activeToast,
    toastOpen,
    setToastHovered,
    dismissToast,
    markRead,
    clearAll,
    openNotif,
  };
}
