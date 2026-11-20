import React, { useEffect, useSyncExternalStore } from 'react';
import * as Dialog from '@radix-ui/react-dialog';
import { BookOpen, X } from 'lucide-react';
import type { BrowserPlugin, Scope } from '../../../packages/plugin-sdk/src';
import type { WorkspaceInfo } from '../../../packages/client/src/types';
import type { SettingsPageProps } from '../../../apps/web/src/app/plugins';
import { invoke } from '../../../apps/web/src/client';
import { Store } from '../../../packages/client-models/src/store';
import { Button } from '../../../apps/web/src/components/ui/button';
import { SkillsController, type Catalog } from './controller';
import './skills.css';

export function SkillList({
  controller,
  workspaceId,
}: {
  controller: SkillsController;
  workspaceId?: string;
}) {
  const store = controller.catalog(workspaceId);
  const { catalog, error, loading } = useSyncExternalStore(
    store.subscribe,
    store.getSnapshot,
    store.getSnapshot,
  );
  useEffect(() => controller.ensure(workspaceId), [controller, workspaceId]);
  return (
    <div className="skills-catalog">
      <div className="skills-toolbar">
        <span>{catalog ? `Навыков: ${catalog.items.length}` : 'Каталог навыков'}</span>
        <Button variant="outline" onClick={() => void controller.refresh(workspaceId)}>
          Обновить
        </Button>
      </div>
      {error && (
        <p role="alert" className="settings-error">
          Не удалось загрузить навыки: {error}
        </p>
      )}
      {loading && <p role="status">Загрузка навыков…</p>}
      {catalog && (
        <>
          {catalog.incomplete && (
            <div role="alert" className="settings-error">
              Часть навыков недоступна.
              {catalog.diagnostics.map((text, index) => (
                <p key={index}>{text}</p>
              ))}
            </div>
          )}
          {!catalog.items.length && (
            <p>
              {workspaceId
                ? 'В этом workspace нет локальных навыков.'
                : 'Глобальные навыки пока не подключены.'}
            </p>
          )}
          {catalog.items.map((skill) => (
            <article className="skills-card" key={`${skill.providerId}:${skill.name}`}>
              <strong>{skill.name}</strong>
              {skill.description && <p>{skill.description}</p>}
              <small>{skill.instructionPath ?? skill.providerId}</small>
              <span className="skills-access">
                {skill.modelInvocable ? 'Доступен модели' : 'Автовызов отключён'} ·{' '}
                {skill.userInvocable ? 'Доступен пользователю' : 'Вызов пользователем отключён'}
              </span>
            </article>
          ))}
        </>
      )}
    </div>
  );
}
function SkillsSettings({
  placement,
  selectedPage,
  onSelectPage,
  controller,
}: SettingsPageProps & { controller: SkillsController }) {
  if (placement === 'navigation')
    return (
      <Button
        variant="ghost"
        className={selectedPage === 'skills' ? 'selected' : ''}
        aria-current={selectedPage === 'skills' ? 'page' : undefined}
        onClick={() => onSelectPage('skills')}
      >
        <BookOpen size={16} /> Навыки
      </Button>
    );
  return (
    <>
      <h2>Глобальные навыки</h2>
      <p>Общие навыки, подключённые для всех рабочих пространств.</p>
      <SkillList controller={controller} />
    </>
  );
}
export function skillsPlugin(scope: Scope): BrowserPlugin {
  return {
    id: 'skills-ui',
    requires: ['minerva-ui'],
    apply(ctx) {
      const controller = new SkillsController(
        async (workspaceId) =>
          (await invoke('plugins.call', {
            id: 'skills',
            method: 'catalog',
            params: workspaceId ? { workspaceId } : {},
          })) as unknown as Catalog,
      );
      const selection = new Store<WorkspaceInfo | null>(null);
      ctx.effect(() => {
        controller.dispose();
        selection.clear(null);
      });
      function SkillsDialog() {
        const workspace = useSyncExternalStore(selection.subscribe, selection.getSnapshot);
        return (
          <Dialog.Root
            open={!!workspace}
            onOpenChange={(open) => {
              if (!open) selection.set(null);
            }}
          >
            <Dialog.Portal>
              <Dialog.Overlay className="settings-overlay" />
              <Dialog.Content className="oe-studio skills-dialog">
                <div className="skills-dialog-header">
                  <Dialog.Title>Локальные навыки — {workspace?.title}</Dialog.Title>
                  <Dialog.Close asChild>
                    <Button variant="ghost" size="icon" aria-label="Закрыть">
                      <X size={16} />
                    </Button>
                  </Dialog.Close>
                </div>
                <Dialog.Description>Навыки выбранного рабочего пространства.</Dialog.Description>
                {workspace && (
                  <SkillList
                    controller={controller}
                    key={workspace.id}
                    workspaceId={workspace.id}
                  />
                )}
              </Dialog.Content>
            </Dialog.Portal>
          </Dialog.Root>
        );
      }
      const scoped = ctx.scope(scope);
      scoped.slots.register('settings.pages', {
        entryId: 'skills',
        order: 200,
        component: (props: SettingsPageProps) => (
          <SkillsSettings {...props} controller={controller} />
        ),
      });
      scoped.slots.register('shell.content.after', {
        entryId: 'skills-dialog',
        component: SkillsDialog,
      });
      scoped.slots.register('contextMenu.icon', {
        entryId: 'workspace.skills',
        component: () => <BookOpen size={14} />,
      });
      scoped.contextMenus.register({
        id: 'workspace.skills',
        target: 'workspace',
        order: 200,
        resolve: ({ workspace, disabled }) => [
          {
            id: 'skills',
            label: 'Локальные навыки',
            icon: { key: 'workspace.skills' },
            disabled,
            run: () => selection.set(workspace),
          },
        ],
      });
    },
  };
}
