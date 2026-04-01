import commonjs from '@rollup/plugin-commonjs';
import { nodeResolve } from '@rollup/plugin-node-resolve';
import replace from '@rollup/plugin-replace';
import terser from '@rollup/plugin-terser';
import typescript from '@rollup/plugin-typescript';
import del from 'rollup-plugin-delete';

const production = !('NODE_ENV' in process.env) || process.env.NODE_ENV === 'production';

export default {
  input: 'src/main.ts',
  output: {
    dir: 'dist/',
    format: 'es',
    sourcemap: !production,
  },
  plugins: [
    del({ targets: 'dist/*' }),
    replace({
      preventAssignment: true,
      'process.env.NODE_ENV': JSON.stringify(production ? 'production' : 'development'),
      'process.emit': 'undefined',
    }),
    nodeResolve({ preferBuiltins: false }),
    typescript({
      tsconfig: './tsconfig.json',
      exclude: ['*.config.ts'],
    }),
    commonjs({
      extensions: ['.js', '.ts', '.cjs'],
      transformMixedEsModules: true,
    }),
    ...(production
      ? [
          terser({
            compress: { ecma: 2020, module: true, passes: 2 },
            ecma: 2020,
            mangle: true,
          }),
        ]
      : []),
  ],
  external: [/^1fpga:/],
  onLog(level, log, handler) {
    // Suppress TypeScript warnings about custom JSX elements (view, text, image)
    // conflicting with React's SVG type definitions.
    if (log.code === 'PLUGIN_WARNING' && log.plugin === 'typescript' && log.message?.includes('TS2322')) {
      return;
    }
    // Suppress missing type declaration for react-reconciler
    if (log.code === 'PLUGIN_WARNING' && log.plugin === 'typescript' && log.message?.includes('TS7016')) {
      return;
    }
    handler(level, log);
  },
};
