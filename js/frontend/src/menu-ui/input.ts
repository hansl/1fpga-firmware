// JS-side input listener registry + batch dispatcher.
//
// The runtime previously called Rust→Boa once per evdev event, paying
// Boa's per-call overhead 6-18 times per drain under heavy mash. We
// now register a single dispatcher with the host; the runtime calls
// it once per drain with all events as one JS array, and this module
// fans out to registered listeners on the JS side (cheap JS→JS calls).
//
// The public API (`addIntent`, `addRaw`, `remove`) is a drop-in
// replacement for `gui.addIntentListener` / `gui.addRawInputListener`
// / `gui.removeListener` — useIntent / useRawInput route through here
// now so React components don't need to know about the indirection.

import * as gui from '1fpga:gui';

type IntentHandler = (e: gui.IntentEvent) => void;
type RawHandler = (e: gui.RawInputEvent) => void;
type Source = gui.RawInputEvent['source'];

interface IntentEntry {
  id: number;
  handler: IntentHandler;
}
interface RawEntry {
  id: number;
  source: Source;
  handler: RawHandler;
}

const intentByName: Map<string, IntentEntry[]> = new Map();
const rawBySource: Map<Source, RawEntry[]> = new Map();
let nextId = 1;

export function addIntent(name: string, handler: IntentHandler): number {
  const id = nextId++;
  const list = intentByName.get(name);
  if (list) list.push({ id, handler });
  else intentByName.set(name, [{ id, handler }]);
  return id;
}

export function addRaw(source: Source, handler: RawHandler): number {
  const id = nextId++;
  const list = rawBySource.get(source);
  if (list) list.push({ id, source, handler });
  else rawBySource.set(source, [{ id, source, handler }]);
  return id;
}

export function remove(id: number): boolean {
  for (const list of intentByName.values()) {
    const i = list.findIndex((l) => l.id === id);
    if (i >= 0) {
      list.splice(i, 1);
      return true;
    }
  }
  for (const list of rawBySource.values()) {
    const i = list.findIndex((l) => l.id === id);
    if (i >= 0) {
      list.splice(i, 1);
      return true;
    }
  }
  return false;
}

/** Snapshot a listener list before iterating so handlers that
 *  subscribe / unsubscribe during dispatch don't break iteration. */
function snapshot<T>(list: T[] | undefined): T[] {
  return list ? list.slice() : [];
}

/** Throttle interval per (intent name, kind). Held-key autorepeat
 *  arrives at ~30/sec on evdev; we cap it at ~15/sec which is fast
 *  enough to feel responsive and slow enough that a held-key sweep
 *  doesn't pin React to ~10 fps. Pressed/Released for human-paced
 *  taps are well under this threshold and pass through unchanged.
 *
 *  ADAPTIVE: 66 ms is the floor, but the live window follows what
 *  dispatch actually costs — React commits run synchronously inside
 *  the intent handlers, so dispatch can time itself. With a fixed
 *  window, one long reconcile (a carousel window recenter re-styles
 *  every card) lets queued repeats keep the loop saturated and the
 *  UI death-spirals into seconds-long freezes under a held key
 *  (measured: ui fps 117 → 7.8 while holding right). Feeding the
 *  measured cost back caps the intent rate at what the reconciler
 *  can actually sustain. */
const COALESCE_MS_MIN = 66;
const COALESCE_MS_MAX = 250;
let coalesceMs = COALESCE_MS_MIN;
const lastIntentAt: Map<string, number> = new Map();

function dispatch(events: gui.InputBatchEntry[]): void {
  // Coalesce all events first so we know how many distinct intents
  // we'll actually fire. Then dispatch the deduped set.
  //
  // Why coalesce: each setState that produces a *different* state
  // triggers a React commit, and each commit costs ~5-10 ms in Boa's
  // interpreter. Held-key autorepeat at 30/sec → ~30 commits/sec →
  // ~150-300 ms of commit work per second, stealing time from
  // everything else. After coalescing the same key down to ~15/sec
  // we halve that floor.
  const now = Date.now();
  const intentsToFire: gui.IntentEvent[] = [];
  // Raw events stay as-is — there's no equivalent semantic
  // collapsing for "key 0xFF pressed" repeated 30 times.
  for (const ev of events) {
    const rawList = snapshot(rawBySource.get(ev.raw.source));
    for (const l of rawList) {
      try {
        l.handler(ev.raw);
      } catch (err) {
        console.warn(`raw handler ${l.id} threw:`, err);
      }
    }
    for (const intent of ev.intents) {
      // Released always fires (so press → release pairs stay
      // semantically intact; otherwise a held key could "stick"
      // visually). Pressed / Repeat are throttled by (name, kind).
      if (intent.kind === 'released') {
        intentsToFire.push(intent);
        lastIntentAt.delete(`${intent.name}:pressed`);
        lastIntentAt.delete(`${intent.name}:repeat`);
        continue;
      }
      const key = `${intent.name}:${intent.kind}`;
      const last = lastIntentAt.get(key);
      if (last !== undefined && now - last < coalesceMs) {
        // Skip — same (name, kind) fired too recently.
        continue;
      }
      lastIntentAt.set(key, now);
      intentsToFire.push(intent);
    }
  }
  for (const intent of intentsToFire) {
    const list = snapshot(intentByName.get(intent.name));
    for (const l of list) {
      try {
        l.handler(intent);
      } catch (err) {
        console.warn(`intent handler '${intent.name}' threw:`, err);
      }
    }
  }
  // Adaptive backpressure: fold this dispatch's real cost (handlers +
  // synchronous React commits) into the throttle window. Fast frames
  // decay it back toward the floor.
  if (intentsToFire.length > 0) {
    const cost = Date.now() - now;
    const target = Math.max(COALESCE_MS_MIN, Math.min(COALESCE_MS_MAX, cost * 2));
    // One-pole smoothing: react quickly to slowdowns, relax gradually.
    coalesceMs = target > coalesceMs ? target : coalesceMs * 0.8 + target * 0.2;
  }
}

// Register on module import. The dispatcher persists for the lifetime
// of the bundle; closures inside it capture the `intentByName` /
// `rawBySource` maps, so future adds / removes are visible without
// re-registering.
gui.setInputDispatcher(dispatch);
