// React hooks wrapping the `1fpga:gui` input bindings. Each hook
// subscribes on mount + dependency change, unsubscribes on cleanup.

import { useEffect } from 'react';
import * as gui from '1fpga:gui';

/**
 * Subscribe to a high-level intent (e.g. `'confirm'`, `'navigate_up'`).
 * `handler` fires on every press / repeat / release; gate on
 * `e.kind === 'pressed'` if you only want presses.
 */
export function useIntent(
  name: string,
  handler: (e: gui.IntentEvent) => void,
  opts?: gui.ListenerOpts,
): void {
  useEffect(() => {
    const id = gui.addIntentListener(name, handler, opts);
    return () => {
      gui.removeListener(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [name, handler, opts?.global, opts?.nodeId]);
}

/** Subscribe to raw events from a given input source. */
export function useRawInput(
  source: 'keyboard' | 'gamepad' | 'mouse',
  handler: (e: gui.RawInputEvent) => void,
  opts?: gui.ListenerOpts,
): void {
  useEffect(() => {
    const id = gui.addRawInputListener(source, handler, opts);
    return () => {
      gui.removeListener(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source, handler, opts?.global, opts?.nodeId]);
}
