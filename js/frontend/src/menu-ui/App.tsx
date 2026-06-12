// XMB-style menu shell. Top: horizontal strip of category icons
// (the "X"). Centre: vertical column of items for the selected
// category (the "MB"). Bottom: action hints that reflect the
// currently active input device. Top-right: status strip with
// network / user / clock.

import { useCallback, useState } from 'react';
import type { CSSProperties } from 'react';

import { useIntent } from './hooks';
import { CATEGORIES } from './data';
import { VW, VH } from './scale';
import { ActionBar } from './components/ActionBar';
import { MenuBar } from './components/MenuBar';
import { MenuColumn } from './components/MenuColumn';
import { StatusBar } from './components/StatusBar';

const root: CSSProperties = {
  position: 'relative',
  width: VW,
  height: VH,
  // No background colour: the wallpaper is a hardware scanout-compositor
  // layer (Phase C), and this full-screen root div must stay transparent so
  // the compositor's wallpaper shows through. An opaque bg here would paint
  // over the transparent content clear and hide the wallpaper entirely.
  // (When compositing is off, the Rust paint path clears to black.)
};

export function App() {
  // Two-axis cursor: `cat` selects a column among the top icons;
  // `item` selects a row within that column. Switching categories
  // resets the row cursor to 0, matching the XMB convention.
  const [cat, setCat] = useState(0);
  const [item, setItem] = useState(0);

  useIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setCat((c) => {
          const next = Math.max(0, c - 1);
          if (next !== c) setItem(0);
          return next;
        });
      }
    }, []),
  );
  useIntent(
    'navigate_right',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setCat((c) => {
          const next = Math.min(CATEGORIES.length - 1, c + 1);
          if (next !== c) setItem(0);
          return next;
        });
      }
    }, []),
  );
  useIntent(
    'navigate_up',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setItem((i) => Math.max(0, i - 1));
      }
    }, []),
  );
  useIntent(
    'navigate_down',
    // `cat` is in the dep array: the clamp reads the *current* category's
    // item count, so the handler must refresh when the category changes.
    // (With `[]` it was a stale closure pinned to category 0's length —
    // over/under-scrolling the row cursor in every other category.)
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setItem((i) => {
          const len = CATEGORIES[cat].items.length;
          return Math.min(len - 1, i + 1);
        });
      }
    }, [cat]),
  );

  const category = CATEGORIES[cat];
  const items = category.items;
  const currentItem = items[Math.min(item, items.length - 1)];
  const actions = currentItem?.actions ?? category.defaultActions;

  return (
    <div style={root}>
      <StatusBar user="hansl" notifications={2} wifi="connected" />
      <MenuBar categories={CATEGORIES} selected={cat} />
      <MenuColumn items={items} selected={item} />
      <ActionBar actions={actions} />
    </div>
  );
}
