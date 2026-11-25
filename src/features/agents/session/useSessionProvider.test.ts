import type { AgentConfigOption } from "@/bindings";
import { renderHook } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { useSessionProvider } from "./useSessionProvider";

const fixture = vi.hoisted(() => ({
  providers: [
    { id: "minerva", name: "Minerva", installed: true },
    { id: "codex", name: "Codex", installed: true },
  ],
  refresh: vi.fn(),
}));
vi.mock("../store", () => ({
  useAgents: (read: (state: { providers: typeof fixture.providers }) => unknown) =>
    read({ providers: fixture.providers }),
  refreshTaskModels: fixture.refresh,
}));
beforeEach(() => fixture.refresh.mockClear());
const config: AgentConfigOption[] = [
  {
    id: "model",
    name: "Model",
    description: null,
    category: "model",
    current: "api:a/one",
    via: "option",
    values: [
      { value: "api:a/one", name: "One", description: null },
      { value: "api:openai-codex/two", name: "Two", description: null },
    ],
  },
];
it.each(["minerva", "minerva.api.a", "minerva.api.removed"])(
  "shows unified Minerva for %s without changing session identity",
  (id) => {
    const { result } = renderHook(() => useSessionProvider("task", id, "api:openai-codex/two", config));
    expect(result.current).toEqual({
      value: "minerva",
      name: "Minerva",
      options: [{ value: "minerva", label: "Minerva" }],
      modelAvailable: true,
    });
    expect(fixture.refresh).toHaveBeenCalledExactlyOnceWith("task", false);
  },
);
it("uses actual model options and waits for metadata before declaring a route missing", () => {
  const { result, rerender } = renderHook(
    ({ options }) => useSessionProvider("task", "minerva.api.a", "api:missing/model", options),
    { initialProps: { options: [] as AgentConfigOption[] } },
  );
  expect(result.current?.modelAvailable).toBe(true);
  rerender({ options: config });
  expect(result.current?.modelAvailable).toBe(false);
});
it("leaves native Codex and other external engines unchanged", () => {
  const { result } = renderHook(() => useSessionProvider("task", "codex", "model", config));
  expect(result.current).toBeNull();
  expect(fixture.refresh).not.toHaveBeenCalled();
});

it("requires a model when neither a selected route nor catalog metadata exists", () => {
  const { result } = renderHook(() => useSessionProvider("task", "minerva", null, []));
  expect(result.current?.modelAvailable).toBe(false);
});
