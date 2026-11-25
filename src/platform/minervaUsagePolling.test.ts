import { afterEach, expect, it, vi } from "vitest";
import { createUsagePolling } from "./minervaUsagePolling";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("owns one timer until the last consumer leaves", () => {
  vi.useFakeTimers();
  const load = vi.fn(async () => {});
  const polling = createUsagePolling(load, 60_000);
  const first = polling.observe();
  const second = polling.observe();
  expect(load).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(1);
  first();
  vi.advanceTimersByTime(60_000);
  expect(load).toHaveBeenCalledTimes(2);
  second();
  second();
  expect(vi.getTimerCount()).toBe(0);
  document.dispatchEvent(new Event("visibilitychange"));
  expect(load).toHaveBeenCalledTimes(2);
});

it("suspends hidden refreshes and loads once when visible", () => {
  vi.useFakeTimers();
  const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
  const load = vi.fn(async () => {});
  const polling = createUsagePolling(load, 60_000);
  const stop = polling.observe();
  vi.advanceTimersByTime(120_000);
  expect(load).not.toHaveBeenCalled();
  visibility.mockReturnValue("visible");
  document.dispatchEvent(new Event("visibilitychange"));
  expect(load).toHaveBeenCalledTimes(1);
  stop();
});
