import type { MinervaUsageProvider, MinervaUsageWindow } from "@/platform/minervaUsageTypes";

// Calendar months use their upper bound only for ordering, never as a displayed duration.
const PERIOD_ORDER_SECONDS: Record<string, number> = { "5h": 5 * 3600, weekly: 7 * 86400, monthly: 31 * 86400 };

export function subscriptionProviderId(model: string | null): string | null {
  return model?.match(/^api:([^/]+)\/.+/)?.[1] ?? null;
}

/** Prefer the shortest duration; named periods provide an ordering fallback when duration is absent. */
export function shortestUsageWindow(windows: readonly MinervaUsageWindow[]): MinervaUsageWindow | null {
  const duration = (window: MinervaUsageWindow) =>
    window.windowSeconds !== null && window.windowSeconds > 0
      ? window.windowSeconds
      : (PERIOD_ORDER_SECONDS[window.id] ?? Infinity);
  return windows.reduce<MinervaUsageWindow | null>(
    (shortest, window) => (!shortest || duration(window) < duration(shortest) ? window : shortest),
    null,
  );
}

export function selectedSubscriptionUsage(providers: readonly MinervaUsageProvider[], model: string | null) {
  const id = subscriptionProviderId(model);
  return providers.find((provider) => provider.providerId === id) ?? null;
}
