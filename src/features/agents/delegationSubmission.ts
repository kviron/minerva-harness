import { call, commands } from "@/platform/ipc";

const outstanding = new Map<string, string>();

async function submissionKey(task: string, subagent: string, prompt: string) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(prompt));
  const hash = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `minerva.delegation.followup.${task}.${subagent}.${hash}`;
}

export async function sendDelegationMessage(task: string, subagent: string, prompt: string) {
  const key = await submissionKey(task, subagent, prompt);
  let operation = outstanding.get(key);
  try {
    operation ??= localStorage.getItem(key) ?? undefined;
  } catch {
    // Keep retries in this window if the webview has disabled storage.
  }
  operation ??= crypto.randomUUID();
  outstanding.set(key, operation);
  try {
    localStorage.setItem(key, operation);
  } catch {
    // The in-memory operation remains available for retry.
  }
  const receipt = await call(commands.agentDelegationFollowup(task, subagent, operation, prompt));
  if (outstanding.get(key) === operation) outstanding.delete(key);
  try {
    if (localStorage.getItem(key) === operation) localStorage.removeItem(key);
  } catch {
    // Storage is optional.
  }
  return receipt;
}

export function sendDelegationMessageNow(task: string, subagent: string, workItem: string, expectedRun: string) {
  return call(commands.agentDelegationFollowupNow(task, subagent, workItem, expectedRun));
}
