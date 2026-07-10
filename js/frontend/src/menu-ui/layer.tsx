// LayerPortal: render a subtree into a hardware scanout plane.
//
// A `layer`-marked div is partitioned out of the content framebuffer
// by the host: its children render into an offscreen surface and the
// scanout compositor blends that surface at the div's screen position
// (over wallpaper + content, by per-pixel alpha). The payoff is that
// MOVING the portal — a translateX/translateY tween — is a single
// position-register write per frame: zero blit traffic, no repaint of
// what it slides over.
//
// z is the plane-allocation rank, not a name: the highest-z portal
// that fits the hardware caps (width ≤ 4095, height ≤ 512) wins the
// (single, today) plane; every other portal renders as a normal div
// in the content layer — same pixels, graceful degradation. Multiple
// hardware planes later change the allocator, not this API.
//
// The portal's size is its surface size, INCLUDING parts that hang
// off-screen: content there is pre-rendered, and sliding reveals it
// without a repaint. Portal scale/rotate are unsupported (the plane
// blends 1:1); position comes from left/top plus translate.

import { forwardRef } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import type * as gui from '1fpga:gui';

export interface LayerPortalProps {
  /** Plane-allocation rank; higher wins. Must be > 0. */
  z?: number;
  /** Screen position of the surface's top-left (before translate). */
  x: number;
  y: number;
  /** Surface size. Hardware caps: width ≤ 4095, height ≤ 512. */
  width: number;
  height: number;
  /** Extra styles (e.g. an initial translateX for slide tweens). */
  style?: CSSProperties;
  children?: ReactNode;
}

export const LayerPortal = forwardRef<gui.NodeId, LayerPortalProps>(
  function LayerPortal({ z = 1, x, y, width, height, style, children }, ref) {
    return (
      <div
        ref={ref as never}
        style={{
          position: 'absolute',
          left: x,
          top: y,
          width,
          height,
          layer: z,
          ...style,
        }}
      >
        {children}
      </div>
    );
  },
);
