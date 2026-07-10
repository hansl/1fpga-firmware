// Zone-based focus management — the browser-like model agreed for the
// frontend rebuild.
//
// One <FocusProvider> registers the shared navigation intents ONCE
// and routes them to the ACTIVE zone's handlers. Screens declare
// <FocusZone id="carousel" .../> regions; components inside a zone
// subscribe with `useZoneIntent` (fires only while their zone is
// active) and read `useZoneFocused()` for focus visuals.
//
// Zone-to-zone movement is EXPLICIT, not spatial magic: a zone's own
// handler decides when its internal cursor hits an edge and calls
// `moveFocus(dir)`, which follows the zone's declared `neighbors`
// map. This keeps each zone self-contained (it owns its cursor
// state) while the crossings stay declarative — and there is no
// central "App god component" holding everyone's state.

import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { ReactNode } from 'react';
import type * as gui from '1fpga:gui';

import { useIntent } from '../hooks';

export type Direction = 'up' | 'down' | 'left' | 'right';
export type ZoneId = string;

export interface ZoneConfig {
  /** Neighbouring zone per direction, used by `moveFocus`. */
  neighbors?: Partial<Record<Direction, ZoneId>>;
}

interface ZoneRecord extends ZoneConfig {
  /** Intent-name → handlers, live only while the zone is mounted. */
  listeners: Map<string, Set<(e: gui.IntentEvent) => void>>;
}

interface FocusApi {
  /** The currently active zone id (null before the first zone mounts). */
  active: ZoneId | null;
  /** Programmatically focus a zone (route changes, modals). */
  focusZone(id: ZoneId): void;
  /** Follow the active zone's `neighbors` map. No-op without one. */
  moveFocus(dir: Direction): void;
  /** @internal zone registry — used by <FocusZone>. */
  _register(id: ZoneId, record: ZoneRecord): () => void;
  _zone(id: ZoneId): ZoneRecord | undefined;
}

const FocusContext = createContext<FocusApi | null>(null);
const ZoneContext = createContext<ZoneId | null>(null);

const NAV_INTENTS: Record<string, Direction> = {
  navigate_up: 'up',
  navigate_down: 'down',
  navigate_left: 'left',
  navigate_right: 'right',
};

/** Intents forwarded to the active zone beyond the four directions. */
const ACTION_INTENTS = ['confirm', 'back', 'menu', 'detail'];

export function FocusProvider({
  initial,
  children,
}: {
  /** Zone to activate once it mounts. */
  initial?: ZoneId;
  children: ReactNode;
}) {
  const zones = useRef(new Map<ZoneId, ZoneRecord>());
  const [active, setActive] = useState<ZoneId | null>(null);
  const wanted = useRef<ZoneId | null>(initial ?? null);

  const focusZone = useCallback((id: ZoneId) => {
    if (zones.current.has(id)) {
      wanted.current = null;
      setActive(id);
    } else {
      // Zone not mounted yet (e.g. focusing a route's zone before its
      // screen commits) — activate on registration.
      wanted.current = id;
    }
  }, []);

  const api = useMemo<FocusApi>(
    () => ({
      active,
      focusZone,
      moveFocus(dir) {
        if (active == null) return;
        const next = zones.current.get(active)?.neighbors?.[dir];
        if (next && zones.current.has(next)) {
          setActive(next);
        }
      },
      _register(id, record) {
        zones.current.set(id, record);
        if (wanted.current === id || active == null) {
          wanted.current = null;
          setActive(id);
        }
        return () => {
          zones.current.delete(id);
          // Focus falls back to any remaining zone so intents keep
          // routing after a screen unmounts.
          setActive((cur) => {
            if (cur !== id) return cur;
            const first = zones.current.keys().next();
            return first.done ? null : first.value;
          });
        };
      },
      _zone(id) {
        return zones.current.get(id);
      },
    }),
    [active, focusZone],
  );

  // The ONLY intent subscriptions for navigation in the whole app.
  const dispatch = useCallback(
    (name: string, e: gui.IntentEvent) => {
      if (active == null) return;
      const handlers = zones.current.get(active)?.listeners.get(name);
      if (!handlers) return;
      for (const h of [...handlers]) h(e);
    },
    [active],
  );
  useIntent('navigate_up', useCallback((e) => dispatch('navigate_up', e), [dispatch]));
  useIntent('navigate_down', useCallback((e) => dispatch('navigate_down', e), [dispatch]));
  useIntent('navigate_left', useCallback((e) => dispatch('navigate_left', e), [dispatch]));
  useIntent('navigate_right', useCallback((e) => dispatch('navigate_right', e), [dispatch]));
  useIntent('confirm', useCallback((e) => dispatch('confirm', e), [dispatch]));
  useIntent('back', useCallback((e) => dispatch('back', e), [dispatch]));

  return <FocusContext.Provider value={api}>{children}</FocusContext.Provider>;
}

export function FocusZone({
  id,
  neighbors,
  children,
}: ZoneConfig & { id: ZoneId; children: ReactNode }) {
  const api = useFocus();
  // The record is stable across renders; `neighbors` refreshes on it.
  const record = useRef<ZoneRecord>({ listeners: new Map() });
  record.current.neighbors = neighbors;
  useLayoutEffect(() => api._register(id, record.current), [api, id]);
  return <ZoneContext.Provider value={id}>{children}</ZoneContext.Provider>;
}

export function useFocus(): FocusApi {
  const api = useContext(FocusContext);
  if (!api) throw new Error('useFocus outside <FocusProvider>');
  return api;
}

/** True while the surrounding <FocusZone> is the active zone. */
export function useZoneFocused(): boolean {
  const api = useFocus();
  const zone = useContext(ZoneContext);
  return zone != null && api.active === zone;
}

/**
 * Subscribe to an intent, delivered only while the surrounding zone
 * is active. Valid names: navigate_up/down/left/right, confirm, back.
 */
export function useZoneIntent(
  name: string,
  handler: (e: gui.IntentEvent) => void,
): void {
  const api = useFocus();
  const zone = useContext(ZoneContext);
  useLayoutEffect(() => {
    if (zone == null) return;
    const rec = api._zone(zone);
    if (!rec) return;
    let set = rec.listeners.get(name);
    if (!set) {
      set = new Set();
      rec.listeners.set(name, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  }, [api, zone, name, handler]);
}

export { NAV_INTENTS, ACTION_INTENTS };
