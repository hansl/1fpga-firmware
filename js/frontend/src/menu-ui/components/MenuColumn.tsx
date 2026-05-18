// XMB-style item list: hangs straight down from the selected
// category's column. The leftmost edge of each row aligns with the
// selected-category x position; items extend rightward from there.
// Selected row is brighter and very slightly larger; others sit at
// rest size and dimmed.
//
// We previously cross-faded the entire column when items changed,
// using a setTimeout to swap a `shown` snapshot mid-fade. Under
// rapid input the setTimeout was repeatedly cancelled, leaving
// `shown` stuck on stale items while the title (rendered from
// `cat` directly) raced ahead — visually the column displayed the
// wrong category's items. The reconciler's clobber of style on
// every commit also fought the opacity tween. We now render the
// current items synchronously; the brightness / scale transition
// on the *selected* row already provides enough motion that the
// loss of the column-wide cross-fade isn't visually jarring.

import { memo, useRef } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import type { MenuItem } from '../data';
import { SELECTED_CATEGORY_X, STRIP_BOTTOM_Y } from './MenuBar';

const ROW_HEIGHT = 64;
const ICON_SIZE = 40;
/** First row begins this far below the strip's bottom edge. */
const COLUMN_TOP_OFFSET = 32;
/** Distance from the selected-category x to where the row content
 *  starts. Negative-leaning so the icon sits slightly left of
 *  centre and the text extends rightward, matching how the XMB
 *  positions items "anchored" to the column. */
const COLUMN_LEFT_OFFSET = -40;

const rootStyle: CSSProperties = {
  position: 'absolute',
  top: STRIP_BOTTOM_Y + COLUMN_TOP_OFFSET,
  left: SELECTED_CATEGORY_X + COLUMN_LEFT_OFFSET,
  right: 0,
  display: 'flex',
  flexDirection: 'column',
};

const rowStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  height: ROW_HEIGHT,
  paddingRight: 24,
};

const iconStyle: CSSProperties = {
  width: ICON_SIZE,
  height: ICON_SIZE,
  marginRight: 16,
};

const textColStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'flex-start',
  justifyContent: 'center',
};

const nameStyle: CSSProperties = {
  fontSize: 26,
  color: '#d0d8e0',
};

const nameSelectedStyle: CSSProperties = {
  ...nameStyle,
  color: '#ffffff',
};

const subtitleStyle: CSSProperties = {
  fontSize: 16,
  color: '#80909a',
  marginTop: 2,
};

const Row = memo(function Row({
  item,
  selected,
}: {
  item: MenuItem;
  selected: boolean;
}) {
  const ref = useRef<gui.NodeId | null>(null);
  useTween(
    ref,
    { scale: selected ? 1.04 : 1.0, opacity: selected ? 1.0 : 0.5 },
    { duration: 180, easing: 'easeOut' },
  );
  return (
    <div
      ref={ref}
      style={{
        ...rowStyle,
        scale: selected ? 1.04 : 1.0,
        opacity: selected ? 1.0 : 0.5,
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
