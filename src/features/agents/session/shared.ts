import type { AgentEntry, AgentTask } from "@/bindings";
import { useEffect, useState } from "react";
import type { SubagentEntry } from "../model";
import { useAgents } from "../store";
import { agentProviderId } from "../providerOptions";

export function providerName(id: string) {
  const providerId = agentProviderId(id);
  return (
    useAgents.getState().providers.find((p) => p.id === providerId)?.name ?? (providerId === "minerva" ? "Minerva" : id)
  );
}

export function conversationAuthor(provider: string, subagent: SubagentEntry | null) {
  const id = agentProviderId(provider);
  // Minerva's delegation projection titles native children with their driver ID
  // and internal children with their model route.
  const target = id === "minerva" && subagent?.id.startsWith("del_") ? subagent.title : null;
  const native = target && useAgents.getState().providers.find((p) => p.id === target);
  return native ? { id: native.id, name: native.name } : { id, name: providerName(id) };
}

/** `Date.now()`, updated every `interval` ms. */
export function useNow(interval: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), interval);
    return () => clearInterval(t);
  }, [interval]);
  return now;
}

export type UserEntry = Extract<AgentEntry, { type: "user" }>;

/** Agent-written titles with the task's own folder dropped from paths. */
export const shortTitle = (task: AgentTask, title: string) => title.replaceAll(`${task.cwd}/`, "");
