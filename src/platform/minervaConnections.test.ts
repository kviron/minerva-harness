import { expect, it, vi } from "vitest";
import { useRemote } from "./remote";
import {
  minervaConnectionEvents,
  minervaConnectionsMutating,
  trackMinervaConnectionOperation,
} from "./minervaConnections";

it("publishes catalog changes for models and names without a credential boundary", async () => {
  const events = vi.fn();
  minervaConnectionEvents.addEventListener("change", events);
  const provider = { id: "opencode-go", baseUrl: "https://opencode.ai/inference/go", protocol: "openai-chat" };
  try {
    await trackMinervaConnectionOperation("providers.list", {}, async () => [provider]);
    for (const method of ["codex.configure", "providers.save"]) {
      events.mockClear();
      await trackMinervaConnectionOperation(method, { ...provider, apiKey: null, models: ["new"] }, async () => [
        provider,
      ]);
      expect(events).toHaveBeenCalledTimes(1);
      expect(events.mock.calls[0]![0].detail).toEqual({ kind: "catalog", phase: "end" });
      expect(minervaConnectionsMutating()).toBe(false);
    }
  } finally {
    minervaConnectionEvents.removeEventListener("change", events);
  }
});

it("holds the credential boundary through key writes and releases it on failure", async () => {
  const events = vi.fn();
  minervaConnectionEvents.addEventListener("change", events);
  try {
    await expect(
      trackMinervaConnectionOperation("providers.save", { id: "opencode-go", apiKey: "new-key" }, async () => {
        expect(minervaConnectionsMutating()).toBe(true);
        throw new Error("write failed");
      }),
    ).rejects.toThrow("write failed");
    expect(minervaConnectionsMutating()).toBe(false);
    expect(events.mock.calls.map(([event]) => event.detail)).toEqual([
      { kind: "credentials", phase: "start" },
      { kind: "credentials", phase: "end" },
    ]);
  } finally {
    minervaConnectionEvents.removeEventListener("change", events);
  }
});

it("rejects old runtime metadata and never uses it to classify a new connection", async () => {
  const provider = { id: "opencode-go", baseUrl: "https://opencode.ai/inference/go", protocol: "openai-chat" };
  let finish!: (providers: (typeof provider)[]) => void;
  const oldRequest = trackMinervaConnectionOperation(
    "providers.list",
    {},
    () =>
      new Promise<(typeof provider)[]>((resolve) => {
        finish = resolve;
      }),
  );
  const rejection = expect(oldRequest).rejects.toThrow("Minerva connection changed");
  useRemote.setState({
    session: { target: { kind: "ssh", config: "audit" }, root: "/project", host: "audit", os: "linux", commands: [] },
  });
  finish([provider]);
  await rejection;
  const events = vi.fn();
  minervaConnectionEvents.addEventListener("change", events);
  try {
    await trackMinervaConnectionOperation("providers.save", { ...provider, apiKey: null }, async () => [provider]);
    expect(events.mock.calls[0]![0].detail).toEqual({ kind: "credentials", phase: "start" });
  } finally {
    minervaConnectionEvents.removeEventListener("change", events);
    useRemote.setState({ session: null });
  }
});
