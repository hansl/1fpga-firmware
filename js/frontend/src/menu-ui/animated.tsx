/**
 * react-spring shim for the menu-ui host.
 *
 * react-spring ships a `createHost` API that lets non-DOM targets plug
 * in their own animated-value applicator. We use it to route animated
 * style updates straight into `gui.setStyle`, bypassing React's commit
 * path entirely (so per-frame interpolation costs zero reconciler
 * work).
 *
 * The spring's internal frame loop is wired to our
 * `gui.requestAnimationFrame` via `Globals.assign` so it ticks in step
 * with the runtime's frame loop instead of trying to use the absent
 * browser global.
 *
 * Surface re-exported below: `useSpring`, `useSprings`, `useTrail`,
 * `useTransition`, `useChain`, `useSpringValue`, `config` (preset
 * spring configs), and the `animated` proxy.
 */

import { createHost } from '@react-spring/animated';
import { Globals } from '@react-spring/shared';
import * as gui from '1fpga:gui';

// react-spring resolves to a numeric handle for animation frames; our
// gui.requestAnimationFrame returns a non-zero u32 with the same
// shape, so it drops in as a direct replacement for the absent global.
//
// `now` is overridden alongside RAF so the spring's frame-time deltas
// (computed via I.now()) live on the same monotonic clock as the
// timestamps passed to RAF callbacks. Date.now() in Boa is wall-clock
// ms; that differs from the Rust-side `runtime_elapsed` baseline used
// by gui.requestAnimationFrame's callback arg, but spring math works
// off deltas — both clocks advance at the same rate, so deltas match
// up to clock resolution.
Globals.assign({
  requestAnimationFrame: gui.requestAnimationFrame,
  now: () => Date.now(),
});

const host = createHost(['div', 'img'], {
  /**
   * Apply the latest props (with animated values resolved) to a host
   * node. react-spring delivers only the *animated* keys here — every
   * other prop on `<animated.div style={...}>` is left to the host's
   * normal create/commit path. We therefore use `gui.updateStyle`
   * (merge into existing) rather than `gui.setStyle` (replace), so
   * the node's static layout stays intact while colours, opacities,
   * etc., tween from frame to frame.
   *
   * Returning truthy tells react-spring it doesn't need to fall back
   * to DOM mutation — there's no DOM.
   */
  applyAnimatedValues(node: unknown, props: Record<string, unknown>) {
    if (typeof node !== 'number') return true;
    const style = props.style;
    if (style && typeof style === 'object') {
      gui.updateStyle(node as gui.NodeId, style as Partial<gui.Style>);
    }
    return true;
  },
});

export const animated = host.animated;

export {
  config,
  useChain,
  useSpring,
  useSpringRef,
  useSpringValue,
  useSprings,
  useTrail,
  useTransition,
} from '@react-spring/core';
