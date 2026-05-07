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
