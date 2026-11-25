/** One visible-window polling lifecycle, shared by every usage consumer. */
export function createUsagePolling(load: () => Promise<void>, intervalMs: number) {
  let consumers = 0;
  let timer: ReturnType<typeof setInterval> | null = null;
  const refreshVisible = () => {
    if (consumers > 0 && document.visibilityState !== "hidden") void load();
  };
  const observe = () => {
    if (++consumers === 1) {
      document.addEventListener("visibilitychange", refreshVisible);
      timer = setInterval(refreshVisible, intervalMs);
      refreshVisible();
    }
    let active = true;
    return () => {
      if (!active) return;
      active = false;
      if (--consumers === 0) {
        if (timer !== null) clearInterval(timer);
        timer = null;
        document.removeEventListener("visibilitychange", refreshVisible);
      }
    };
  };
  return { observe, refreshVisible };
}
