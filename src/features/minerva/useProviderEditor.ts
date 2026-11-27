import { useState } from "react";
import { minervaCall, type CatalogModel, type MinervaProvider } from "@/platform/minerva";
import type { ProviderDraft } from "./providerDraft";
const blank: ProviderDraft = {
  id: "",
  name: "",
  baseUrl: "",
  protocol: "openai-chat",
  models: [],
  modelWindows: {},
  apiKey: "",
};
export function useProviderEditor(provider: MinervaProvider | null) {
  const initial = provider
    ? {
        ...(provider.creditGrants ? { creditGrants: true } : {}),
        id: provider.id,
        name: provider.name,
        baseUrl: provider.baseUrl,
        protocol: provider.protocol,
        models: provider.models,
        modelWindows: provider.modelWindows ?? {},
        apiKey: "",
      }
    : blank;
  const [draft, setDraft] = useState<ProviderDraft>(initial);
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
        const catalog = await minervaCall<CatalogModel[]>("providers.catalog", input);
        const models = [...new Set(catalog.map((row) => row.id))];
        const modelWindows = Object.fromEntries(
          catalog.flatMap((row) => {
            const discovered = row.contextWindow;
            const limit = draft.modelWindows[row.id]?.limit;
            const window = {
              ...(discovered ? { discovered } : {}),
              ...(limit === undefined ? {} : { limit }),
            };
            return Object.keys(window).length ? [[row.id, window]] : [];
          }),
        );
        setDraft({ ...draft, models, modelWindows });
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
