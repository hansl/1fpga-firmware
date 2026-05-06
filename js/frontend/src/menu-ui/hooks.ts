// React hooks wrapping the `1fpga:gui` input bindings. Each hook
// subscribes on mount + dependency change, unsubscribes on cleanup.
//
// Implementation notes:
// - useLayoutEffect (sync, fires during commit) instead of useEffect.
//   Boa has no scheduler; passive effects would never run.
// - Handlers are wrapped in `flushAfter` so setState calls inside
//   them commit synchronously. Without this wrap, React queues the
//   update but never flushes it to a re-render.

import { useLayoutEffect } from 'react';
import * as gui from '1fpga:gui';
import { flushAfter } from './reconciler';

export function useIntent(
  name: string,
  handler: (e: gui.IntentEvent) => void,
  opts?: gui.ListenerOpts,
): void {
  useLayoutEffect(() => {
    const wrapped = (e: gui.IntentEvent) => flushAfter(() => handler(e));
    const id = gui.addIntentListener(name, wrapped, opts);
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
    const wrapped = (e: gui.RawInputEvent) => flushAfter(() => handler(e));
    const id = gui.addRawInputListener(source, wrapped, opts);
    return () => {
      gui.removeListener(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, handler, opts?.global, opts?.nodeId]);
}
