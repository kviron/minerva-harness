# Рабочая область агента

Кнопка разделения в правом верхнем углу общей области открывает нативный редактор OpenIDEA рядом с разговором и сохраняет экранную позицию при открытии/сворачивании. Соседняя кнопка разворачивает workspace на всю ширину поверх места диалога; возврат восстанавливает прежнее разделение без размонтирования чата и редактора. Закрытие последней вкладки сворачивает workspace. Чат остаётся отдельной поверхностью: файл, терминал или вкладка плагина его не заменяет. Сворачивание не размонтирует редактор, не закрывает документы и процессы. Ширина разделения сохраняется; при узком окне история отдаёт место рабочей области, сохраняя исходное предпочтение для закрытого состояния.

Стартовая вкладка: Files, New terminal, Browser, New File, Go to File, More tools и представления плагинов. Files и все инструменты из More tools открываются нативными вкладками `workspace.tool` внутри редактора. Commit, GraphQL и другие представления занимают всю вкладку; отдельные IDE-колонки слева, справа или снизу не открываются. Сохраняются собственные внутренние вкладки, toolbar и меню инструмента, а управление доком исключено. Browser/терминал также используют всю вкладку. Development сохраняет обычную IDE-раскладку. Новый терминал запускается в cwd текущей задачи, включая worktree; при ошибке создания сессии существующий терминал не переносится.

В режиме «Агент» встроенный модуль `features/workspace` назначает единый хост для зарегистрированных окон. Значки, команды и плагины используют обычный сервис окон, а хост открывает их во вкладках независимо от исходной позиции дока. Дерево «Проект» открывает документы через штатный редактор. Событие успешного открытия показывает воркспейс даже при повторном открытии уже выбранного файла. При переходе в «Разработку» хост снимается и применяется сохранённая раскладка доков.

Публичный API WASM-плагинов позволяет показывать представления самого плагина. Выбор хоста для штатных окон проходит через внутренний интерфейс `toolWindowHost`, а реализация и правила режима входят во встроенный модуль Minerva.

## Границы модулей

| Ответственность                                                            | Код                                                            |
| -------------------------------------------------------------------------- | -------------------------------------------------------------- |
| Выбор разговора, черновики, кэш задач                                      | `features/agents/store.ts`                                     |
| Самостоятельная поверхность чата                                           | `features/agents/AgentConversation.tsx`                        |
| Раскладка, нативный splitter, сворачивание без размонтирования редактора   | `workbench/WorkspaceLayout.tsx`                                |
| Общий реестр стартовых элементов                                           | `registerWorkspaceItem` в `workbench/contributions.ts`         |
| Стартовая вкладка, действия, web preview                                   | `features/workspace/`                                          |
| Адаптация существующих слотов и сервисов SDK                               | `platform/minervaWorkspaceUi.tsx`                              |
| Загрузка плагинов, RPC, настройки                                          | Существующий `platform/minervaPlugins.tsx` и исходный SDK host |
| Вкладки, внутренние splits, Save/Undo, dirty prompts, сохранение редактора | Существующий редактор OpenIDEA                                 |
| PTY, вывод, завершение терминалов                                          | Существующая feature terminal                                  |
| WASM-плагины и их webviews                                                 | Существующий runtime OpenIDEA, без изменений                   |

Старые agent-editor tabs закрываются при передаче владения выделенной поверхности Agent. Открытие чата не создаёт второй SessionEditor внутри рабочего редактора. Кэш, активный composer, удаление задачи и выделение истории учитывают самостоятельную поверхность. В Development разговор по-прежнему размещается в нативном Agents tool window.

Область принадлежит выбранному проекту IDE, сопоставленному с workspace Minerva. Разговоры проекта используют его редактор и вкладки совместно. Это осознанно отличается от per-session docking tree DSH. ID задачи OpenIDEA не выдаётся за ID runtime-сессии Minerva.

## Контракт плагинов

При переходе в Development feature workspace через `closeWorkspace` закрывает служебные `workspace.*` и Minerva canvas/panel вкладки нативным guarded `closeInput`, затем сбрасывает open/maximized. Обычные файловые и CLI-вкладки остаются ресурсами IDE. Отказ закрытия оставляет Agent; отменённый более новым запросом асинхронный переход не меняет режим. Включение Development происходит после завершения закрытия.

