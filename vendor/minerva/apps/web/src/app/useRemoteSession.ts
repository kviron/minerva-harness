import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { SessionModel } from '../../../../packages/client-models/src/session.ts';
import {
  createSessionStream,
  createDesktopSessionStream,
} from '../../../../packages/client/src/streams.ts';
import { clientMode, clientScope, invoke } from '../client.ts';
import { registerScopeCleanup } from './models.ts';

export function useRemoteSession(sessionId: string) {
  // Identity changes reboot the document through replaceClientScope.
  const { workspace } = clientScope;
  const model = useMemo(
    () =>
      new SessionModel(
        sessionId,
        clientMode === 'tauri'
          ? createDesktopSessionStream(invoke)
          : createSessionStream({ workspaceId: workspace }),
      ),
    [sessionId, workspace],
  );
  useEffect(() => {
    const unregister = registerScopeCleanup(model.dispose);
    void model.follow();
    return () => {
      unregister();
      model.dispose();
    };
  }, [model]);
  return useSyncExternalStore(model.subscribe, model.getSnapshot);
}
