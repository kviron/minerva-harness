import { call, commands } from "./ipc";

export interface MinervaProvider {
  id: string;
  name: string;
  baseUrl: string;
  models: string[];
  protocol: string;
  hasKey: boolean;
}
export interface MinervaPlugin {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  canToggle: boolean;
  origin: string;
  hasUi: boolean;
  error: string | null;
}
export interface MinervaWorkspace {
  id: string;
  title: string;
  path: string | null;
  sessionIds: string[];
}
export async function minervaCall<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  return JSON.parse(await call(commands.minervaCall(method, JSON.stringify(params)))) as T;
}
export async function minervaWorkspace(): Promise<MinervaWorkspace> {
  return JSON.parse(await call(commands.minervaWorkspace())) as MinervaWorkspace;
}
