# Minerva IDE

Этот документ фиксирует этап интеграции от 20 ноября 2026 года, включая прежний MH plugin host. Целевая архитектура встроенного агента и приёмка текущего MVP описаны в [плане от 24 ноября](minerva-agent-mvp.md).

Рабочая область справа от чата и контракт вкладок плагинов описаны в [minerva-workspace.md](minerva-workspace.md).

Рабочая основа — OpenIDEA в отдельной папке `C:\dev\minerva-ide`, ветка
`main` в `kviron/minerva-harness`, upstream `l0ng-ai/openidea`, baseline
`e67613895608cff06bdb69f9bcb69974b9ed00f8`. Прежняя самостоятельная реализация сохранена в ветке `clear-harness`.
Его snapshot `d3613ef9eccecca1282cb35472accd20b8901dad` находится в
`vendor/minerva`; происхождение и изменения перечислены в `vendor/minerva/IMPORT.md`.
Лицензия и NOTICE OpenIDEA сохранены.

## Владельцы состояния

- OpenIDEA владеет окнами, Project, редактором CodeMirror, LSP, терминалом,
  Git, задачами, отображением истории и нативными драйверами Claude/Codex.
- Minerva runtime работает внутри Rust backend. Он владеет прямыми API-моделями,
  исполнением своего агента, permissions/questions и своим plugin host.
- `ide-minerva::Service` запускает runtime лениво и завершает его вместе с IDE.
  `ide-host` объявляет IPC через обычный upstream domain и генерируемые bindings.
- Внутренний адаптер `ide-agent/src/native/minerva.rs` преобразует события в
  штатный chat OpenIDEA. Внешний ACP-процесс Minerva не запускается.

Chat и Development используют один chat/store. Development размещает Project
слева, редактор по центру и Agents справа. Chat делает разговор основной областью.
Новые элементы используют UI-компоненты и токены OpenIDEA. Старый frontend
Minerva не монтируется; из него переиспользуется контракт plugin SDK.

Уточнение 2026-11-20: основной режим теперь называется Agent и открывается по
умолчанию. Переключатель Agent / Development находится в titlebar. Minerva
не предлагается как подключение: в picker показываются настроенные API
провайдеры и model-engine пакеты, затем модели только выбранного подключения.
Внутренний legacy id `minerva` сохранён для совместимости уже созданных задач.

В Agent слева находятся Chats, Plugins и Settings, рядом — постоянная колонка
Chat history с поиском и New chat. IDE stripes и панели скрываются на уровне
представления; editor DOM и сохранённая раскладка инструментов не заменяются.
Development возвращает штатные панели. Верхний run/debug toolbar и статусная
строка пока остаются общими для обоих режимов.

Ширина истории меняется перетаскиванием правой границы или стрелками на
разделителе и сохраняется в настройке `minerva.chatHistory.width` браузерного
профиля. Размер ограничен, чтобы оставалось место для разговора.

Независимая проверка на browser fixtures (1440×960 и 1100×800) подтвердила поиск,
выбор чатов, New chat, открытие настроек и сохранение task/messages/draft при
переключениях. Ошибок страницы нет. Реальные backend/model вызовы в этой
визуальной проверке не использовались. Сборка и Safari checks прошли.

## Проекты

Доверенный корень выбранного проекта регистрируется через исходный
`workspaces.create`: канонизация и дедупликация остаются в Minerva. Сессия
проверяется на принадлежность workspace при resume. При worktree-задаче
execution workspace соответствует рабочему каталогу worktree, а родительский
проект остаётся в Task OpenIDEA. Черновики текста, файлов и base изолированы
по корню проекта и восстанавливаются при A → B → A.

Данные новой программы изолированы от старого приложения. Автоматического
переноса старой истории и секретов нет. Debug launcher дополнительно изолирует
идентификатор по checkout; release identifier — `ru.minerva.ide`.

## Модели и плагины

Settings → Agents → Model Providers: прямые провайдеры OpenAI Chat/Responses, Anthropic и Ollama;
секрет сохраняется исходным backend, обратно в форму не выводится. Имена
API-провайдеров и model-engine плагинов попадают в штатный provider picker;
model picker ограничен соответствующим `api:<id>/` или `plugin:<id>/`. Claude
и Codex используют существующие драйверы OpenIDEA.

Пакеты устанавливаются исходным Minerva installer с проверкой manifest/digest,
в отключённом состоянии. После установки нужен перезапуск host, о чём форма
сообщает. Включение/отключение и backend-вызовы обслуживает исходный plugin host.
UI загружается только из проверенного пакета, с общей React и адаптером native
Button/Input/Select. Подключены settings.pages и панели canvas/workspace/shell.

Инвентаризированы `C:\dev\minerva-codex-plugin` и
`C:\dev\minerva-home-plugin`: их UI использует settings.pages, Button/Input и
compound Select, которые покрыты адаптером. Это не обещание совместимости
всех возможных legacy-слотов: context menus, произвольные SDK service consumers
и остальные слоты требуют отдельной проверки. Сами пользовательские пакеты
пока не установлены в новую IDE.
Собственный HTML пакетов (например checkbox/progress Codex) остаётся внутри
пакета; layout адаптирован к host tokens, но это не замена каждого HTML-узла
компонентом OpenIDEA.

