// Viewport-aware scaling helpers.
//
// All component literals (font sizes, slot widths, padding, etc.) are
// written against a 1920×1080 design reference. At runtime, every
// pixel value passes through `s()` which scales it by the actual
// viewport's height ratio. The same JS bundle renders correctly at
// any render resolution the host configures — 1080p, 720p, 480p,
// 320×240, etc. — because the host's framework / ASCAL pipeline
// stretches whatever the FB contains to the active HDMI mode.
//
// Why scale by *height* and not width: most UI is laid out in vertical
// rhythm (row height, font size, padding). Scaling by height keeps
// that rhythm proportional even at non-16:9 viewports — a 4:3 render
// target won't look squashed vertically, just narrower horizontally.

import * as gui from '1fpga:gui';

const vp = gui.viewport();

/** Render-target dimensions in actual pixels. */
export const VW = vp.width;
export const VH = vp.height;

/** Design reference. Every literal in component code is in these units. */
const DESIGN_H = 1080;
const DESIGN_W = 1920;

/** Scale factor: 1.0 at 1080p, 0.667 at 720p, 0.444 at 480p, etc. */
export const SCALE = VH / DESIGN_H;

/** Scale a design-units value to actual viewport pixels. Rounded to
 *  the nearest integer so Taffy (which takes integer px) doesn't see
 *  fractional dimensions. */
export function s(v: number): number {
  return Math.round(v * SCALE);
}

/** Fraction of viewport width — handy for "centre this at 50 %". */
export function vw(frac: number): number {
  return Math.round(VW * frac);
}

/** Fraction of viewport height. */
export function vh(frac: number): number {
  return Math.round(VH * frac);
}

/** The viewport's logical centre in actual pixels — useful for
 *  X-centring strips. Equivalent to `vw(0.5)`. */
export const CENTRE_X = vw(0.5);

/** Map a design X position (against 1920) to actual pixels. Use this
 *  for absolute X coordinates that should land at the same *relative*
 *  horizontal location regardless of the viewport's aspect ratio.
 *  (Pure `s(x)` scales by height; this scales by width.) */
export function sx(designX: number): number {
  return Math.round((designX / DESIGN_W) * VW);
}
