// React hooks wrapping the `1fpga:gui` input bindings. Each hook
// subscribes on mount + dependency change, unsubscribes on cleanup.
//
// Implementation note: useLayoutEffect (sync, fires during commit)
// instead of useEffect, because useEffect's passive flush relies on
// the scheduler firing — and even though we wired setTimeout into the
// runtime, React 19's reconciler defers passive effects past the
// render that triggered them. useLayoutEffect ensures listener
// registration completes before the runtime's first frame loop tick.

import { useLayoutEffect } from 'react';
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