## Запуск

Нужны Rust, системные зависимости Tauri, Node 24+ и pnpm. Из корня:

```powershell
pnpm install --frozen-lockfile
pnpm dev:app C:\path\to\project
pnpm dev:status
```

Использовать официальный wrapper `dev:app`: он выбирает свободный порт и
изолирует dev instance. Upstream updater отключён, чтобы fork не обновился
чужим бинарником. Бинарник для распространения и подпись не подготовлены.

## Проверки и ограничения

Последующее исправление Codex: обнаружение на Windows учитывает CLI внутри
установленного MSIX-приложения и standalone, сравнивая версии с PATH.
В этом окружении PATH давал 0.164.0, а приложение — 0.171.0-alpha.13. Реальный
`model/list` нового CLI вернул gpt-6.2-sol и gpt-6.2-astra. Служебные hidden
модели не выдаются за обычные чат-модели.

В пользовательском `~/.codex/config.toml` три ошибочно вложенных параметра
Kimi находились в `hooks.state` stop-hook. Удалён только этот блок с резервной
копией `config.toml.before-hook-model-fix-20261120-150315.bak`. Рабочая корневая
модель и hooks сохранены; после исправления real app-server сообщил 0 config
warnings. WebSocket маршрут делал повторные подключения, поэтому только в
профиле этой dev IDE добавлен `agents/codex.toml` с native Codex driver и
OpenAI HTTPS provider через `-c`. Он использует существующий ChatGPT login;
секреты не копируются, глобальный transport не изменён. Контрольный ephemeral
turn gpt-6.2-sol: completed, 0 config warnings, 0 error events. Model/list на
том же HTTPS профиле также вернул современные модели. При смене ChatGPT
login на API key этот локальный override следует удалить или перенастроить.

Проверены production frontend build, Safari syntax/API checks, генерация
Tauri bindings, targeted frontend tests и исходные SDK tests. Независимый
визуальный review выполнен на явно обозначенном browser fixture: общий стиль,
Chat/Development, узкая ширина, labels и A/B/A drafts. Fixture не доказывает
работу native IPC или LSP.
Дополнительно независимо отрисованы реальные compiled UI Codex и Minerva Home:
экраны account/models, изменение compound Select и отсутствие ошибок страницы.
Найденные технические ID в заголовках Settings заменены именами пакетов.

Последний Rust-прогон: `ide-agent` 355 passed, 5 ignored, 1 filtered;
`ide-minerva` 3 passed. Последний targeted frontend-прогон: 6 файлов, 102 passed,
6 skipped. Исходный plugin SDK: ранее 9 passed. Генератор bindings: 2 passed.
Knip оставляет два upstream замечания: `Chip` и `DragCursor`; новые интеграционные
модули не отмечены как неиспользуемые. Lint новых модулей и TypeScript прошли.
Нативное приложение пересобрано и запущено для `C:\dev\minerva-harness`.
Наблюдался штатный Trust Project dialog; сквозной UI-прогон после него не
завершён из-за конфликтующего ручного ввода. Независимые browser fixtures
подтвердили Agent startup, верхний selector, сохранение task/history/draft при
Agent → Development → Agent, читаемые provider/model menus и переход в Model
Providers. Backend SSE fixture проверен отдельно автоматическим тестом;
реальный Codex app-server turn по HTTPS проверен отдельно от native UI.

Реальный backend smoke устанавливает собранный review-plugin, включает его,
вызывает `review.summary`, отключает и проверяет отказ вызова; затем завершает
runtime. Native-adapter tests используют локальный HTTP SSE fixture, проверяют
streaming, PNG attachment, validation, cancellation и изоляцию сессий.

Один upstream agent test `deleting_a_task_waits_for_its_agent_to_exit_before_removing_the_worktree`
завис на Windows и исключён из повторного прогона. Это открытая проверка,
а не прошедший тест. Полный upstream suite, реальные Claude и прямые API
запросы, интерактивный LSP smoke не проверены. Новый адаптер не объявляет fork/rollback/steer или
OpenIDEA MCP-server capabilities; Minerva использует собственные tools/MCP.
Исторический replay адаптера передаёт text/reasoning; tool/image history
опирается на сохранённый timeline OpenIDEA.

Артефакты локальных проверок находятся в игнорируемом `test-results`.

### Изменение разрешений во время API-запуска

Native adapter меняет только `permissionPolicy` через локальное действие
`workspace.set_permission_policy`. Полная запись workspace во время запуска
по-прежнему запрещена. Атомарное обновление сохраняет остальные настройки.

Доверенный Rust-вход `start_native_api_run` разрешает пользовательское изменение
политики в обе стороны. Обычные и фоновые запуски сохраняют исходный потолок
полномочий; JSON-параметры не могут его повысить. Инструмент читает актуальную
политику перед выполнением и после ответа на подтверждение. Открытый запрос Ask
требует явного ответа; уже начатые побочные эффекты не отменяются сменой режима.

Native SSE tests проверяют ReadOnly/Ask → Allow → ReadOnly внутри одного запуска,
сохранение workspace и отказ поддельному JSON-повышению полномочий. Дополнительно
проверены атомарный setter, локальная маршрутизация и ограничения обычных запусков.
