# Проверки и форматирование кода

Используйте Node 24 LTS: ESLint 10 и часть tooling не поддерживают системный Node 23.

```sh
npm run lint:fix
npm run format
npm run lint
npm run format:check
npm test
npm run contracts:check
npm run build
```

## Что проверяется

- JS, JSX, TS, TSX, MJS, CJS, MTS и CTS: базовые correctness rules и TypeScript ESLint.
- React: порядок hooks, полнота зависимостей, ключи JSX, опасные DOM-конструкции,
  конфликтующие children и недопустимые JSX namespaces.
- Неиспользуемые переменные и зависимости hooks — ошибки; `lint` не допускает warnings.
- Prettier встроен в ESLint fix для кода. Отдельная команда format также приводит
  CSS и JSON-конфигурации к тому же стилю: два пробела, точки с запятой, одинарные
  кавычки в JS/TS, переносы LF и ширина строки 100.

[Конфигурация ESLint](../eslint.config.mjs), [стиль Prettier](../.prettierrc.json)
и [исключения](../.prettierignore) общие для приложения, пакетов, UI-плагинов,
скриптов и тестов. Выходные каталоги, зависимости, generated client, lock-файл,
иконки и пользовательские assets не форматируются. Rust не обрабатывается ESLint.
[Git attributes](../.gitattributes) закрепляют LF для JS/TS/CSS/JSON, чтобы Windows
checkout с autocrlf не создавал повторные ошибки форматирования.

`no-explicit-any` намеренно не включён в эту механическую очистку: замена `any`
в transport/LSP/SDK boundaries требует отдельной типизации и runtime validation,
а не автоматической подстановки `unknown`. Это не отменяет существующие
runtime-проверки. Compiler-specific миграции React и изменение публичных API
ради подавления style diagnostics также не включены.

[Регрессионные тесты конфигурации](../tests/lint-config.test.mjs) проверяют TSX,
hooks, missing keys, unused variables, autofix и исключения. Автофикс не решает
за разработчика вопросы lifetime: эффекты и memoization проверяются отдельно,
чтобы форматирование не сбрасывало историю чата и выполняющиеся запросы.

## Результат общей очистки

- `lint`: 0 ошибок и 0 предупреждений; `format:check` проходит.
- 123 Node-теста проходят, включая четыре теста самой lint-конфигурации.
- TypeScript, production build и `contracts:check` проходят.
- Семь layout-сценариев и пять сценариев чатов/workspace (web и эмуляция desktop IPC)
  проходят. Для последних исправлены устаревший Tauri mock и ожидание перенесённого
  в composer поля доступа; настоящие native-окна этим не проверяются.
- Предупреждения Vite о `use client`, sourcemaps и больших chunks остаются;
  они не подавлялись настройками ESLint.
