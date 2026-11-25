import { useEffect, useState } from "react";
import { minervaCall, type MinervaProvider, type MinervaKnownProvider } from "@/platform/minerva";
export function useProviderConnections() {
  const [providers, setProviders] = useState<MinervaProvider[]>([]);
  const [known, setKnown] = useState<MinervaKnownProvider[]>([]);
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const [removing, setRemoving] = useState(false);
  useEffect(() => {
    let active = true;
    const load = async () => {
      try {
        const [rows, presets] = await Promise.all([
          minervaCall<MinervaProvider[]>("providers.list"),
          minervaCall<MinervaKnownProvider[]>("providers.known"),
        ]);
        if (active) {
          setProviders(rows);
          setKnown(presets);
        }
      } catch (e) {
        if (active) setError(String(e));
      } finally {
        if (active) setLoading(false);
      }
    };
    void load();
    return () => {
      active = false;
    };
  }, []);
  const remove = async (id: string) => {
    setRemoving(true);
    setError("");
    try {
      setProviders(await minervaCall<MinervaProvider[]>("providers.delete", { id }));
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setRemoving(false);
    }
  };
  return { providers, known, error, setProviders, loading, removing, remove };
}
