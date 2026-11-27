import type { ModelWindow } from "@/platform/minerva";
export interface ProviderDraft {
  id: string;
  name: string;
  baseUrl: string;
  protocol: string;
  models: string[];
  modelWindows: Record<string, ModelWindow>;
  apiKey: string;
  creditGrants?: boolean;
}
