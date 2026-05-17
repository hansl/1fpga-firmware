// Vertical list of items for the currently-selected category.
// Selected item brightens + scales 1.1×; others sit at rest size
// and dim down. Icons are optional per-item; when present they sit
// to the left of the label.

import { memo, useRef } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import type { MenuItem } from '../data';

const ROW_HEIGHT = 80;
const ICON_SIZE = 56;

const rootStyle: CSSProperties = {
  position: 'absolute',
  top: 320,
  left: 96,
  right: 96,
  display: 'flex',
  flexDirection: 'column',
  gap: 4,
};

const rowStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  height: ROW_HEIGHT,
  paddingLeft: 24,
  paddingRight: 24,
  // Items use absolute scale animation via useTween; the rest
  // layout stays put.
};

const iconStyle: CSSProperties = {
  width: ICON_SIZE,
  height: ICON_SIZE,
  marginRight: 24,
};

const textColStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'flex-start',
  justifyContent: 'center',
};

const nameStyle: CSSProperties = {
  fontSize: 28,
  color: '#d0d8e0',
};

const nameSelectedStyle: CSSProperties = {
  ...nameStyle,
  color: '#ffffff',
};

const subtitleStyle: CSSProperties = {
  fontSize: 18,
  color: '#80909a',
  marginTop: 4,
};

const Row = memo(function Row({
  item,
  selected,
}: {
  item: MenuItem;
  selected: boolean;
}) {
  // Same animation shape as MenuBar but a touch milder; vertical
  // lists feel claustrophobic when individual rows grow 20%.
  const ref = useRef<gui.NodeId | null>(null);
  useTween(
    ref,
    { scale: selected ? 1.08 : 1.0, opacity: selected ? 1.0 : 0.55 },
    { duration: 180, easing: 'easeOut' },
  );
  return (
    <div
      ref={ref}
      style={{
        ...rowStyle,
        scale: selected ? 1.08 : 1.0,
        opacity: selected ? 1.0 : 0.55,
      }}
    >
      {item.icon ? <img src={item.icon} style={iconStyle} /> : null}
      <div style={textColStyle}>
        <div style={selected ? nameSelectedStyle : nameStyle}>{item.name}</div>
        {item.subtitle ? <div style={subtitleStyle}>{item.subtitle}</div> : null}
      </div>
    </div>
  );
});

export function MenuColumn({
  items,
  selected,
}: {
  items: MenuItem[];
  selected: number;
}) {
  return (
    <div style={rootStyle}>
      {items.map((it, i) => (
        <Row key={it.name} item={it} selected={i === selected} />
      ))}
    </div>
  );
}
