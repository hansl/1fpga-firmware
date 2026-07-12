// The Home carousel: a horizontal row of system cards sliding under
// a fixed centre cursor, PS5-style. The selected card lifts behind a
// white ring; unselected cards sit dimmed at rest.
//
// The card strip lives in a LayerPortal — a hardware scanout plane.
// The slide tween moves the PLANE (a position-register write per
// frame, zero blit traffic); the plane surface holds PLANE_SLOTS
// cards, pre-rendered past both screen edges, re-rendered only when
// selection styling changes (two cards' damage) or when the window
// recenters. Cards are keyed by uniqueName; the window base moves
// with hysteresis so consecutive steps keep local positions stable.

import { memo, useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import { CENTRE_X, s } from '../scale';
import { Icon, type IconName } from '../components/Icon';
import { LayerPortal } from '../layer';
import type { SystemCard } from '../services/library';

const CARD_W = s(280);
const CARD_H = s(340);
const GAP = s(44);
const SLOT = CARD_W + GAP;
/** Lift + ring headroom so the selected card isn't clipped. */
const FRAME_PAD = s(28);
/** Selected-card lift (translateY, paint-only). */
const LIFT = s(16);
const FRAME_TOP = s(392);

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
  // Selection emphasis is a translateY lift + opacity + ring — NOT a
  // scale. A scale tween puts every texture and text inside the card
  // through the FPGA's scale-mode copy path (per-pixel round trips,
  // the one path the burst blitter doesn't cover) on EVERY tween
  // frame; measured at ~340 ms/frame across a 7-card band. Translate
  // keeps all copies 1:1 on the burst path. Scale can return when
  // scaled blits ride the affine engine's staged path.
  useTween(
    ref,
    { translateY: selected ? -LIFT : 0, opacity: selected ? 1.0 : 0.55 },
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
        translateY: selected ? -LIFT : 0,
        opacity: selected ? 1.0 : 0.55,
      }}
    >
      {/* Selection ring — the card body's PARENT, not a sibling: the
          host's pristine-dst (dst_clear) chain then knows the body
          paints OVER the ring and blends it correctly, while an
          unselected ring (transparent, skipped) leaves the body on
          the pristine fast path (opaque burst write). */}
      <div
        style={{
          position: 'absolute',
          left: -s(5),
          top: -s(5),
          width: CARD_W + s(10),
          height: CARD_H + s(10),
          backgroundColor: selected ? '#ffffffe0' : '#00000000',
        }}
      >
        <div
          style={{
            position: 'absolute',
            left: s(5),
            top: s(5),
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
    </div>
  );
});

/** Slots held by the plane surface. At 1080p that's 11 × s(324) =
 *  3564 px wide — under the plane's 4095 hardware cap, and wider than
 *  the screen: the slide reveals pre-rendered cards. */
const PLANE_SLOTS = 11;
/** Recenter margin: shift the window when the selection gets this
 *  close to its edge. Between recenters, card LOCAL positions are
 *  stable, so a nav step's plane damage is just the two cards whose
 *  selection styling changed. */
const RECENTER_MARGIN = 2;

export const CardCarousel = memo(function CardCarousel({
  systems,
  selected,
}: {
  systems: SystemCard[];
  selected: number;
}) {
  // The plane window's first slot, moved with hysteresis (see above).
  const [base, setBase] = useState(0);
  useEffect(() => {
    const maxBase = Math.max(0, systems.length - PLANE_SLOTS);
    const clampedBase = Math.min(base, maxBase);
    if (
      selected < clampedBase + RECENTER_MARGIN ||
      selected > clampedBase + PLANE_SLOTS - 1 - RECENTER_MARGIN ||
      clampedBase !== base
    ) {
      const next = Math.max(
        0,
        Math.min(selected - Math.floor(PLANE_SLOTS / 2), maxBase),
      );
      if (next !== base) setBase(next);
    }
  }, [selected, base, systems.length]);

  // The slide is the PORTAL's translate — engine-side that's a plane
  // position-register write per tween frame, zero blit traffic.
  //
  // COORDINATE SPLIT (do not merge these): the tween animates ONLY
  // the continuous strip-space term. The window offset base*SLOT goes
  // through the portal's `left` — instant, and committed in the SAME
  // packet as the re-keyed card positions, so the engine's flip
  // applies both atomically. Folding base*SLOT into the tween target
  // let the recenter's coordinate jump GLIDE (260 ms) while the
  // content jumped instantly: the strip visually snapped k slots left
  // and slid back right on every recenter (HW test 4's "resets to the
  // left and moves back and forth").
  const targetLeft = CENTRE_X - (selected + 0.5) * SLOT;
  const ref = useRef<gui.NodeId | null>(null);
  // 'follow' + snapBeyond: the strip chases the selection with
  // velocity proportional to its lag — held key-repeat produces
  // steady continuous motion (a re-targeted timed ease either lags
  // unboundedly or, snap-capped, advances in whole-slot jolts that
  // read as flicker); release settles with no discontinuity. The
  // snap cap stays as the worst-case lag bound (ring never leaves
  // the screen).
  useTween(
    ref,
    { translateX: targetLeft },
    { duration: 110, easing: 'follow', snapBeyond: SLOT * 1.5 },
  );

  const hi = Math.min(systems.length - 1, base + PLANE_SLOTS - 1);
  const visible: Array<{ system: SystemCard; slot: number }> = [];
  for (let i = base; i <= hi; i++) visible.push({ system: systems[i], slot: i - base });

  const current = systems[selected];
  return (
    <>
      <div style={titleStyle}>{current?.name ?? ''}</div>
      <div style={counterStyle}>{`${selected + 1} / ${systems.length}`}</div>
      <LayerPortal
        ref={ref}
        z={1}
        x={base * SLOT}
        y={FRAME_TOP - FRAME_PAD}
        width={PLANE_SLOTS * SLOT}
        height={CARD_H + FRAME_PAD * 2}
        style={{ translateX: targetLeft }}
      >
        {visible.map(({ system, slot }) => (
          <Card
            key={system.uniqueName}
            system={system}
            slot={slot}
            selected={base + slot === selected}
          />
        ))}
      </LayerPortal>
    </>
  );
});
