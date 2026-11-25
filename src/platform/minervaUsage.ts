import { useCallback, useEffect } from "react";
import { useStore } from "zustand";
import { loadMinervaUsage, minervaUsageStore, observeMinervaUsage } from "./minervaUsageStore";

export function useMinervaUsage(active = true) {
  const state = useStore(minervaUsageStore);
  useEffect(() => {
    if (active) return observeMinervaUsage();
  }, [active]);
  const refresh = useCallback(() => (active ? loadMinervaUsage(true) : Promise.resolve()), [active]);
  return { ...state, refresh };
}
