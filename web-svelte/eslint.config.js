import js from '@eslint/js'
import { defineConfig } from 'eslint/config'
import globals from 'globals'
import svelte from 'eslint-plugin-svelte'
import ts from 'typescript-eslint'
import svelteConfig from './svelte.config.js'

export default defineConfig(
  { ignores: ['dist/', 'node_modules/'] },
  js.configs.recommended,
  ...ts.configs.strictTypeChecked,
  ...svelte.configs.recommended,
  {
    languageOptions: {
      globals: { ...globals.browser },
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: ['.svelte'],
      },
    },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: {
      parserOptions: { parser: ts.parser, svelteConfig },
    },
  },
  {
    rules: {
      // Numbers and booleans stringify predictably; everything else must be explicit.
      '@typescript-eslint/restrict-template-expressions': [
        'error',
        { allowNumber: true, allowBoolean: true },
      ],
      // `onclick={() => doIt()}` and `void promise` are the house style for handlers.
      '@typescript-eslint/no-confusing-void-expression': [
        'error',
        { ignoreArrowShorthand: true, ignoreVoidOperator: true },
      ],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      // `void signal` is how $effect bodies register a dependency without using the value.
      '@typescript-eslint/no-meaningless-void-operator': 'off',
      // Flags every local `new Map/Set/URLSearchParams/Date`, including ones that are
      // built and consumed inside a single function and never reach reactive state.
      'svelte/prefer-svelte-reactivity': 'off',
      // Most unkeyed lists here are static option lists or derived display rows without a
      // unique field; keying them by a non-unique value throws each_key_duplicate at runtime.
      'svelte/require-each-key': 'off',
    },
  },
  {
    // svelte-check stays the type authority for components. typescript-eslint sees an
    // imported `.svelte` component as `any`, and cannot see `bind:this` assignments, so
    // these rules produce false "unsafe" / "always falsy" reports inside components.
    files: ['**/*.svelte'],
    rules: {
      '@typescript-eslint/no-unsafe-argument': 'off',
      '@typescript-eslint/no-unsafe-assignment': 'off',
      '@typescript-eslint/no-unsafe-call': 'off',
      '@typescript-eslint/no-unsafe-member-access': 'off',
      '@typescript-eslint/no-unsafe-return': 'off',
      '@typescript-eslint/no-unnecessary-condition': 'off',
    },
  },
  {
    files: ['**/*.test.ts', 'src/test/**'],
    rules: {
      // `expect(api.method)` / `vi.mocked(api.method)` reference methods without calling them.
      '@typescript-eslint/unbound-method': 'off',
      // Mock implementations are `async` to match the mocked signature.
      '@typescript-eslint/require-await': 'off',
      // Tests index into fixtures and DOM queries they have just asserted on.
      '@typescript-eslint/no-non-null-assertion': 'off',
    },
  },
  {
    // Plain-JS tool configs are outside the TypeScript project.
    files: ['*.config.js'],
    extends: [ts.configs.disableTypeChecked],
    languageOptions: { globals: { ...globals.node } },
  },
)
