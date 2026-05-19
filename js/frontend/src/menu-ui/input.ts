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

function dispatch(events: gui.InputBatchEntry[]): void {
  for (const ev of events) {
    const rawList = snapshot(rawBySource.get(ev.raw.source));
    for (const l of rawList) {
      try {
        l.handler(ev.raw);
      } catch (err) {
        // Don't let one handler's throw skip the rest of the batch.
        console.warn(`raw handler ${l.id} threw:`, err);
      }
    }
    for (const intent of ev.intents) {
      const list = snapshot(intentByName.get(intent.name));
      for (const l of list) {
        try {
          l.handler(intent);
        } catch (err) {
          console.warn(`intent handler '${intent.name}' threw:`, err);
        }
      }
    }
  }
}

// Register on module import. The dispatcher persists for the lifetime
// of the bundle; closures inside it capture the `intentByName` /
// `rawBySource` maps, so future adds / removes are visible without
// re-registering.
gui.setInputDispatcher(dispatch);
