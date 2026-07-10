// Home: news ticker top-left, status cluster top-right, system card
// carousel centre — the themed shell, running on real library data
// (schema + local scan via ensureBooted, systems from the catalog
// tables).
//
// Zone layout: STATUS (cluster items, left/right + confirm routes)
// above CAROUSEL (cards, left/right + confirm opens the collection);
// crossings are explicit moveFocus calls at cursor edges.

import { useCallback, useState } from 'react';
import type { CSSProperties } from 'react';

import { VW, VH, s } from '../scale';
import { ActionBar, type ActionBinding } from '../components/ActionBar';
import { FocusZone, useFocus, useZoneFocused, useZoneIntent } from '../focus';
import { useRouter } from '../router';
import { globalGet } from '../services/db';
import { listSystems, type SystemCard } from '../services/library';
import { useAsync } from '../services/useAsync';
import { CardCarousel } from '../theme/CardCarousel';
import { NewsTicker } from '../theme/NewsTicker';
import { STATUS_ITEMS, StatusCluster } from '../theme/StatusCluster';

const root: CSSProperties = {
  position: 'relative',
  width: VW,
  height: VH,
  // Transparent: the wallpaper is the hardware base layer.
};

const centerMsg: CSSProperties = {
  position: 'absolute',
  left: 0,
  right: 0,
  top: Math.round(VH * 0.45),
  textAlign: 'center',
  fontSize: s(28),
  color: '#9fb4c8',
};

const CAROUSEL_ACTIONS: ActionBinding[] = [
  { intent: 'navigate_leftright', label: 'Browse' },
  { intent: 'navigate_up', label: 'Status' },
  { intent: 'confirm', label: 'Open' },
];

const DEFAULT_NEWS = 'Welcome to 1FPGA';

export function HomeScreen() {
  const systems = useAsync(() => listSystems(), []);
  const news = useAsync(() => globalGet<string>('ui.news'), []);

  return (
    <div style={root}>
      <NewsTicker text={typeof news.value === 'string' ? news.value : DEFAULT_NEWS} />
      {/* Carousel zone FIRST: zone registration order doubles as the
          "focus falls back here" order when returning to this screen
          (see FocusProvider._register). */}
      <FocusZone id="carousel" neighbors={{ up: 'status' }}>
        {systems.loading ? (
          <div style={centerMsg}>Scanning library…</div>
        ) : systems.error ? (
          <div style={centerMsg}>{`Library unavailable: ${systems.error}`}</div>
        ) : (
          <CarouselZone systems={systems.value ?? []} />
        )}
      </FocusZone>
      <FocusZone id="status" neighbors={{ down: 'carousel' }}>
        <StatusZone />
      </FocusZone>
    </div>
  );
}

/** Owns the status cluster's item cursor and routes its confirms. */
function StatusZone() {
  const focused = useZoneFocused();
  const focus = useFocus();
  const router = useRouter();
  const [sel, setSel] = useState(0);

  useZoneIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setSel((s) => Math.max(0, s - 1));
      }
    }, []),
  );
  useZoneIntent(
    'navigate_right',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setSel((s) => Math.min(STATUS_ITEMS.length - 1, s + 1));
      }
    }, []),
  );
  useZoneIntent(
    'confirm',
    useCallback(
      (e) => {
        if (e.kind === 'pressed') router.navigate(STATUS_ITEMS[sel].route);
      },
      [router, sel],
    ),
  );
  useZoneIntent(
    'navigate_down',
    useCallback(
      (e) => {
        if (e.kind === 'pressed') focus.moveFocus('down');
      },
      [focus],
    ),
  );
  useZoneIntent(
    'back',
    useCallback(
      (e) => {
        if (e.kind === 'pressed') focus.moveFocus('down');
      },
      [focus],
    ),
  );

  return <StatusCluster focused={focused} selected={sel} />;
}

/** Owns the card cursor; confirm opens the system's collection. */
function CarouselZone({ systems }: { systems: SystemCard[] }) {
  const focus = useFocus();
  const router = useRouter();
  const [sel, setSel] = useState(0);

  useZoneIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setSel((s) => Math.max(0, s - 1));
      }
    }, []),
  );
  useZoneIntent(
    'navigate_right',
    useCallback(
      (e) => {
        if (e.kind === 'pressed' || e.kind === 'repeat') {
          setSel((s) => Math.min(systems.length - 1, s + 1));
        }
      },
      [systems.length],
    ),
  );
  useZoneIntent(
    'navigate_up',
    useCallback(
      (e) => {
        if (e.kind === 'pressed') focus.moveFocus('up');
      },
      [focus],
    ),
  );
  useZoneIntent(
    'confirm',
    useCallback(
      (e) => {
        if (e.kind === 'pressed' && systems.length > 0) {
          const sys = systems[Math.min(sel, systems.length - 1)];
          router.navigate(`/collections/${encodeURIComponent(sys.uniqueName)}`);
        }
      },
      [router, systems, sel],
    ),
  );

  if (systems.length === 0) {
    return <div style={centerMsg}>No systems found on this card.</div>;
  }
  const clamped = Math.min(sel, systems.length - 1);
  return (
    <>
      <CardCarousel systems={systems} selected={clamped} />
      <ActionBar actions={CAROUSEL_ACTIONS} />
    </>
  );
}
