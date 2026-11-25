import { useState } from "react";
import { minervaCall, type MinervaProvider } from "@/platform/minerva";
const blank = { id: "", name: "", baseUrl: "", protocol: "openai-chat", models: [] as string[], apiKey: "" };
export function useProviderEditor(provider: MinervaProvider | null) {
  const initial = provider
    ? {
        id: provider.id,
        name: provider.name,
        baseUrl: provider.baseUrl,
        protocol: provider.protocol,
        models: provider.models,
        apiKey: "",
      }
    : blank;
  const [draft, setDraft] = useState(initial);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [fetching, setFetching] = useState(false);
  const action = async (run: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await run();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const input = { ...draft, apiKey: draft.apiKey || null, isNew: !provider };
  const save = (onSaved: (providers: MinervaProvider[]) => void) =>
    action(async () => {
      const providers = await minervaCall<MinervaProvider[]>("providers.save", input);
      setDraft({ ...draft, apiKey: "" });
      onSaved(providers);
    });
  const discover = () =>
    action(async () => {
      setFetching(true);
      try {
        const models = await minervaCall<string[]>("providers.discover", input);
        setDraft({ ...draft, models: [...new Set(models)] });
      } finally {
        setFetching(false);
      }
    });
  return {
    save,
    discover,
    fetching,
    draft,
    setDraft,
    reset: () => setDraft(initial),
    busy,
    error,
    action,
    editing: provider !== null,
    dirty: JSON.stringify(draft) !== JSON.stringify(initial),
    input: { ...draft, apiKey: draft.apiKey || null, isNew: !provider },
  };
}
