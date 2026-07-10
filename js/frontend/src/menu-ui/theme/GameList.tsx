// Virtualized entry list for a Collection screen. Renders only the
// visible window (plus nothing — rows are cheap to remount) so a
// 3000-file games directory costs the same as a 12-row one.

import { memo } from 'react';
import type { CSSProperties } from 'react';

import { s, vw } from '../scale';
import { Icon } from '../components/Icon';
import type { LibEntry } from '../services/library';

export const ROW_H = s(56);
export const VISIBLE_ROWS = 11;

const LIST_X = s(96);
const LIST_W = vw(0.62);

const rowBaseStyle: CSSProperties = {
  position: 'absolute',
  left: 0,
  width: LIST_W,
  height: ROW_H,
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: s(16),
  paddingLeft: s(20),
};

const accentStyle: CSSProperties = {
  position: 'absolute',
  left: 0,
  top: s(6),
  width: s(5),
  height: ROW_H - s(12),
  backgroundColor: '#7fc4ff',
};

const Row = memo(function Row({
  entry,
  y,
  selected,
}: {
  entry: LibEntry;
  y: number;
  selected: boolean;
}) {
  return (
    <div
      style={{
        ...rowBaseStyle,
        top: y,
        backgroundColor: selected ? '#ffffff22' : '#00000000',
      }}
    >
      {selected ? <div style={accentStyle} /> : null}
      <Icon
        name={entry.dir ? 'folder' : 'description'}
        size={s(24)}
        color={entry.dir ? '#ffd060' : '#6f8296'}
      />
      <div style={{ fontSize: s(26), color: selected ? '#ffffff' : '#b9c6d2' }}>
        {entry.name}
      </div>
    </div>
  );
});

export const GameList = memo(function GameList({
  entries,
  cursor,
  top,
}: {
  entries: LibEntry[];
  cursor: number;
  /** Absolute y of the first row. */
  top: number;
}) {
  const start = Math.max(
    0,
    Math.min(cursor - Math.floor(VISIBLE_ROWS / 2), entries.length - VISIBLE_ROWS),
  );
  const end = Math.min(entries.length, start + VISIBLE_ROWS);
  const rows = [];
  for (let i = start; i < end; i++) {
    rows.push(
      <Row key={i - start} entry={entries[i]} y={(i - start) * ROW_H} selected={i === cursor} />,
    );
  }
  return (
    <div
      style={{
        position: 'absolute',
        left: LIST_X,
        top,
        width: LIST_W,
        height: VISIBLE_ROWS * ROW_H,
        overflow: 'hidden',
      }}
    >
      {rows}
      {entries.length > 0 ? (
        <div
          style={{
            position: 'absolute',
            right: s(16),
            top: VISIBLE_ROWS * ROW_H - s(34),
            fontSize: s(20),
            color: '#7e8ea0',
          }}
        >
          {`${cursor + 1} / ${entries.length}`}
        </div>
      ) : (
        <div style={{ position: 'absolute', left: s(20), top: s(8), fontSize: s(26), color: '#7e8ea0' }}>
          Empty
        </div>
      )}
    </div>
  );
});
