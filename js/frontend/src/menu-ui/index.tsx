// menu-ui entry — N2: React reconciler integration.
//
// JSX renders through `react-reconciler` into the Rust-side host tree
// via `1fpga:gui`. Same visual outcome as N1 (full-screen dark blue +
// red box), but the tree is now driven by React.

import * as gui from '1fpga:gui';

import { App } from './App';
import { render } from './reconciler';

// Printable ASCII (32-126) plus a few unicode glyphs we use in the
// demo (arrows, middle dot). Pre-warming all sizes used by the App's
// styles eliminates per-nav atlas rebuilds — the user-visible frame
// rate is steady from the very first nav instead of dipping while
// each new char rasterises.
const ASCII =
  ' !"#$%&\'()*+,-./0123456789:;<=>?@' +
  'ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`' +
  'abcdefghijklmnopqrstuvwxyz{|}~';
const SYMBOLS = '←→·';
const DEMO_CHARS = ASCII + SYMBOLS;
const DEMO_SIZES = [20, 24, 28, 32, 48, 72];

export async function main(): Promise<void> {
  gui.warmupGlyphs('default', DEMO_SIZES, DEMO_CHARS);
  render(<App />);
}
