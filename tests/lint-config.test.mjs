import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { ESLint } from 'eslint';
import prettier from 'prettier';

const cwd = fileURLToPath(new URL('../', import.meta.url));
const eslint = new ESLint({ cwd });

async function lint(source, extension = 'tsx') {
  const filePath = `apps/web/src/lint-config-fixture.${extension}`;
  const options = await prettier.resolveConfig(new URL('../.prettierrc.json', import.meta.url));
  const formatted = await prettier.format(source, { ...options, filepath: filePath });
  const [result] = await eslint.lintText(formatted, { filePath });
  return result.messages;
}

test('React TSX accepts typed hooks and correctly keyed JSX', async () => {
  const messages = await lint(`
    import { useState } from 'react';
    export function Counter() {
      const [count, setCount] = useState<number>(0);
      return <button type="button" onClick={() => setCount(count + 1)}>{count}</button>;
    }
  `);
  assert.deepEqual(messages, []);
});

test('React rules reject conditional hooks, missing dependencies and missing keys', async () => {
  const messages = await lint(`
    import { useEffect, useState } from 'react';
    export function Broken({ enabled, items }: { enabled: boolean; items: string[] }) {
      if (enabled) useState(0);
      useEffect(() => { console.log(enabled); }, []);
      return <ul>{items.map(item => <li>{item}</li>)}</ul>;
    }
  `);
  const rules = new Set(messages.map((message) => message.ruleId));
  assert.ok(rules.has('react-hooks/rules-of-hooks'));
  assert.ok(rules.has('react-hooks/exhaustive-deps'));
  assert.ok(rules.has('@eslint-react/no-missing-key'));
});

test('unused TypeScript variables are errors, not a warning-only baseline', async () => {
  const messages = await lint('export const visible: number = 1; const forgotten = 2;', 'ts');
  assert.ok(
    messages.some(
      (message) => message.ruleId === '@typescript-eslint/no-unused-vars' && message.severity === 2,
    ),
  );
});

test('autofix formats JSX and ignores generated files, build outputs and user assets', async () => {
  const fixer = new ESLint({ cwd, fix: true });
  const [result] = await fixer.lintText('export const view=<div>Ready</div>', {
    filePath: 'apps/web/src/lint-config-fixture.tsx',
  });
  assert.equal(result.output, 'export const view = <div>Ready</div>;\n');
  assert.deepEqual(result.messages, []);
  for (const path of [
    'packages/client/src/generated.ts',
    'dist/bundle.js',
    'agent-artifacts/fixture.tsx',
    'assets/example.js',
    'output/example.js',
  ]) {
    assert.equal(await eslint.isPathIgnored(path), true, path);
  }
});
