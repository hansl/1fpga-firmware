// Home screen: the carousel-first shell.
//
// The old App god-component's cursor state is decomposed here: the
// CAROUSEL zone owns its own category/item cursors, the STATUS zone
// owns its own highlight, and crossings are explicit `moveFocus`
// calls at cursor edges (see focus/index.tsx). Selecting a category
// pushes a route — screens never reach into each other's state.
//
// Still rendering the demo CATEGORIES data and XMB-era components;
// both get replaced as the new theme's screens and the 1fpga:db-
// backed catalog queries land. The architecture (zones + routes) is
// what this file establishes.

import { useCallback, useState } from 'react';
import type { CSSProperties } from 'react';

import { CATEGORIES } from '../data';
import { VW, VH } from '../scale';
import { ActionBar } from '../components/ActionBar';
import { MenuBar } from '../components/MenuBar';
import { StatusBar } from '../components/StatusBar';
import { FocusZone, useFocus, useZoneFocused, useZoneIntent } from '../focus';
import { useRouter } from '../router';

const root: CSSProperties = {
  position: 'relative',
  width: VW,
  height: VH,
  // Transparent: the wallpaper is the hardware base layer.
};

export function HomeScreen() {
  return (
    <div style={root}>
      <FocusZone id="status" neighbors={{ down: 'carousel' }}>
        <StatusZone />
      </FocusZone>
      <FocusZone id="carousel" neighbors={{ up: 'status' }}>
        <CarouselZone />
      </FocusZone>
    </div>
  );
}

/**
 * Top status strip. Focusable so UP from the carousel reaches the
 * settings/wifi/bluetooth/account/notification cluster; per-item
 * focus visuals arrive with the themed StatusCluster component —
 * for now the whole strip brightens while focused and left/right
 * move an (invisible) highlight index so the interaction contract
 * is already exercised.
 */
function StatusZone() {
  const focused = useZoneFocused();
  const focus = useFocus();
  const [, setSel] = useState(0);

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
        setSel((s) => Math.min(4, s + 1));
      }
    }, []),
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

  return (
    <div style={{ opacity: focused ? 1 : 0.75 }}>
      <StatusBar user="hansl" notifications={2} wifi="connected" />
    </div>
  );
}

/** The system/category carousel. Owns its own cursor. */
function CarouselZone() {
  const focus = useFocus();
  const router = useRouter();
  const [cat, setCat] = useState(0);

  useZoneIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setCat((c) => Math.max(0, c - 1));
      }
    }, []),
  );
  useZoneIntent(
    'navigate_right',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setCat((c) => Math.min(CATEGORIES.length - 1, c + 1));
      }
    }, []),
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
        if (e.kind === 'pressed') {
          // Route by index until categories come from the catalog DB
          // with stable ids.
          router.navigate(`/collections/${cat}`);
        }
      },
      [router, cat],
    ),
  );

  const category = CATEGORIES[cat];
  return (
    <>
      <MenuBar categories={CATEGORIES} selected={cat} />
      <ActionBar actions={category.defaultActions} />
    </>
  );
}
