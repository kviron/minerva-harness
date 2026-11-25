/** Public subscription observations; credentials and transport state remain backend-only. */
export interface MinervaUsageWindow {
  id: string;
  windowSeconds: number | null;
  usedPercent: number;
  resetAt: number | null;
}

export interface MinervaUsageProvider {
  providerId: string;
  providerName: string;
  status: "ok" | "not-configured" | "unavailable" | "unsupported";
  windows: MinervaUsageWindow[];
  updatedAt: number | null;
  errorCode: null | "auth" | "timeout" | "rate-limited" | "network" | "invalid-response";
}
