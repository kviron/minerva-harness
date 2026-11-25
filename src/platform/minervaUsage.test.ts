import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { minervaCall } from "./minerva";
import { useMinervaUsage } from "./minervaUsage";
import type { MinervaUsageProvider } from "./minervaUsageTypes";
import { useRemote } from "./remote";
import { invalidateMinervaUsage } from "./minervaUsageStore";
import { minervaConnectionEvents } from "./minervaConnections";
const connectionChange = (phase: "start" | "end", kind = "credentials") =>
  minervaConnectionEvents.dispatchEvent(new CustomEvent("change", { detail: { phase, kind } }));
vi.mock("./minerva", () => ({ minervaCall: vi.fn() }));
const row: MinervaUsageProvider = {
  providerId: "openai-codex",
  providerName: "Codex",
  status: "ok",
  windows: [{ id: "primary", usedPercent: 35, windowSeconds: 7200, resetAt: 1900000000000 }],
  updatedAt: Date.now(),
  errorCode: null,
};
beforeEach(async () => {
  useRemote.setState({ session: null, lost: null, reconnecting: false });
  await Promise.resolve();
  invalidateMinervaUsage();
  vi.clearAllMocks();
});
it("keeps last known values and observation time on transport failure", async () => {
  vi.mocked(minervaCall).mockResolvedValueOnce([row]).mockRejectedValueOnce(new Error("secret detail"));
  const { result } = renderHook(() => useMinervaUsage());
  await waitFor(() => expect(result.current.providers).toEqual([row]));
  await act(() => result.current.refresh());
  expect(result.current.providers).toEqual([row]);
  expect(result.current.error).toBe(true);
});
it("rejects an old account's in-flight response", async () => {
  let resolve!: (rows: MinervaUsageProvider[]) => void;
  vi.mocked(minervaCall)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolve = r as typeof resolve;
        }),
    )
    .mockResolvedValueOnce([{ ...row, status: "not-configured", windows: [] }]);
  const { result } = renderHook(() => useMinervaUsage());
  act(() => connectionChange("start"));
  await act(async () => resolve([row]));
  expect(result.current.providers).toEqual([]);
  act(() => connectionChange("end"));
  await waitFor(() => expect(result.current.providers[0]?.status).toBe("not-configured"));
});
it("reopens warm data immediately without another IPC request", async () => {
  vi.mocked(minervaCall).mockResolvedValue([row]);
  const { result, rerender } = renderHook(({ active }) => useMinervaUsage(active), { initialProps: { active: true } });
  await waitFor(() => expect(result.current.providers).toEqual([row]));
  rerender({ active: false });
  rerender({ active: true });
  expect(result.current.providers).toEqual([row]);
  expect(result.current.loading).toBe(false);
  expect(minervaCall).toHaveBeenCalledTimes(1);
});
it("deduplicates requests across two consumers", async () => {
  let resolve!: (rows: MinervaUsageProvider[]) => void;
  vi.mocked(minervaCall).mockImplementationOnce(
    () =>
      new Promise((r) => {
        resolve = r as typeof resolve;
      }),
  );
  const first = renderHook(() => useMinervaUsage());
  const second = renderHook(() => useMinervaUsage());
  expect(minervaCall).toHaveBeenCalledTimes(1);
  await act(async () => resolve([row]));
  expect(first.result.current.providers).toEqual([row]);
  expect(second.result.current.providers).toEqual([row]);
});
it("clears snapshots on connection changes even with no mounted consumer", async () => {
  vi.mocked(minervaCall)
    .mockResolvedValueOnce([row])
    .mockImplementationOnce(() => new Promise(() => {}));
  const first = renderHook(() => useMinervaUsage());
  await waitFor(() => expect(first.result.current.providers).toEqual([row]));
  first.unmount();
  act(() => {
    connectionChange("start");
    connectionChange("end");
  });
  const second = renderHook(() => useMinervaUsage());
  expect(second.result.current.providers).toEqual([]);
});
it("uses authoritative backend snapshots and never resurrects expired or unauthorized data", async () => {
  vi.mocked(minervaCall)
    .mockResolvedValueOnce([row])
    .mockResolvedValueOnce([{ ...row, status: "unavailable", windows: [], updatedAt: null, errorCode: "network" }])
    .mockResolvedValueOnce([{ ...row, status: "unavailable", windows: [], updatedAt: null, errorCode: "auth" }]);
  const { result } = renderHook(() => useMinervaUsage());
  await waitFor(() => expect(result.current.providers).toEqual([row]));
  await act(() => result.current.refresh());
  expect(result.current.providers[0]).toMatchObject({
    status: "unavailable",
    windows: [],
    updatedAt: null,
  });
  await act(() => result.current.refresh());
  expect(result.current.providers[0]?.windows).toEqual([]);
});

it("keeps counters visible while catalog metadata is refreshed", async () => {
  let finish!: (rows: MinervaUsageProvider[]) => void;
  vi.mocked(minervaCall)
    .mockResolvedValueOnce([row])
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve as typeof finish;
        }),
    );
  const { result } = renderHook(() => useMinervaUsage());
  await waitFor(() => expect(result.current.providers).toEqual([row]));
  await act(async () => connectionChange("end", "catalog"));
  expect(result.current.providers).toEqual([row]);
  expect(result.current.loading).toBe(true);
  await act(async () => finish([{ ...row, providerName: "Renamed Codex" }]));
  expect(result.current.providers[0]?.providerName).toBe("Renamed Codex");
  expect(result.current.providers[0]?.updatedAt).toBe(row.updatedAt);
});

const remoteSession = {
  target: { kind: "ssh" as const, config: "test" },
  root: "/project",
  host: "test",
  os: "linux",
  commands: ["minerva_call"],
};
it("invalidates warm local usage before remote route installation and reloads on the next microtask", async () => {
  vi.mocked(minervaCall)
    .mockResolvedValueOnce([row])
    .mockResolvedValueOnce([{ ...row, providerName: "Remote Codex" }]);
  const { result } = renderHook(() => useMinervaUsage());
  await waitFor(() => expect(result.current.providers).toEqual([row]));
  act(() => {
    useRemote.setState({ session: remoteSession });
    expect(minervaCall).toHaveBeenCalledTimes(1);
  });
  expect(result.current.providers).toEqual([]);
  await waitFor(() => expect(result.current.providers[0]?.providerName).toBe("Remote Codex"));
});
it("drops old remote results after switching back to local and does not load while disconnected", async () => {
  await act(async () => useRemote.setState({ session: remoteSession }));
  let resolve!: (rows: MinervaUsageProvider[]) => void;
  vi.mocked(minervaCall)
    .mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolve = r as typeof resolve;
        }),
    )
    .mockResolvedValueOnce([row]);
  const { result } = renderHook(() => useMinervaUsage());
  await act(async () => useRemote.setState({ lost: "test" }));
  await act(async () => result.current.refresh());
  expect(minervaCall).toHaveBeenCalledTimes(1);
  await act(async () => resolve([{ ...row, providerName: "Old Remote" }]));
  expect(result.current.providers).toEqual([]);
  await act(async () => useRemote.setState({ session: null, lost: null }));
  await waitFor(() => expect(result.current.providers).toEqual([row]));
});
