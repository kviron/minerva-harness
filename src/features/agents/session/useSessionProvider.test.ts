import { renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useSessionProvider } from "./useSessionProvider";

const fixture = vi.hoisted(() => ({
  providers: [
    { id: "minerva.api.a", name: "Provider A", installed: true },
    { id: "minerva.api.b", name: "Provider B", installed: true },
    { id: "claude", name: "Claude Code", installed: true },
    { id: "minerva.api.disabled", name: "Unavailable", installed: false },
  ],
  refresh: vi.fn(),
}));
vi.mock("../store", () => ({
  useAgents: (read: (state: { providers: typeof fixture.providers }) => unknown) =>
    read({ providers: fixture.providers }),
  refreshTaskModels: fixture.refresh,
}));
beforeEach(() => fixture.refresh.mockClear());

it("offers installed API providers and refreshes the bound task without changing its model", () => {
  const { result, rerender } = renderHook(({ provider }) => useSessionProvider("task", provider, "api:a/model"), {
    initialProps: { provider: "minerva.api.a" },
  });
  expect(result.current?.options.map((option) => option.value)).toEqual(["minerva.api.a", "minerva.api.b"]);
  expect(result.current?.modelAvailable).toBe(true);
  rerender({ provider: "minerva.api.b" });
  expect(result.current?.modelAvailable).toBe(false);
  expect(result.current?.name).toBe("Provider B");
  expect(fixture.refresh).toHaveBeenCalledTimes(2);
});

it("keeps missing providers identifiable and leaves external engines alone", () => {
  const { result, rerender } = renderHook(({ provider }) => useSessionProvider("task", provider, "api:missing/model"), {
    initialProps: { provider: "minerva.api.missing" },
  });
  expect(result.current?.name).toBe("minerva.api.missing");
  expect(result.current?.modelAvailable).toBe(false);
  rerender({ provider: "claude" });
  expect(result.current).toBeNull();
  expect(fixture.refresh).toHaveBeenCalledTimes(1);
});
