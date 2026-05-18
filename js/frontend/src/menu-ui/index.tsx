// menu-ui entry — N2: React reconciler integration.
//
// JSX renders through `react-reconciler` into the Rust-side host tree
// via `1fpga:gui`. Same visual outcome as N1 (full-screen dark blue +
// red box), but the tree is now driven by React.

import * as gui from '1fpga:gui';

import { App } from './App';
import { ALL_ICON_CODEPOINTS } from './components/Icon';
import { render } from './reconciler';

// Printable ASCII (32-126) plus a few unicode glyphs the demo uses
// (arrows, middle dot). The list of *sizes* below must mirror every
// fontSize that survives into a text node — if any (font, size) used
// by the app is missing here, the atlas builds lazily on first use
// AND rebuilds whenever a new char appears, costing ~10-20 ms in
// `text_prep` per nav. The frame-timing log will show those rebuilds
// as repeated `fontatlas: <name> @ <size>px — <N> glyphs` lines.
const ASCII =
  ' !"#$%&\'()*+,-./0123456789:;<=>?@' +
  'ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`' +
  'abcdefghijklmnopqrstuvwxyz{|}~';
const SYMBOLS = '←→·';
const DEMO_CHARS = ASCII + SYMBOLS;
/** Every default-font size used across the App's components. Sourced
 *  by hand from grepping `fontSize:` in `js/frontend/src/menu-ui/` —
 *  add new sizes here when you introduce them. */
const DEFAULT_SIZES = [16, 20, 22, 24, 26, 28, 32, 36, 48, 72];
/** Icon sizes used in components (StatusBar uses 24, ActionBar uses 22). */
const ICON_SIZES = [22, 24];

export async function main(): Promise<void> {
  gui.warmupGlyphs('default', DEFAULT_SIZES, DEMO_CHARS);
  gui.warmupGlyphs('icons', ICON_SIZES, ALL_ICON_CODEPOINTS);
  render(<App />);
}
