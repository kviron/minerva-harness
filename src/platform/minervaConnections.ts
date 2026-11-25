/** Connection lifecycle events shared by configuration and account consumers. */
import { tx } from "./i18n";
import { useRemote } from "./remote";
export interface MinervaConnectionChange {
  kind: "credentials" | "catalog";
  phase: "start" | "end";
}

export const minervaConnectionEvents = new EventTarget();
const channel = typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel("minerva-connections-changed");
let mutations = 0;
let generation = 0;
let accountState: string | undefined;
const routes = new Map<string, { baseUrl: string; protocol: string }>();

useRemote.subscribe((state, previous) => {
  if (state.session === previous.session && state.lost === previous.lost) return;
  generation++;
  mutations = 0;
  routes.clear();
  accountState = undefined;
});

export const minervaConnectionsMutating = () => mutations > 0;

function publish(change: MinervaConnectionChange) {
  minervaConnectionEvents.dispatchEvent(new CustomEvent("change", { detail: change }));
  channel?.postMessage(change);
}
channel?.addEventListener("message", ({ data }: MessageEvent<MinervaConnectionChange>) => {
  if (
    data &&
    (data.kind === "credentials" || data.kind === "catalog") &&
    (data.phase === "start" || data.phase === "end")
  )
    minervaConnectionEvents.dispatchEvent(new CustomEvent("change", { detail: data }));
});

function mutationKind(method: string, params: Record<string, unknown>): MinervaConnectionChange["kind"] | null {
  if (method === "codex.signIn" || method === "codex.signOut" || method === "providers.delete") return "credentials";
  if (method === "codex.configure") return "catalog";
  if (method !== "providers.save") return null;
  const route = typeof params.id === "string" ? routes.get(params.id) : undefined;
  return params.isNew ||
    (typeof params.apiKey === "string" && params.apiKey.trim()) ||
    !route ||
    route.baseUrl !== params.baseUrl ||
    route.protocol !== params.protocol
    ? "credentials"
    : "catalog";
}

function observeResult(method: string, result: unknown) {
  if (
    ["providers.list", "providers.save", "providers.delete", "codex.configure"].includes(method) &&
    Array.isArray(result)
  ) {
    routes.clear();
    for (const provider of result) {
      if (
        typeof provider?.id === "string" &&
        typeof provider.baseUrl === "string" &&
        typeof provider.protocol === "string"
      )
        routes.set(provider.id, { baseUrl: provider.baseUrl, protocol: provider.protocol });
    }
  }
  if (method === "codex.status" || method === "codex.signIn" || method === "codex.signOut") {
    const state = (result as { state?: unknown } | null)?.state;
    if (typeof state !== "string") return;
    const changed = accountState !== undefined && accountState !== state;
    accountState = state;
    if (changed) publish({ kind: "credentials", phase: "end" });
  }
}

/** The facade reports domain changes; subscribers decide how to invalidate their own data. */
export async function trackMinervaConnectionOperation<T>(
  method: string,
  params: Record<string, unknown>,
  request: () => Promise<T>,
): Promise<T> {
  const expected = generation;
  const kind = mutationKind(method, params);
  if (kind === "credentials") {
    mutations++;
    publish({ kind, phase: "start" });
  }
  try {
    const result = await request();
    if (expected !== generation) throw new Error(tx("Minerva connection changed during the request."));
    observeResult(method, result);
    if (kind === "catalog") publish({ kind, phase: "end" });
    return result;
  } finally {
    if (kind === "credentials" && expected === generation) {
      mutations--;
      publish({ kind, phase: "end" });
    }
  }
}
