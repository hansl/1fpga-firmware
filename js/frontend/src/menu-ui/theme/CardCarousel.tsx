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

import {
  forwardRef,
  memo,
  useCallback,
  useEffect,
  useImperativeHandle,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useTween } from '../hooks';
import { CENTRE_X, s } from '../scale';
import { Icon, type IconName } from '../components/Icon';
import { LayerPortal } from '../layer';
import { createMicroStore, useMicroStore, type MicroStore } from '../microStore';
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
  abs,
  selected,
  onNodes,
}: {
  system: SystemCard;
  slot: number;
  /** Absolute system index — the imperative selection path keys its
   *  node registry by this. */
  abs: number;
  selected: boolean;
  /** Stable reporter: (abs, kind, nodeId | null on unmount). */
  onNodes: (abs: number, kind: 'card' | 'ring', id: gui.NodeId | null) => void;
}) {
  const ref = useRef<gui.NodeId | null>(null);
  const ringRef = useRef<gui.NodeId | null>(null);
  // Selection emphasis is a translateY lift + opacity + ring — NOT a
  // scale (scale-mode copies are the slow blit path). All three ride
  // 'follow' tweens: under held key-repeat the emphasis re-targets
  // every step, and restarted timed eases (plus latest-wins frame
  // dropping) turned each step into a near-binary POP — two bright
  // blinks per step at repeat rate read as flicker even with the
  // strip itself gliding smoothly (HW test 10's diff maps: labels
  // doubled ~30 px apart, ring rectangles a full card apart).
  // Followers blend re-targets mid-flight: the ring cross-fades
  // between cards, the lift/dim glide continuously.
  useTween(
    ref,
    { translateY: selected ? -LIFT : 0, opacity: selected ? 1.0 : 0.55 },
    { duration: 90, easing: 'follow' },
  );
  // Ring: constant white fill, SELECTION EXPRESSED AS OPACITY so the
  // follower can fade it. Faded out, its effective alpha is 0 and the
  // host skips the fill op entirely (zero-alpha skip) — an invisible
  // ring costs nothing. A sibling (not the body's parent) so its
  // opacity doesn't multiply into the card content.
  useTween(
    ringRef,
    { opacity: selected ? 0.88 : 0.0 },
    { duration: 90, easing: 'follow' },
  );
  const art = artFor(system.uniqueName);
  return (
    <div
      ref={(id: gui.NodeId | null) => {
        ref.current = id;
        onNodes(abs, 'card', id);
      }}
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
      <div
        ref={(id: gui.NodeId | null) => {
          ringRef.current = id;
          onNodes(abs, 'ring', id);
        }}
        style={{
          position: 'absolute',
          left: -s(5),
          top: -s(5),
          width: CARD_W + s(10),
          height: CARD_H + s(10),
          backgroundColor: '#ffffff',
          opacity: selected ? 0.88 : 0.0,
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

/** Slots held by the plane surface. At 1080p that's 12 × s(324) =
 *  3888 px wide — under the plane's 4095 hardware cap, and wider than
 *  the screen: the slide reveals pre-rendered cards. 12 (was 11)
 *  stretches the recenter interval to ~7 held steps — the recenter is
 *  the most expensive reconcile we have (every card re-styled), and
 *  its frequency set the freeze cadence under held keys. */
const PLANE_SLOTS = 12;
/** Recenter margin: shift the window when the selection gets this
 *  close to its edge. Between recenters, card LOCAL positions are
 *  stable, so a nav step's plane damage is just the two cards whose
 *  selection styling changed. */
const RECENTER_MARGIN = 2;

/** Imperative surface: selection changes ride this handle, NOT props.
 *  In Boa's interpreter a full carousel reconcile costs 30-60 ms; a
 *  selection step through here costs four startTween calls and two
 *  one-text-node micro-renders. React keeps ownership of STRUCTURE
 *  (mounts, window recenters, data changes) — the same split the rest
 *  of the pipeline uses: reconcile for structure, registers/tweens
 *  for animation. */
export interface CarouselHandle {
  getSelected(): number;
  select(next: number): void;
}

const EMPHASIS_TWEEN: gui.TweenOpts = { duration: 90, easing: 'follow' };
const STRIP_TWEEN: gui.TweenOpts = {
  duration: 110,
  easing: 'follow',
  snapBeyond: SLOT * 1.5,
};

function StoreText({
  store,
  style,
}: {
  store: MicroStore<string>;
  style: CSSProperties;
}) {
  const text = useMicroStore(store);
  return <div style={style}>{text}</div>;
}

export const CardCarousel = memo(
  forwardRef(function CardCarousel(
    { systems }: { systems: SystemCard[] },
    handle: React.Ref<CarouselHandle>,
  ) {
    // Selection lives in a REF (no reconcile per step); the window
    // base is React state (a recenter re-renders the strip, every
    // ~7 held steps). baseRef mirrors it for reads inside select().
    const selRef = useRef(0);
    const [base, setBase] = useState(0);
    const baseRef = useRef(0);
    baseRef.current = base;

    const titleStore = useMemo(() => createMicroStore(''), []);
    const counterStore = useMemo(() => createMicroStore(''), []);
    useEffect(() => {
      selRef.current = Math.min(selRef.current, Math.max(0, systems.length - 1));
      titleStore.set(systems[selRef.current]?.name ?? '');
      counterStore.set(`${selRef.current + 1} / ${systems.length}`);
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [systems]);

    // Host-node registry for the imperative tweens, keyed by absolute
    // system index. Cards report on mount/unmount via a STABLE
    // callback (a fresh closure per render would defeat Card's memo).
    const nodes = useRef(
      new Map<number, { card?: gui.NodeId | null; ring?: gui.NodeId | null }>(),
    );
    const onNodes = useCallback(
      (abs: number, kind: 'card' | 'ring', id: gui.NodeId | null) => {
        const entry = nodes.current.get(abs) ?? {};
        entry[kind] = id;
        if (id == null && entry.card == null && entry.ring == null) {
          nodes.current.delete(abs);
        } else {
          nodes.current.set(abs, entry);
        }
      },
      [],
    );

    const ref = useRef<gui.NodeId | null>(null);

    // Portal fade-in on mount — hardware alpha ramp (see PLANE_ALPHA).
    // Initial 0 is imperative, never JSX: commitUpdate REPLACES style,
    // so a static 0 would re-apply on the next differing render and
    // blank the plane with nothing re-arming the one-shot tween.
    useLayoutEffect(() => {
      if (ref.current != null) {
        gui.updateStyle(ref.current, { opacity: 0 });
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);
    useTween(ref, { opacity: 1.0 }, { duration: 140, easing: 'follow' });

    // Strip position for renders (mount, recenter). Between renders
    // the imperative path re-targets the same follow tween.
    const targetLeft = CENTRE_X - (selRef.current + 0.5) * SLOT;
    useTween(ref, { translateX: targetLeft }, STRIP_TWEEN);

    useImperativeHandle(
      handle,
      () => ({
        getSelected: () => selRef.current,
        select(next: number) {
          const clamped = Math.max(0, Math.min(systems.length - 1, next));
          const prev = selRef.current;
          if (clamped === prev) return;
          selRef.current = clamped;

          // ---- Hot path: zero reconcile. -------------------------
          const off = nodes.current.get(prev);
          if (off?.card != null) {
            gui.startTween(
              off.card,
              { translateY: 0, opacity: 0.55 },
              EMPHASIS_TWEEN,
            );
          }
          if (off?.ring != null) {
            gui.startTween(off.ring, { opacity: 0.0 }, EMPHASIS_TWEEN);
          }
          const on = nodes.current.get(clamped);
          if (on?.card != null) {
            gui.startTween(
              on.card,
              { translateY: -LIFT, opacity: 1.0 },
              EMPHASIS_TWEEN,
            );
          }
          if (on?.ring != null) {
            gui.startTween(on.ring, { opacity: 0.88 }, EMPHASIS_TWEEN);
          }
          if (ref.current != null) {
            gui.startTween(
              ref.current,
              { translateX: CENTRE_X - (clamped + 0.5) * SLOT },
              STRIP_TWEEN,
            );
          }
          titleStore.set(systems[clamped]?.name ?? '');
          counterStore.set(`${clamped + 1} / ${systems.length}`);

          // ---- Structure change: the React path (every ~7 steps). -
          const maxBase = Math.max(0, systems.length - PLANE_SLOTS);
          const b = Math.min(baseRef.current, maxBase);
          if (
            clamped < b + RECENTER_MARGIN ||
            clamped > b + PLANE_SLOTS - 1 - RECENTER_MARGIN ||
            b !== baseRef.current
          ) {
            const nb = Math.max(
              0,
              Math.min(clamped - Math.floor(PLANE_SLOTS / 2), maxBase),
            );
            if (nb !== baseRef.current) setBase(nb);
          }
        },
      }),
      [systems, titleStore, counterStore],
    );

    const hi = Math.min(systems.length - 1, base + PLANE_SLOTS - 1);
    const visible: Array<{ system: SystemCard; slot: number }> = [];
    for (let i = base; i <= hi; i++)
      visible.push({ system: systems[i], slot: i - base });

    return (
      <>
        <StoreText store={titleStore} style={titleStyle} />
        <StoreText store={counterStore} style={counterStyle} />
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
              abs={base + slot}
              selected={base + slot === selRef.current}
              onNodes={onNodes}
            />
          ))}
        </LayerPortal>
      </>
    );
  }),
);
