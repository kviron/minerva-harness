import { createStore } from "zustand/vanilla";
import { useRemote } from "./remote";
import { minervaCall } from "./minerva";
import {
  minervaConnectionEvents,
  minervaConnectionsMutating,
  type MinervaConnectionChange,
} from "./minervaConnections";
import { createUsagePolling } from "./minervaUsagePolling";
import type { MinervaUsageProvider } from "./minervaUsageTypes";

interface UsageState {
  providers: MinervaUsageProvider[];
  loading: boolean;
  error: boolean;
}
export const minervaUsageStore = createStore<UsageState>(() => ({ providers: [], loading: false, error: false }));
const TTL_MS = 60_000;
const MAX_SAMPLE_AGE_MS = 15 * 60_000;
let generation = 0;
let settledAt = 0;
let pending: Promise<void> | null = null;
const polling = createUsagePolling(() => loadMinervaUsage(), TTL_MS);
let changingRuntime = false;
let runtimeVersion = 0;

export function loadMinervaUsage(refresh = false): Promise<void> {
  if (minervaConnectionsMutating() || changingRuntime || useRemote.getState().lost !== null) return Promise.resolve();
  if (pending) return pending;
  if (!refresh && settledAt > 0 && Date.now() - settledAt < TTL_MS) return Promise.resolve();
  const expected = generation;
  minervaUsageStore.setState({ loading: true });
  const request = async () => {
    try {
      const rows = await minervaCall<MinervaUsageProvider[]>("usage.list", { refresh });
      if (generation !== expected) return;
      minervaUsageStore.setState({ providers: rows.filter((row) => row.status !== "unsupported"), error: false });
    } catch {
      if (generation === expected)
        minervaUsageStore.setState((state) => ({
          error: true,
          providers: state.providers.filter(
            (row) =>
              row.updatedAt !== null && Date.now() >= row.updatedAt && Date.now() - row.updatedAt <= MAX_SAMPLE_AGE_MS,
          ),
        }));
    } finally {
      if (generation === expected) {
        settledAt = Date.now();
        pending = null;
        minervaUsageStore.setState({ loading: false });
      }
    }
  };
  pending = request();
  return pending;
}

export function invalidateMinervaUsage() {
  generation++;
  settledAt = 0;
  pending = null;
  minervaUsageStore.setState({ providers: [], loading: false, error: false });
}
useRemote.subscribe((state, previous) => {
  if (state.session === previous.session && state.lost === previous.lost) return;
  changingRuntime = true;
  invalidateMinervaUsage();
  const expectedRuntime = ++runtimeVersion;
  // enableRemote publishes session state before installing its IPC route.
  queueMicrotask(() => {
    if (runtimeVersion !== expectedRuntime) return;
    changingRuntime = false;
    polling.refreshVisible();
  });
});
minervaConnectionEvents.addEventListener("change", (event) => {
  const change = (event as CustomEvent<MinervaConnectionChange>).detail;
  if (change.kind === "catalog") {
    const expected = generation;
    void (pending ?? Promise.resolve()).then(() => {
      if (generation !== expected) return;
      settledAt = 0;
      polling.refreshVisible();
    });
    return;
  }
  invalidateMinervaUsage();
  if (change.phase === "end") polling.refreshVisible();
});

export const observeMinervaUsage = polling.observe;
