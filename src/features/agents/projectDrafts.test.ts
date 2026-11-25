import { expect, it } from "vitest";
import { HOME, selectDraftProject, updateComposer, updateDraft, useAgents, reconcileDraftProvider } from "./store";
import type { AgentProvider } from "@/bindings";

it("keeps a new task's prompt, file context and settings in its own project", () => {
  selectDraftProject("/projects/one");
  updateComposer(HOME, { text: "Project one draft" });
  updateDraft({ base: "one-branch" });
  selectDraftProject("/projects/two");
  expect(useAgents.getState().composers.get(HOME)?.text).toBe("");
  expect(useAgents.getState().draft.base).toBeNull();
  updateComposer(HOME, { text: "Project two draft" });
  selectDraftProject("/projects/one");
  expect(useAgents.getState().composers.get(HOME)?.text).toBe("Project one draft");
  expect(useAgents.getState().draft.base).toBe("one-branch");
  selectDraftProject(null);
});

it("moves the old Minerva selection to its configured connection without losing its model", () => {
  useAgents.setState({
    providers: [
      { id: "minerva.api.one", args: ["api:one/"], installed: true },
      { id: "codex", args: [], installed: true },
    ] as AgentProvider[],
  });
  updateDraft({ provider: "minerva", config: { model: "api:one/model" } });
  reconcileDraftProvider();
  expect(useAgents.getState().draft.provider).toBe("minerva.api.one");
  expect(useAgents.getState().draft.config.model).toBe("api:one/model");
  useAgents.setState({ providers: [useAgents.getState().providers.find((provider) => provider.id === "codex")!] });
  reconcileDraftProvider();
  expect(useAgents.getState().draft.provider).toBe("codex");
  expect(useAgents.getState().draft.config).toEqual({});
});

it("restores a legacy Minerva draft under unified discovery without clearing its route or effort", () => {
  useAgents.setState({ providers: [{ id: "minerva", installed: true }] as AgentProvider[] });
  updateDraft({ provider: "minerva.api.one", config: { model: "api:one/model", thought_level: "high" } });
  reconcileDraftProvider();
  expect(useAgents.getState().draft.provider).toBe("minerva");
  expect(useAgents.getState().draft.config).toEqual({ model: "api:one/model", thought_level: "high" });
});
