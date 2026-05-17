// XMB-style category strip: the SELECTED category sits at the
// screen's horizontal centre, and the entire strip slides left/right
// so neighbouring categories scroll past a fixed cursor.
//
// Visual conventions taken from PS3/PSP XMB:
//   - Strip is vertically centred (around y = 460).
//   - Selected category is bigger (scale 1.08, not a giant 1.2)
//     and brighter, but does NOT change colour — the icon's natural
//     tint reads against the wallpaper.
//   - Inactive categories shrink slightly and dim (opacity 0.5).
//   - Only the SELECTED category shows a textual label; the rest
//     are icon-only. The category name also appears as a header
//     above the strip.

import { memo, useRef } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import type { MenuCategory } from '../data';

const FB_WIDTH = 1920;
/** Centre of the screen — where the selected category icon lives. */
const CENTER_X = FB_WIDTH / 2;
/** Horizontal distance between consecutive category icons. */
const SLOT_WIDTH = 220;
/** Strip vertical centre. */
const STRIP_CENTER_Y = 460;
const STRIP_HEIGHT = 160;
const STRIP_TOP = STRIP_CENTER_Y - STRIP_HEIGHT / 2;
const ICON_SIZE = 96;

const stripFrameStyle: CSSProperties = {
  position: 'absolute',
  top: STRIP_TOP,
  left: 0,
  right: 0,
  height: STRIP_HEIGHT,
  overflow: 'hidden',
};

const stripInnerStyle: CSSProperties = {
  position: 'absolute',
  top: 0,
  height: STRIP_HEIGHT,
  display: 'flex',
  flexDirection: 'row',
};

const slotStyle: CSSProperties = {
  width: SLOT_WIDTH,
  height: STRIP_HEIGHT,
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  justifyContent: 'center',
};

const iconStyle: CSSProperties = {
  width: ICON_SIZE,
  height: ICON_SIZE,
};

const titleStyle: CSSProperties = {
  position: 'absolute',
  top: STRIP_TOP - 56,
  left: 0,
  right: 0,
  textAlign: 'center',
  fontSize: 36,
  color: '#ffffff',
};

const CategoryIcon = memo(function CategoryIcon({
  category,
  selected,
}: {
  category: MenuCategory;
  selected: boolean;
}) {
  // Selected → 1.08× scale, full opacity. Unselected → 1.0×, dimmed.
  // No colour tint change — XMB convention is brightness + scale.
  const ref = useRef<gui.NodeId | null>(null);
  useTween(
    ref,
    { scale: selected ? 1.08 : 1.0, opacity: selected ? 1.0 : 0.5 },
    { duration: 220, easing: 'easeOut' },
  );
  return (
    <div style={slotStyle}>
      <img
        ref={ref}
        src={category.icon}
        style={{
          ...iconStyle,
          scale: selected ? 1.08 : 1.0,
          opacity: selected ? 1.0 : 0.5,
        }}
      />
    </div>
  );
});

export const MenuBar = memo(function MenuBar({
  categories,
  selected,
}: {
  categories: MenuCategory[];
  selected: number;
}) {
  // Position the strip so the SELECTED category's slot centres on
  // CENTER_X. Slot i sits at `inner_left + (i + 0.5) * SLOT_WIDTH`;
  // solve for inner_left given selected sits at CENTER_X.
  const targetLeft = CENTER_X - (selected + 0.5) * SLOT_WIDTH;
  const ref = useRef<gui.NodeId | null>(null);
  useTween(ref, { left: targetLeft }, { duration: 280, easing: 'easeOut' });

  return (
    <>
      <div style={titleStyle}>{categories[selected].name}</div>
      <div style={stripFrameStyle}>
        <div
          ref={ref}
          style={{
            ...stripInnerStyle,
            left: targetLeft,
            width: SLOT_WIDTH * categories.length,
          }}
        >
          {categories.map((c, i) => (
            <CategoryIcon key={c.name} category={c} selected={i === selected} />
          ))}
        </div>
      </div>
    </>
  );
});

/** Exported so other components (e.g. MenuColumn) can align with
 *  the selected category's column at screen centre. */
export const SELECTED_CATEGORY_X = CENTER_X;
export const STRIP_BOTTOM_Y = STRIP_TOP + STRIP_HEIGHT;
