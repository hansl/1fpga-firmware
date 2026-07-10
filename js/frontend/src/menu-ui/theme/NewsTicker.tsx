// Top-left news / notification line. One line of text next to a
// campaign glyph — content comes from GlobalStorage ('ui.news') so
// anything (a script, the future notification service) can post to
// it; the default greets a fresh install.

import { memo } from 'react';
import type { CSSProperties } from 'react';

import { s, vw } from '../scale';
import { Icon } from '../components/Icon';

const rootStyle: CSSProperties = {
  position: 'absolute',
  top: s(18),
  left: s(32),
  height: s(56),
  maxWidth: vw(0.45),
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: s(12),
  overflow: 'hidden',
};

const textStyle: CSSProperties = {
  fontSize: s(22),
  color: '#c8d4e0',
};

export const NewsTicker = memo(function NewsTicker({ text }: { text: string }) {
  return (
    <div style={rootStyle}>
      <Icon name="campaign" size={s(26)} color="#ffd060" />
      <div style={textStyle}>{text}</div>
    </div>
  );
});
