// Top horizontal strip of category icons (the "X" of XMB). The
// selected icon scales up 1.2× and brightens; the others sit at
// rest size and dimmed.

import { memo, useRef } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import type { MenuCategory } from '../data';

const ICON_SIZE = 96;
const ICON_GAP = 64;

const rootStyle: CSSProperties = {
  position: 'absolute',
  top: 80,
  left: 0,
  right: 0,
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  justifyContent: 'center',
  gap: ICON_GAP,
};

const slotStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  // Reserved height so the scaled-up icon doesn't push neighbours
  // around; the label sits below with consistent spacing across
  // selection states.
  width: 160,
  height: 160,
  justifyContent: 'center',
};

const iconStyle: CSSProperties = {
  width: ICON_SIZE,
  height: ICON_SIZE,
};

const labelStyle: CSSProperties = {
  marginTop: 12,
  fontSize: 24,
  color: '#d0d8e0',
};

const labelSelectedStyle: CSSProperties = {
  ...labelStyle,
  color: '#ffffff',
};

const CategoryIcon = memo(function CategoryIcon({
  category,
  selected,
}: {
  category: MenuCategory;
  selected: boolean;
}) {
  // Selected → 1.2× scale, full opacity. Unselected → unit scale,
  // 0.6 opacity. Host-side tween, so React only commits when
  // `selected` actually changes.
  const ref = useRef<gui.NodeId | null>(null);
  useTween(
    ref,
    { scale: selected ? 1.2 : 1.0, opacity: selected ? 1.0 : 0.6 },
    { duration: 200, easing: 'easeOut' },
  );
  return (
    <div style={slotStyle}>
      <img
        ref={ref}
        src={category.icon}
        style={{
          ...iconStyle,
          // React commits the target so layout is stable across
          // tween ticks; the tween manager overrides the values
          // each frame.
          scale: selected ? 1.2 : 1.0,
          opacity: selected ? 1.0 : 0.6,
        }}
      />
      <div style={selected ? labelSelectedStyle : labelStyle}>{category.name}</div>
    </div>
  );
});

export function MenuBar({
  categories,
  selected,
}: {
  categories: MenuCategory[];
  selected: number;
}) {
  return (
    <div style={rootStyle}>
      {categories.map((c, i) => (
        <CategoryIcon key={c.name} category={c} selected={i === selected} />
      ))}
    </div>
  );
}
