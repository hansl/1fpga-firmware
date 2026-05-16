// React hooks wrapping the `1fpga:gui` input bindings. Each hook
// subscribes on mount + dependency change, unsubscribes on cleanup.
//
// Implementation note: useLayoutEffect (sync, fires during commit)
// instead of useEffect, because useEffect's passive flush relies on
// the scheduler firing — and even though we wired setTimeout into the
// runtime, React 19's reconciler defers passive effects past the
// render that triggered them. useLayoutEffect ensures listener
// registration completes before the runtime's first frame loop tick.

import { useLayoutEffect, useRef } from 'react';
import * as gui from '1fpga:gui';

export function useIntent(
  name: string,
  handler: (e: gui.IntentEvent) => void,
  opts?: gui.ListenerOpts,
): void {
  useLayoutEffect(() => {
    const id = gui.addIntentListener(name, handler, opts);
    return () => {
      gui.removeListener(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name, handler, opts?.global, opts?.nodeId]);
}

export function useRawInput(
  source: 'keyboard' | 'gamepad' | 'mouse',
  handler: (e: gui.RawInputEvent) => void,
  opts?: gui.ListenerOpts,
): void {
  useLayoutEffect(() => {
    const id = gui.addRawInputListener(source, handler, opts);
    return () => {
      gui.removeListener(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, handler, opts?.global, opts?.nodeId]);
}

/**
 * Drive `tick(now)` on every animation frame for the lifetime of the
 * mounted component. `tick` is given the current high-resolution
 * timestamp (ms since runtime start) and is expected to mutate styles
 * via `gui.setStyle` directly — the hook deliberately stays out of
 * React's commit path so per-frame updates don't trigger reconciler
 * work.
 *
 * The `tick` reference is captured once on mount and refreshed via a
 * ref so callers can pass a fresh closure every render without
 * resubscribing the RAF chain.
 */
export function useAnimationFrame(tick: (now: number) => void): void {
  const ref = useRef(tick);
  ref.current = tick;
  useLayoutEffect(() => {
    let cancelled = false;
    let handle = 0;
    const loop = (now: number) => {
      if (cancelled) return;
      ref.current(now);
      handle = gui.requestAnimationFrame(loop);
    };
    handle = gui.requestAnimationFrame(loop);
    return () => {
      cancelled = true;
      gui.cancelAnimationFrame(handle);
    };
  }, []);
}

/**
 * Animate one or more numeric style properties on a host node from
 * their current values to the supplied `target` over `opts.duration`
 * milliseconds. The actual interpolation runs **on the host** (Rust
 * AnimationManager): each loop iteration the manager advances all
 * tweens and applies the interpolated value via the same style-merge
 * path React's `gui.updateStyle` uses, so the damage system picks the
 * change up naturally. This avoids React re-rendering once per tween
 * tick — the component only re-renders when `target` itself changes.
 *
 * Pass a `ref` that you've attached to the element you want to
 * animate. The hook resolves the host `NodeId` from `ref.current` on
 * every dependency change. Re-targeting mid-animation glides from
 * the in-progress value rather than snapping.
 *
 * Example:
 * ```tsx
 * const ref = useRef<gui.NodeId>(null);
 * useTween(ref, { opacity: focused ? 1 : 0.5 }, { duration: 200 });
 * return <div ref={ref} />;
 * ```
 */
export function useTween(
  ref: { current: gui.NodeId | null },
  target: Partial<gui.Style>,
  opts?: gui.TweenOpts,
): void {
  // Cheap stable identity for the dep array — JSON.stringify is fine
  // because the objects are tiny and only contain primitives.
  const targetKey = JSON.stringify(target);
  const optsKey = JSON.stringify(opts ?? null);
  useLayoutEffect(() => {
    const node = ref.current;
    if (node == null) return;
    gui.startTween(node, target, opts);
    // No cleanup: a tween either completes on its own or is replaced
    // by the next dep-change. Letting one expire naturally is the
    // intended behaviour.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey, optsKey]);
}
