import { call, commands } from "./ipc";

export interface MinervaProvider {
  id: string;
  name: string;
  baseUrl: string;
  models: string[];
  protocol: string;
  hasKey: boolean;
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
  return JSON.parse(await call(commands.minervaCall(method, JSON.stringify(params)))) as T;
}
