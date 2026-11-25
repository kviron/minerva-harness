import { useEffect, useRef, useState } from "react";
import { minervaCall, type MinervaCodexStatus, type MinervaProvider } from "@/platform/minerva";
import { openExternalUrl } from "@/platform/openUrl";

export function useCodexSettings(configuredModels: string[], onSaved?: (providers: MinervaProvider[]) => void) {
  const [status, setStatus] = useState<MinervaCodexStatus>({ state: "signed-out" });
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  const [catalog, setCatalog] = useState<string[]>([]);
  const [models, setModels] = useState<string[]>(configuredModels);
  const mounted = useRef(false);
  const openedUrl = useRef<string | undefined>(undefined);
  useEffect(() => {
    if (status.state !== "signing-in") {
      openedUrl.current = undefined;
      return;
    }
    const url = status.authorizationUrl;
    if (!url || openedUrl.current === url) return;
    openedUrl.current = url;
    void openExternalUrl(url).catch((e) => {
      if (mounted.current) setError(String(e));
    });
  }, [status.state, status.authorizationUrl]);
  useEffect(() => {
    mounted.current = true;
    void minervaCall<MinervaCodexStatus>("codex.status")
      .then((account) => {
        if (!mounted.current) return;
        setStatus(account);
      })
      .catch((e) => {
        if (mounted.current) setError(String(e));
      })
      .finally(() => {
        if (mounted.current) setLoading(false);
      });
    return () => {
      mounted.current = false;
    };
  }, []);
  useEffect(() => {
    if (status.state !== "signing-in" && status.state !== "signing-out") return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await minervaCall<MinervaCodexStatus>("codex.status");
        if (!active) return;
        setStatus(next);
        if (next.state === "signing-in" || next.state === "signing-out") timer = setTimeout(() => void poll(), 1500);
      } catch (e) {
        if (active) {
          setError(String(e));
          setStatus({ state: "error" });
        }
      }
    };
    timer = setTimeout(() => void poll(), 1500);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [status.state]);
  const action = async (run: () => Promise<void>) => {
    setBusy(true);
    setError("");
    setSaved(false);
    try {
      await run();
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setBusy(false);
    }
  };
  return {
    status,
    busy: busy || loading || status.state === "signing-out",
    error: error || status.error,
    saved,
    catalog,
    models,
    setModels: (next: string[]) => {
      setModels(next);
      setSaved(false);
    },
    signIn: () =>
      action(async () => {
        const next = await minervaCall<MinervaCodexStatus>("codex.signIn");
        if (!mounted.current) return;
        setStatus(next);
      }),
    signOut: () =>
      action(async () => {
        const next = await minervaCall<MinervaCodexStatus>("codex.signOut");
        if (mounted.current) setStatus(next);
      }),
    fetchModels: () =>
      action(async () => {
        const next = await minervaCall<string[]>("codex.models");
        if (mounted.current) setCatalog([...new Set(next)]);
      }),
    save: () =>
      action(async () => {
        const providers = await minervaCall<MinervaProvider[]>("codex.configure", { models });
        if (mounted.current) onSaved?.(providers);
        if (mounted.current) setSaved(true);
      }),
    reopen: () =>
      action(async () => {
        if (status.authorizationUrl) await openExternalUrl(status.authorizationUrl);
      }),
  };
}
