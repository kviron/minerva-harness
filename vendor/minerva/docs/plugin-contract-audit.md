# Аудит контрактов плагинов — 2026-11-13

## Источник и границы сравнения

Проверены опубликованные исходники DeepSeek Harness на закреплённой ревизии
`fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159`:

- [Slots: владение, lifecycle, props и запрет runtime-импорта чужих feature-компонентов](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/docs/subsystems/slots.md).
- [Правила клиентских пакетов: apply, контроллеры, зависимости, module/store ownership](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/client/AGENTS.md).
- [Client module wire и загрузчик](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/packages/client/modules/src/client/manifest.ts).
- [Client Modules: shared instances, ошибки загрузки и освобождение](https://github.com/deepseek-ai/deepseek-harness/blob/fcc0ad462172522aaf5c93a7b2e01f6b9f0b0159/docs/subsystems/client-modules.md).

Локальный путь к DSH внутри `app.asar` не оказался доступным каталогом. Это
сверка с указанной публичной ревизией, а не подтверждение версии запущенного DSH.

Minerva **не является wire/package-совместимой реализацией DSH**. Наши действующие
контракты — [UI](plugin-ui.md), [backend](backend-plugins.md),
[browser SDK](../packages/plugin-sdk/README.md) и [native SDK](../crates/plugin-sdk/src/lib.rs).
Проверены все 21 Rust-пакет плагинов, шесть feature UI-плагинов, root UI,
регистрации, зависимости, владение ресурсами и ключевые trust boundaries.
Это целевой аудит поверхностей контрактов, не построчная проверка всех реализаций
и не аудит сторонних библиотек.

## Совпадения и осознанные отличия

| Область | DSH | Minerva |
| --- | --- | --- |
| Владение | Регистрации/эффекты принадлежат fiber | Scoped Ownership, автоматический отзыв слотов/сервисов/меню |
| Слоты | Parent-owned children, inject, пять derived shares | Статический типизированный каталог, single/list/keyed, явные props |
| Реактивность | Renderer создаёт стандартные hooks; feature-компоненты не вызывают uSES | Контроллеры вне компонентов, подписка через useSyncExternalStore разрешена нашим контрактом |
| UI-пакеты | Lazy CJS table, package graph, chunks/HMR | Single-file CJS SDK v2, shared React, host-supplied loader/compiler; нет межпакетных импортов/chunks/HMR |
| Backend | Cordis services/inject | Rust Cordis; внешние доверенные process-пакеты и versioned service contracts |
| Зависимости | Module graph и service inject — разные механизмы | Browser requires указывает ID плагина; backend requires указывает имя/версию сервиса |
| Безопасность | Lifecycle/DI не заменяют sandbox | Scoped RPC/import whitelist не являются OS/JS sandbox |

Отсутствие DSH-specific children/factories/hooks не исправлялось добавлением
несовместимого полуконтракта. Если требуется буквальное соответствие этим API,
это отдельная миграция renderer/SDK/всех клиентов, а не исправление одного плагина.

## Подтверждённые нарушения и исправления

1. **Чужое владение моделью workspace.** Отключение workspaces-ui вызывало dispose
   общей app-модели, сбрасывая данные и подписки editor/terminal. Удалено чужое
   освобождение; владельцем остаётся app scope. [Регистрация](../plugins/mh-workspaces/ui/index.tsx),
   [регрессионный тест](../tests/workspaces-plugin-lifecycle.test.mjs).
2. **Lifecycle editor.** До исправления listener beforeunload, Monaco models,
   provider registrations, blob URLs и LSP-документы не имели plugin cleanup.
   Исправление и проверки сосредоточены в [editor](../apps/web/src/plugins/editor/index.tsx),
   [store](../apps/web/src/plugins/editor/store.ts) и [пуле документов](../apps/web/src/plugins/editor/documentPool.ts).
   Обычный unmount представления сохраняет документы. Disable снимает ресурсы;
   несохранённый текст и исходная disk version сохраняются в памяти для re-enable.
   Pending saves сериализованы, cleanup ждёт их и обновляет recovery baseline.
   **Recovery не переживает перезагрузку страницы/процесса** и не записывает файл
   автоматически. [Поведенческие lifecycle-тесты](../tests/editor-lifecycle.test.mjs).
3. **Lifecycle terminal UI.** Dynamic Canvas tabs переживали disable; поздний
   open мог создать orphan shell и отправить очередь после unmount. Добавлены
   per-apply ownership, fencing, close позднего результата и compare-and-delete
   remembered ID. Primary shells сохраняются при обычном remount, но закрываются
   при disable своего плагина. [Регистрация](../apps/web/src/plugins/terminal/index.tsx),
   [панель](../apps/web/src/plugins/terminal/panel.tsx),
   [тесты](../tests/terminal-lifecycle.test.mjs).
4. **Контроллер skills.** Каталог терял snapshot при переходе со страницы настроек.
   Контроллер создаётся в apply, хранит отдельные workspace snapshots, fence-ит
   устаревшие ответы и освобождается при disable. [Контроллер](../plugins/mh-skills/ui/controller.ts),
   [тесты](../tests/skills-controller.test.mjs).
5. **Порядок остановки optional backend.** Reverse BTreeMap order не совпадал с
   reverse dependency order. Теперь используется обратный порядок каталога
   активации, тест проверяет каждое объявленное ребро и подмножество.
   [Реализация и тест](../crates/runtime/src/learning.rs).
6. **Rollback позднего bootstrap.** Ошибки открытия learning/подключения optional
   раньше обходили cleanup уже активных сервисов. Добавлен rollback optional,
   runtime fibers и manager. [Runtime](../crates/runtime/src/lib.rs),
   [learning](../crates/runtime/src/learning.rs).
7. **Shell fiber и удерживаемый Arc.** Добавлены остановка admission и освобождение
   PTY-ресурсов при dispose fiber независимо от внешних ссылок на ShellService;
   хранится reader JoinHandle, завершившиеся readers join-ятся. [Shell](../plugins/mh-shell/src/lib.rs),
   [PTY lifecycle и тесты](../plugins/mh-shell/src/terminal.rs).
   **Остаточное ограничение:** portable-pty не даёт переносимой отмены каждого
   blocking reader/Drop. После ограниченного ожидания cleanup worker может
   продолжить владеть ресурсами; уже заблокированный I/O под mutex также может
   задержать вход в shutdown. Полный bounded stop/join на всех ОС не заявляется.
8. Обновлены устаревшие описания browser loader и бинарных editor previews в
   [SDK README](../packages/plugin-sdk/README.md) и [backend API](backend-plugins.md).
9. **Проверка generated wire на Windows.** `contracts:check` сравнивал LF-вывод
   генератора с checkout CRLF побайтно и сообщал ложный drift без смыслового diff.
   [Проверка](../scripts/generate-contracts.mjs) теперь нормализует CRLF только для
   сравнения; реальные изменения типов по-прежнему обнаруживаются.

## Покрытие каждого плагина

«Не найдено» означает отсутствие дополнительного подтверждённого нарушения в
проверенных регистрациях/lifecycle/boundaries, не доказательство отсутствия всех багов.

| Rust-плагин | Проверенная поверхность / результат |
| --- | --- |
| [mh-plugin-manager](../plugins/mh-plugin-manager/src/plugins.rs) | Bootstrap/rollback, protected IDs, RPC/tool admission, roster; orchestration исправлен в runtime |
| [mh-providers](../plugins/mh-providers/src/lib.rs) | Публикация сервиса, hosted endpoint/credential policy; не найдено |
| [mh-shell](../plugins/mh-shell/src/lib.rs) | Admission, process limits, fiber/PTY cleanup; исправлено с ограничением portable-pty выше |
| [mh-tool-runtime](../plugins/mh-tool-runtime/src/lib.rs) | Schemas, owner generation, approval, cancellation, output limits; не найдено |
| [mh-context](../plugins/mh-context/src/lib.rs) | Публикация и AccessContext boundaries; не найдено |
| [mh-workspace-tools](../plugins/mh-workspace-tools/src/lib.rs) | Local/hosted gate, tool revocation, session-bound file/attachment/skill calls; не найдено |
| [mh-editor](../plugins/mh-editor/src/lib.rs) | Workspace prerequisite, traversal/symlink/capped-read/CAS-save; исправлена документация previews |
| [mh-lsp](../plugins/mh-lsp/src/lib.rs) | Provider conflicts, no-shell startup, session buffer ownership, unregister/shutdown effect; не найдено |
| [mh-lsp-rust](../plugins/mh-lsp-rust/src/lib.rs) | LSP prerequisite, registration rollback и dispose; не найдено |
| [mh-mcp](../plugins/mh-mcp/src/lib.rs) | Generation cancellation, revoke/close/kill, conservative side effects, pause policy; не найдено |
| [mh-sessions](../plugins/mh-sessions/src/lib.rs) | Shutdown effect до публикации; не найдено |
| [mh-agent-loop](../plugins/mh-agent-loop/src/lib.rs) | Service prerequisites, shutdown effect; поздний runtime rollback исправлен |
| [mh-workspaces](../plugins/mh-workspaces/src/lib.rs) | Context/Sessions prerequisites и stopped admission; не найдено |
| [mh-session-query](../plugins/mh-session-query/src/lib.rs) | Actor/workspace/session authorization, bounded cursors/source reads; порядок teardown исправлен |
| [mh-skills](../plugins/mh-skills/src/lib.rs) | Scope/provider authorization, discovery limits; порядок teardown исправлен |
| [mh-memory](../plugins/mh-memory/src/lib.rs) | Scoped reads/writes, limits, idempotency; не найдено |
| [mh-experience](../plugins/mh-experience/src/lib.rs) | Scoped bundles/digests, verifier-controlled publication/CAS retirement; не найдено |
| [mh-experiments](../plugins/mh-experiments/src/lib.rs) | Required isolation fail-closed, authority/quotas; порядок teardown исправлен |
| [mh-improvement](../plugins/mh-improvement/src/lib.rs) | Journal/resume validation, finite schedule/budget/cancellation; порядок teardown исправлен |
| [mh-skill-curation](../plugins/mh-skill-curation/src/lib.rs) | Scope, host actor, evaluation/CAS gates; порядок teardown исправлен |
| [review-example](../plugins/review-example/backend/src/main.rs) | Manifest/initialize/review/shutdown; backend-only, браузерного UI в пакете нет |

| Browser-плагин | Результат |
| --- | --- |
| [minerva-ui](../apps/web/src/app/plugins.tsx) | Root registration принадлежит host; дополнительных ресурсов нет |
| [workspaces-ui](../plugins/mh-workspaces/ui/index.tsx) | Исправлено освобождение чужой app-модели; context menu ownership/abort guards проверены |
| [skills-ui](../plugins/mh-skills/ui/index.tsx) | Контроллер и selection теперь apply-owned; retained snapshots и disposal |
| [interactions-ui](../plugins/mh-interactions/ui/index.tsx) | Scoped registrations, serial polling и fences; дополнительного нарушения не найдено |
| [message-queue-ui](../plugins/mh-message-queue/ui/index.tsx) | Scoped registration, нет дополнительных plugin-owned фоновых ресурсов; не найдено |
| [mh-editor-ui](../apps/web/src/plugins/editor/index.tsx) | Исправлен plugin-owned cleanup; shared documents, dirty recovery, pending-save fencing и serial diagnostics проверены тестами |
| [mh-terminal-ui](../apps/web/src/plugins/terminal/index.tsx) | Исправлены dynamic/primary ownership и late async initialization |

## ESLint и проверки

[Flat config](../eslint.config.mjs) подключён через `npm run lint` и `npm run lint:fix`.
Проверяются JS/TS/TSX, базовые correctness rules, React hook order и dependencies.
Сгенерированный клиент, build outputs и зависимости исключены. CJS fixtures
сохраняют разрешённый require; declaration-merging seam и защитные проверки
fixture cleanup имеют точечные мотивированные исключения.

Правила unused variables и exhaustive-deps пока warnings: их наличие не скрывается
и не считается доказательством соблюдения plugin lifecycle. Все ошибки первого
lint-прогона исправлены отдельно от архитектурных нарушений. Установлены ESLint 10,
TypeScript ESLint и React Hooks; версии и integrity сохранены в
[lock-файле](../package-lock.json). Проверки запускались с bundled Node 24.21.0;
системный Node 23 не входит в поддерживаемый диапазон ESLint 10.

### Выполненные проверки

| Проверка | Результат |
| --- | --- |
| `npm run lint` | Exit 0: 0 ошибок, 19 warnings (unused/dependency arrays вне исправленных lifecycle-путей) |
| `npm test` | 119 passed, 0 failed |
| `npm run build` | TypeScript + production Vite build, exit 0 |
| `contracts:check` | Exit 0; дополнительный assert self-check прошёл для LF и CRLF checkout |
| `cargo test -p 'mh-*' -p minerva-review-example -p minerva-plugin-sdk -p minerva-plugin-host` | Все запущенные suites прошли; 4 явно ignored: Python/rust-analyzer fixtures и копия пользовательской БД |
| `cargo test -p minerva-runtime --lib` | 20 passed до добавления дополнительного bootstrap-теста |
| `cargo test -p minerva-runtime late_learning_failure_releases_runtime_storage_for_retry --lib` | 1 passed; проверяет отказ на future-schema learning DB и успешный повторный запуск |
| Playwright: skills, plugin-settings, plugin-roster, plugin-isolation, layout | 11 passed, headless fixtures Minerva |
| `git diff --check` по изменённому коду/контрактам/тестам | Exit 0 |

Vite оставляет предупреждения о `use client`, sourcemap, dynamic/static import
и размере chunks; сборка успешна. Нативный GUI и Linux/macOS PTY не проверялись.
Сквозная проверка установленного DSH GUI не подменяется запуском Minerva-сервера.

`npm audit` сообщает 10 moderate advisories в dev-цепочке lint/minimatch, 0 high
и 0 critical. Это не 10 независимых runtime-уязвимостей: advisory распространяется
на зависимые lint-пакеты. Принудительный downgrade или major override minimatch
не применялся; [снимок audit](../agent-artifacts/npm-audit-plugin-review.json).

Попытка workflow с `opencode-go/deepseek-v4-flash` вернула `null` без результата
агента. Исправления и проверки выполнены обычными доступными субагентами и
родительским агентом; работу DeepSeek-модели не заявляем.