Сохранены исходные ownership, scoped slots, loader, RPC, enable/disable. `shell.canvas.tabs` и `workspace.panels` становятся нативными editor kinds и элементами стартовой страницы, вместо скрытых tool windows. `shell.sections` остаётся зарегистрированным tool window, но в Agent его содержимое размещается вкладкой через More tools; `settings.pages` остаётся страницей настроек.

У SlotEntry появились необязательные `title` и `icon` — имя нативной иконки OpenIDEA. Старые плагины получают имя пакета и стандартную иконку. Адаптер сравнивает identity записей: изменение другого слота не пересоздаёт открытую вкладку. Disable удаляет вклад и закрывает его вкладки. Native keep-alive сохраняет DOM между вкладками и при сворачивании.

```tsx
import type { WorkspaceService } from '@minerva/plugin-sdk';

export const sdkVersion = 2;
export function apply(ctx) {
  const workspace = ctx.services.get<WorkspaceService>({
    name: 'minerva.workspace', version: 1,
  });
  ctx.slots.register('shell.canvas.tabs', {
    entryId: 'my-report', title: 'Report', icon: 'file',
    component: ({ workspaceId }) => <Report workspaceId={workspaceId} />,
  });
  // Из обработчика кнопки плагина:
  // await workspace.open('my-report', ctx.ownerPluginId);
  // await workspace.openFile('/absolute/path/report.md');
  // workspace.close(); // Свернуть область, сохранив документы.
}
```

SDK также экспортирует `workspaceServiceKey`. Компонент получает workspace-scoped props: `workspaceId`, `part: 'body'`, `placement: 'right'`, `selected`. Импорты из старого приложения, глобальный `canvas.show()` и session-dependent представления не эмулируются поддельным sessionId: им нужна адаптация к scoped service либо отдельный контракт runtime-сессий. Slot views являются UI, а не поставщиками редактируемых документов; для собственных несохранённых документов необходим нативный document lifecycle, а не одно React state.

## Что взято из открытых реализаций

- [DSH: контракт right sidebar](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/sidebar-right.md): разделение layout, реестра типов, navigation service и ресурсов. Использованы соответствующие слои OpenIDEA, второй docking engine не добавлен.
- [DSH: lifecycle поверхности](https://github.com/deepseek-ai/deepseek-harness/blob/master/packages/client/ui-sidebar-right/README.md): смена представления сохраняет дерево содержимого. Наш редактор остаётся mounted; floating panes и fullscreen-представление DSH здесь не реализованы.
- [OpenDesign: архитектура](https://github.com/nexu-io/open-design/blob/main/README.md#architecture): отдельные chat/file workspace/runtime definitions и sandboxed preview. Его plugin manifests — другой контракт; перенос ещё одного plugin host не нужен.
- [Hermes Desktop](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/desktop.md): общий agent core/state между поверхностями и сохранение происхождения artifact/cwd. Раскладка не создаёт второй runtime, терминал использует cwd задачи.

Ограниченный разбор старого кода обнаружил глобальный canvas store/localStorage key и app-owned render callbacks в `vendor/minerva/apps/web/src/app/canvas.tsx`. Они не перенесены. В предыдущем адаптере миграции все canvas tabs превращались в tool windows, а регистрации пересоздавались на каждом slot event. Новый workspace bridge исправляет эти два места. Полный аудит Minerva runtime не проводился.

## Ограничения и проверка

Browser — HTTP(S) preview, а не полноценный Chromium-браузер. Executable/file URL, credentials и внутренние IPC hosts отклоняются. iframe изолирован sandbox без same-origin/native IPC; для сайтов, запрещающих embedding, есть внешнее открытие. CSP разрешает HTTP(S) frames отдельно от app scripts/IPC. Уже запущенный native binary требует rebuild/restart для применения CSP. Browser tabs живут в текущем сеансе и автоматически не восстанавливаются с исполнением удалённых страниц при запуске.

Тесты покрывают ownership/reconciliation/disable SDK bridge, URL validation, terminal routing/failure, Agent navigation/composer, восстановление native tool windows и text catalogs. Независимая browser-проверка покрывает 1440/1100, широкую историю, resize/persistence, черновик и scroll, сохранение DOM native New File, plugin-view DOM/disposal, переходы Agent/Development. Она использует mock IPC, а не реальный PTY/установленный plugin package. Нативный Agent экран наблюдался через accessibility; клики заблокировал geometry guard инструмента управления. Native CSP/browser и реальный терминал для этого изменения не проверены.
