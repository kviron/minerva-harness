import type { ModelWindow } from "../../vendor/minerva/packages/client/src/generated";
import { trackMinervaConnectionOperation } from "./minervaConnections";
import { call, commands } from "./ipc";

export interface MinervaCodexStatus {
  state: "signed-out" | "signing-in" | "signed-in" | "signing-out" | "error";
  authorizationUrl?: string;
  error?: string;
}

export type { CatalogModel, ModelWindow } from "../../vendor/minerva/packages/client/src/generated";

export interface MinervaProvider {
  id: string;
  name: string;
  baseUrl: string;
  models: string[];
  protocol: string;
  hasKey: boolean;
  creditGrants?: boolean;
  modelWindows?: Record<string, ModelWindow>;
}
export interface MinervaKnownProvider {
  id: string;
  name: string;
  baseUrl: string;
  protocol: string;
  models: string[];
  envVar: string | null;
}
export async function minervaCall<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  return trackMinervaConnectionOperation(
    method,
    params,
    async () => JSON.parse(await call(commands.minervaCall(method, JSON.stringify(params)))) as T,
  );
}
