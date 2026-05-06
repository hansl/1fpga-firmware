import commonjs from '@rollup/plugin-commonjs';
import json from '@rollup/plugin-json';
import { nodeResolve } from '@rollup/plugin-node-resolve';
import replace from '@rollup/plugin-replace';
import terser from '@rollup/plugin-terser';
import typescript from '@rollup/plugin-typescript';
import del from 'rollup-plugin-delete';

import * as child_process from 'node:child_process';

import codegen from './rollup/rollup-plugin-codegen.js';
import constants from './rollup/rollup-plugin-consts.js';
import dbMigrations from './rollup/rollup-plugin-db-migrations.js';
import {
  transformCommonTags,
  transformTaggedTemplate,
} from './rollup/rollup-plugin-template-literals.js';

const production = !('NODE_ENV' in process.env) || process.env.NODE_ENV === 'production';

const gitRev = child_process
  .execSync('git describe --all --always --dirty')
  .toString()
  .trim()
  .replace(/^.*\//, '');

function commonPlugins() {
  return [
    codegen(),
    dbMigrations(),
    // Boa has no `process` global. React + react-reconciler are full
    // of `process.env.NODE_ENV === 'development'` guards; substitute
    // at build time so terser DCE removes the dev branches entirely.
    replace({
      preventAssignment: true,
      values: {
        'process.env.NODE_ENV': JSON.stringify(production ? 'production' : 'development'),
      },
    }),
    nodeResolve({
      preferBuiltins: false,
    }),
    constants({
      environment: process.env.NODE_ENV,
      production,
      revision: gitRev,
    }),
    typescript({
      tsconfig: './tsconfig.json',
      exclude: ['src/**/*.spec.ts', 'src/**/*.test.ts', '*.config.ts'],
      compilerOptions: {
        declaration: !production,
      },
    }),
    json({}),
    commonjs({
      extensions: ['.js', '.ts', '.cjs'],
      transformMixedEsModules: true,
    }),
    ...(production
      ? [
          transformTaggedTemplate({
            tagsToProcess: ['sql', 'sql1', 'sql2', 'sql3', 'sql4', 'sql5', 'sql6', 'sql7'],
            transformer: sql => {
              return sql.replace(/\n/g, ' ').replace(/\s\s+/g, ' ');
            },
          }),
          transformCommonTags('oneLine'),
          transformCommonTags('source'),
          transformCommonTags('stripIndent'),
          transformCommonTags('stripIndents'),
        ]
      : []),
    ...(production
      ? [
          terser({
            compress: {
              arguments: true,
              ecma: 2020,
              module: true,
              passes: 2,
              pure_new: true,
              unsafe: true,
              unsafe_arrows: true,
              unsafe_comps: true,
              unsafe_math: true,
            },
            ecma: 2020,
            mangle: true,
          }),
        ]
      : []),
  ];
}

const onLog = (level, log, handler) => {
  if (log.code === 'CIRCULAR_DEPENDENCY') {
    handler('warn', log);
  } else if (level === 'warn') {
    handler('error', log);
  } else {
    handler(level, log);
  }
};

// Two independent Rollup configs so each entry produces a fully
// self-contained ESM bundle (no shared chunks). The menu-ui runtime
// loads its bundle as a single file via `--bundle <path>`, so any
// chunked output would break at load time.
export default [
  {
    input: { main: 'src/main.ts' },
    output: {
      dir: 'dist/',
      format: 'es',
      sourcemap: !production,
      hoistTransitiveImports: false,
      entryFileNames: '[name].js',
    },
    plugins: [del({ targets: 'dist/*' }), ...commonPlugins()],
    external: [/^1fpga:/],
    onLog,
  },
  {
    input: 'src/menu-ui/index.tsx',
    output: {
      file: 'dist/menu_ui.js',
      format: 'es',
      sourcemap: !production,
      inlineDynamicImports: true,
    },
    // Skip terser for menu-ui during N2 bring-up so JS error stacks
    // remain readable on device. Re-enable once stable.
    plugins: commonPluginsUnminified(),
    external: [/^1fpga:/],
    onLog,
  },
];

function commonPluginsUnminified() {
  return commonPlugins().filter((p) => p && p.name !== 'terser');
}
