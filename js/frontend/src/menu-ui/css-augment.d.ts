// Augment React's CSSProperties with the menu-ui host's custom paint
// transforms.
//
// `translateX` / `translateY` are NOT standard CSS properties (CSS only
// has the `translate` shorthand and the translate*() transform
// functions), so csstype — and therefore React.CSSProperties — doesn't
// know them. The 1fpga:gui host, however, treats them as first-class
// paint-only transform offsets (see `gui.Style` and `style.rs`'s
// `translate_x/translate_y`): animating them slides an element or whole
// subtree without ever triggering a Taffy reflow.
//
// Declaring them here lets components set them inline on `<div style>`
// without a cast — exactly the way `scale` / `rotate` (which csstype
// already ships) are used in MenuBar/MenuColumn.
import 'react';

declare module 'react' {
  interface CSSProperties {
    translateX?: number;
    translateY?: number;
    /** Hardware-layer z rank (LayerPortal) — see gui.Style.layer. */
    layer?: number;
  }
}
