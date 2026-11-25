import { describe, expect, it } from "vitest";
import type { MinervaUsageProvider, MinervaUsageWindow } from "@/platform/minervaUsageTypes";
import { selectedSubscriptionUsage, shortestUsageWindow, subscriptionProviderId } from "./subscriptionUsage";

const window = (id: string, windowSeconds: number | null, usedPercent = 0): MinervaUsageWindow => ({
  id,
  windowSeconds,
  usedPercent,
  resetAt: null,
});

describe("selected subscription usage", () => {
  it("uses exact provider identity from the selected route, including nested model names", () => {
    const row = (providerId: string): MinervaUsageProvider => ({
      providerId,
      providerName: "Same name",
      status: "ok",
      windows: [],
      updatedAt: null,
      errorCode: null,
    });
    const providers = [row("openai-codex"), row("opencode-go")];
    expect(selectedSubscriptionUsage(providers, "api:opencode-go/vendor/haiku")).toBe(providers[1]);
    expect(selectedSubscriptionUsage(providers, "api:openai-codex/gpt-6.2-astra")).toBe(providers[0]);
    expect(selectedSubscriptionUsage(providers, "api:other/gpt-6.2-astra")).toBeNull();
    expect(subscriptionProviderId("plugin:agent/model")).toBeNull();
    expect(subscriptionProviderId(null)).toBeNull();
  });

  it("shows the shortest period even when its consumption is zero", () => {
    const windows = [window("monthly", null, 69), window("weekly", 604800, 18), window("5h", 18000, 0)];
    expect(shortestUsageWindow(windows)).toBe(windows[2]);
    expect(shortestUsageWindow(windows.slice(0, 2))).toBe(windows[1]);
    expect(shortestUsageWindow(windows.slice(0, 1))).toBe(windows[0]);
  });

  it("uses actual server durations instead of primary/secondary ordering", () => {
    const windows = [window("primary", 604800, 56), window("secondary", 7200, 12)];
    expect(shortestUsageWindow(windows)).toBe(windows[1]);
    expect(shortestUsageWindow([window("unknown", null), window("weekly", null)])?.id).toBe("weekly");
    expect(shortestUsageWindow([window("long", 90 * 86400), window("monthly", null)])?.id).toBe("monthly");
    expect(shortestUsageWindow([])).toBeNull();
  });
});
