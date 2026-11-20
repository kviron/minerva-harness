type Workspace = { id: string; sessionIds: string[] };
/** A chat without an attached project must never inherit another project's files. */
export function workspaceForSession<T extends Workspace>(
  workspaces: readonly T[],
  sessionId: string,
  picked: string | null = null,
): T | undefined {
  return (
    workspaces.find((workspace) => workspace.id === picked) ??
    workspaces.find((workspace) => workspace.sessionIds.includes(sessionId))
  );
}
