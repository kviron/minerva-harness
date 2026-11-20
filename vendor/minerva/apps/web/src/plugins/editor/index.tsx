import React, { Suspense, lazy } from 'react';
import { Code2 } from 'lucide-react';
import type { BrowserPlugin, Scope } from '../../../../../packages/plugin-sdk/src/index';
import type { ShellSectionProps } from '../../../../../packages/plugin-sdk/src/slots.types';
import { Button } from '../../components/ui/button';
import { canvas, useCanvas } from '../../app/canvas';
import { enableEditor } from './lifecycle';
import type { AppSlotProps } from '../../app/plugins';
export { editorIcons } from './icons';
export type { IconContext } from './iconRegistry';
const EditorView = lazy(() => import('./view'));
function Section(props: ShellSectionProps) {
  if (props.placement === 'rail')
    return (
      <Button
        variant="ghost"
        size="icon-lg"
        className={`rail-button ${props.selected ? 'selected' : ''}`}
        aria-label="Разработка"
        aria-current={props.selected ? 'page' : undefined}
        onClick={() => props.onNavigate('mh-editor')}
      >
        <Code2 />
      </Button>
    );
  if (props.placement === 'title') return <>Разработка</>;
  return (
    <Suspense fallback={<div role="status">Загрузка редактора…</div>}>
      <EditorView sessionId={props.sessionId} placement={props.placement} />
    </Suspense>
  );
}
function Panel(props: AppSlotProps['shell.canvas.tabs']) {
  if (props.part === 'tab')
    return (
      <>
        <Code2 aria-hidden="true" />
        Редактор
      </>
    );
  return (
    <Suspense fallback={<div role="status">Загрузка редактора…</div>}>
      <EditorView sessionId={props.sessionId} placement="panel" />
    </Suspense>
  );
}
function PanelButton() {
  const { open, tab } = useCanvas();
  return (
    <Button
      variant="ghost"
      size="icon-lg"
      className="titlebar-button"
      aria-label="Редактор в боковой панели"
      title="Редактор в боковой панели"
      aria-pressed={open && tab === 'mh-editor'}
      onClick={() => canvas.toggle('mh-editor')}
    >
      <Code2 />
    </Button>
  );
}
export function editorPlugin(scope: Scope): BrowserPlugin {
  return {
    id: 'mh-editor-ui',
    requires: ['minerva-ui'],
    apply(ctx) {
      const scoped = ctx.scope(scope);
      scoped.effect(enableEditor());
      scoped.slots.register('shell.sections', {
        entryId: 'mh-editor',
        order: 30,
        component: Section,
      });
      scoped.slots.register('shell.canvas.tabs', { entryId: 'mh-editor', component: Panel });
      scoped.slots.register('shell.titlebar.end', {
        entryId: 'mh-editor',
        order: 90,
        component: PanelButton,
      });
    },
  };
}
