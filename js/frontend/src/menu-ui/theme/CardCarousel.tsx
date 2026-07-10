// The Home carousel: a horizontal row of system cards sliding under
// a fixed centre cursor, PS5-style. The selected card scales up
// behind a white ring; unselected cards sit dimmed at rest.
//
// Virtualized: only the cards within WINDOW slots of the selection
// are mounted (the strip can hold 150+ systems). Cards are keyed by
// uniqueName so entering/leaving the window doesn't disturb the
// mounted ones, and the strip slides via translateX — paint-only, no
// Taffy reflow during the glide (same trick as the old MenuBar).

import { memo, useRef } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import { CENTRE_X, s } from '../scale';
import { Icon, type IconName } from '../components/Icon';
import type { SystemCard } from '../services/library';

const CARD_W = s(280);
const CARD_H = s(340);
const GAP = s(44);
const SLOT = CARD_W + GAP;
/** Scale headroom so the selected card's ring isn't clipped. */
const FRAME_PAD = s(28);
const FRAME_TOP = s(392);
/** Cards mounted on each side of the selection. */
const WINDOW = 5;

const ASSETS = '/media/fat/menu_ui_assets';

/** Card art heuristic against the demo asset set; everything else
 *  renders a glyph. Replaced wholesale when per-system art ships
 *  with the catalog port. */
function artFor(uniqueName: string): string | null {
  const n = uniqueName.toLowerCase();
  if (n === 'snes') return `${ASSETS}/snes.png`;
  if (n === 'nes') return `${ASSETS}/nes.png`;
  if (n.includes('genesis') || n.includes('megadrive')) return `${ASSETS}/genesis.png`;
  if (n.includes('gameboy') || n === 'gba' || n === 'gbc') return `${ASSETS}/gameboy.png`;
  if (n.includes('atari')) return `${ASSETS}/atari.png`;
  return null;
}

function glyphFor(tag: string): IconName {
  switch (tag) {
    case 'console': return 'sports_esports';
    case 'arcade': return 'videogame_asset';
    case 'computer': return 'keyboard';
    case 'utility': return 'memory';
    default: return 'gamepad';
  }
}

const titleStyle: CSSProperties = {
  position: 'absolute',
  top: FRAME_TOP - s(76),
  left: 0,
  right: 0,
  textAlign: 'center',
  fontSize: s(40),
  color: '#ffffff',
};

const counterStyle: CSSProperties = {
  position: 'absolute',
  top: FRAME_TOP - s(66),
  right: s(48),
  fontSize: s(22),
  color: '#7e8ea0',
};

const frameStyle: CSSProperties = {
  position: 'absolute',
  top: FRAME_TOP - FRAME_PAD,
  left: 0,
  right: 0,
  height: CARD_H + FRAME_PAD * 2,
  overflow: 'hidden',
};

const Card = memo(function Card({
  system,
  slot,
  selected,
}: {
  system: SystemCard;
  slot: number;
  selected: boolean;
}) {
  const ref = useRef<gui.NodeId | null>(null);
  useTween(
    ref,
    { scale: selected ? 1.1 : 1.0, opacity: selected ? 1.0 : 0.55 },
    { duration: 200, easing: 'easeOut' },
  );
  const art = artFor(system.uniqueName);
  return (
    <div
      ref={ref}
      style={{
        position: 'absolute',
        left: slot * SLOT + GAP / 2,
        top: FRAME_PAD,
        width: CARD_W,
        height: CARD_H,
        scale: selected ? 1.1 : 1.0,
        opacity: selected ? 1.0 : 0.55,
      }}
    >
      {/* Selection ring — kept mounted, visibility via colour. */}
      <div
        style={{
          position: 'absolute',
          left: -s(5),
          top: -s(5),
          width: CARD_W + s(10),
          height: CARD_H + s(10),
          backgroundColor: selected ? '#ffffffe0' : '#00000000',
        }}
      />
      <div
        style={{
          position: 'absolute',
          left: 0,
          top: 0,
          width: CARD_W,
          height: CARD_H,
          backgroundColor: '#131d29e6',
          display: 'flex',
          flexDirection: 'column',
          alignItems: 'center',
        }}
      >
        <div
          style={{
            width: CARD_W,
            height: CARD_H - s(96),
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          {art ? (
            <img src={art} style={{ width: s(150), height: s(150) }} />
          ) : (
            <Icon name={glyphFor(system.tag)} size={s(110)} color="#5f7a90" />
          )}
        </div>
        <div style={{ fontSize: s(26), color: '#e8f0f8' }}>{system.name}</div>
        <div style={{ fontSize: s(18), color: '#7e8ea0', marginTop: s(6) }}>
          {system.gamesPath ? 'games' : system.tag}
        </div>
      </div>
    </div>
  );
});

export const CardCarousel = memo(function CardCarousel({
  systems,
  selected,
}: {
  systems: SystemCard[];
  selected: number;
}) {
  const targetLeft = CENTRE_X - (selected + 0.5) * SLOT;
  const ref = useRef<gui.NodeId | null>(null);
  useTween(ref, { translateX: targetLeft }, { duration: 260, easing: 'easeOut' });

  const lo = Math.max(0, selected - WINDOW);
  const hi = Math.min(systems.length - 1, selected + WINDOW);
  const visible: Array<{ system: SystemCard; slot: number }> = [];
  for (let i = lo; i <= hi; i++) visible.push({ system: systems[i], slot: i });

  const current = systems[selected];
  return (
    <>
      <div style={titleStyle}>{current?.name ?? ''}</div>
      <div style={counterStyle}>{`${selected + 1} / ${systems.length}`}</div>
      <div style={frameStyle}>
        <div
          ref={ref}
          style={{
            position: 'absolute',
            left: 0,
            top: 0,
            height: CARD_H + FRAME_PAD * 2,
            width: SLOT * systems.length,
            translateX: targetLeft,
          }}
        >
          {visible.map(({ system, slot }) => (
            <Card
              key={system.uniqueName}
              system={system}
              slot={slot}
              selected={slot === selected}
            />
          ))}
        </div>
      </div>
    </>
  );
});
