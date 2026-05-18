// menu-ui entry — N2: React reconciler integration.
//
// JSX renders through `react-reconciler` into the Rust-side host tree
// via `1fpga:gui`. Same visual outcome as N1 (full-screen dark blue +
// red box), but the tree is now driven by React.

import * as gui from '1fpga:gui';

import { App } from './App';
import { ALL_ICON_CODEPOINTS } from './components/Icon';
import { render } from './reconciler';
import { s } from './scale';

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
/** Every default-font size used across the App's components, in
 *  design units (1080p reference). Passed through `s()` so the
 *  atlases match the actual pixel sizes the layout will request at
 *  this viewport. Sourced by grepping `fontSize:` in the components
 *  — keep in sync when new sizes are introduced. */
const DEFAULT_DESIGN_SIZES = [16, 20, 22, 24, 26, 28, 32, 36, 48, 72];
/** Icon sizes used in components (StatusBar/Notifications use 24,
 *  ActionBar uses 22). */
const ICON_DESIGN_SIZES = [22, 24];

function dedup(xs: number[]): number[] {
  const seen = new Set<number>();
  const out: number[] = [];
  for (const x of xs) {
    if (x > 0 && !seen.has(x)) {
      seen.add(x);
      out.push(x);
    }
  }
  return out;
}

export async function main(): Promise<void> {
  // After scaling, multiple design sizes may collapse to the same
  // px size at small viewports (e.g. 22 and 24 both round to 17 at
  // 720p) — dedup before requesting atlases.
  const defaultSizes = dedup(DEFAULT_DESIGN_SIZES.map(s));
  const iconSizes = dedup(ICON_DESIGN_SIZES.map(s));
  gui.warmupGlyphs('default', defaultSizes, DEMO_CHARS);
  gui.warmupGlyphs('icons', iconSizes, ALL_ICON_CODEPOINTS);
  render(<App />);
}
