// Minimal subscribable value — the re-render scalpel.
//
// A component that subscribes via useMicroStore re-renders when THE
// VALUE changes and nothing else does. Used by hot paths that bypass
// React for their main effects (imperative tweens on host nodes) but
// still need a couple of text nodes kept in sync: the subscriber
// component is tiny, so the per-update reconcile cost is a few nodes
// instead of a subtree.
//
// In Boa's interpreter that difference is the whole game: a full
// carousel reconcile costs ~30-60 ms; re-rendering a one-text-node
// component costs well under a millisecond.

import { useSyncExternalStore } from 'react';

export interface MicroStore<T> {
  get(): T;
  set(v: T): void;
  subscribe(f: () => void): () => void;
}

export function createMicroStore<T>(initial: T): MicroStore<T> {
  let value = initial;
  const subs = new Set<() => void>();
  return {
    get: () => value,
    set(v: T) {
      if (Object.is(v, value)) return;
      value = v;
      for (const f of [...subs]) f();
    },
    subscribe(f: () => void) {
      subs.add(f);
      return () => {
        subs.delete(f);
      };
    },
  };
}

export function useMicroStore<T>(store: MicroStore<T>): T {
  return useSyncExternalStore(store.subscribe, store.get);
}
