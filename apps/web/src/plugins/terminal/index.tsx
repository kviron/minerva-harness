import React, { Suspense, lazy } from 'react';
import { TerminalSquare } from 'lucide-react';
import type { BrowserPlugin, Scope } from '../../../../../packages/plugin-sdk/src/index';
import type { AppSlotProps } from '../../app/plugins';
import { canvas, useCanvas } from '../../app/canvas';
import { Button } from '../../components/ui/button';
import { Tooltip, TooltipContent, TooltipTrigger } from '../../components/ui/tooltip';
import { CommandBlock } from './CommandBlock';
import { invoke } from '../../client';
import { pending, terminals } from './store';
const TerminalPanel = lazy(() => import('./panel'));
const TAB = 'mh-terminal';
let next = 2;

function TitlebarButton() {
  const { open, tab } = useCanvas();
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-lg"
          className="titlebar-button"
          aria-label="Терминал"
          aria-pressed={open && tab === TAB}
          onClick={() => canvas.toggle(TAB)}
        >
          <TerminalSquare />
        </Button>
      </TooltipTrigger>
      <TooltipContent className="oe-studio">Терминал</TooltipContent>
    </Tooltip>
  );
}
function Tab(
  props: AppSlotProps['shell.canvas.tabs'] & {
    onNew: () => void;
    onAcquire: (key: string, id: string) => boolean;
  },
) {
  if (props.part === 'tab')
    return (
      <>
        <TerminalSquare aria-hidden="true" />
        Терминал
      </>
    );
  return (
    <Suspense
      fallback={
        <p className="terminal-note" role="status">
          Загрузка терминала…
        </p>
      }
    >
      <TerminalPanel sessionId={props.sessionId} onNew={props.onNew} onAcquire={props.onAcquire} />
    </Suspense>
  );
}
export function terminalPlugin(scope: Scope): BrowserPlugin {
  return {
    id: 'mh-terminal-ui',
    requires: ['minerva-ui'],
    apply(ctx) {
      const scoped = ctx.scope(scope);
      const owned = new Set<string>();
      let alive = true;
      const shells = new Map<string, string>();
      const onAcquire = (key: string, id: string) => {
        if (!alive) return false;
        shells.set(key, id);
        return true;
      };
      const openNewTerminal = () => {
        if (!alive) return;
        const slot = String(next++),
          tabId = `${TAB}:${slot}`;
        owned.add(tabId);
        canvas.show({
          id: tabId,
          title: `Терминал ${slot}`,
          icon: <TerminalSquare aria-hidden="true" />,
          render: ({ sessionId }) => (
            <Suspense
              fallback={
                <p className="terminal-note" role="status">
                  Загрузка терминала…
                </p>
              }
            >
              <TerminalPanel
                sessionId={sessionId}
                slot={slot}
                onNew={openNewTerminal}
                onAcquire={onAcquire}
              />
            </Suspense>
          ),
          onClose: () => {
            owned.delete(tabId);
            for (const [key, id] of terminals.slot(slot)) {
              shells.delete(key);
              pending.delete(key);
              void invoke('mh-terminal.close', { sessionId: key.split('#')[0], id }).catch(
                () => {},
              );
            }
          },
        });
      };
      scoped.effect(() => {
        alive = false;
        for (const id of owned) canvas.closeTab(id);
        for (const [key, id] of shells) {
          terminals.delete(key, id);
          pending.delete(key);
          void invoke('mh-terminal.close', { sessionId: key.split('#')[0], id }).catch(() => {});
        }
        shells.clear();
      });
      const OwnedTab = (props: AppSlotProps['shell.canvas.tabs']) => (
        <Tab {...props} onNew={openNewTerminal} onAcquire={onAcquire} />
      );
      scoped.slots.register('shell.titlebar.end', {
        entryId: TAB,
        order: 100,
        component: TitlebarButton,
      });
      scoped.slots.register('shell.canvas.tabs', { entryId: TAB, component: OwnedTab });
      scoped.slots.register('message.code', { entryId: 'shell', component: CommandBlock });
    },
  };
}
