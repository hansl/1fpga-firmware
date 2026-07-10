// Collection screen: the item list for a selected category.
//
// Reached by route ('/collections/:id') — which also makes it a
// deep-link target (set ui.startupRoute to boot straight here). Owns
// its own row cursor in the LIST zone; `back` pops the route.
//
// Demo data + old MenuColumn for now; the themed card-docked layout
// and the 1fpga:db catalog query replace those here, and the list
// gets virtualized when real collections (1000s of games) arrive.

import { useCallback, useState } from 'react';
import type { CSSProperties } from 'react';

import { CATEGORIES } from '../data';
import { VW, VH } from '../scale';
import { ActionBar } from '../components/ActionBar';
import { MenuColumn } from '../components/MenuColumn';
import { StatusBar } from '../components/StatusBar';
import { FocusZone, useZoneIntent } from '../focus';
import { useRouter } from '../router';

const root: CSSProperties = {
  position: 'relative',
  width: VW,
  height: VH,
};

export function CollectionScreen({ id }: { id: string }) {
  const index = Number.parseInt(id, 10);
  const category = CATEGORIES[Number.isNaN(index) ? 0 : index] ?? CATEGORIES[0];

  return (
    <div style={root}>
      <StatusBar user="hansl" notifications={2} wifi="connected" />
      <FocusZone id="list">
        <ListZone categoryIndex={CATEGORIES.indexOf(category)} />
      </FocusZone>
    </div>
  );
}

function ListZone({ categoryIndex }: { categoryIndex: number }) {
  const router = useRouter();
  const category = CATEGORIES[categoryIndex];
  const [item, setItem] = useState(0);

  useZoneIntent(
    'navigate_up',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setItem((i) => Math.max(0, i - 1));
      }
    }, []),
  );
  useZoneIntent(
    'navigate_down',
    useCallback(
      (e) => {
        if (e.kind === 'pressed' || e.kind === 'repeat') {
          setItem((i) => Math.min(category.items.length - 1, i + 1));
        }
      },
      [category],
    ),
  );
  useZoneIntent(
    'back',
    useCallback(
      (e) => {
        if (e.kind === 'pressed') router.back();
      },
      [router],
    ),
  );

  const items = category.items;
  const current = items[Math.min(item, items.length - 1)];
  return (
    <>
      <MenuColumn items={items} selected={item} />
      <ActionBar actions={current?.actions ?? category.defaultActions} />
    </>
  );
}
