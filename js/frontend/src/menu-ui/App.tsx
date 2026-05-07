import { memo, useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { animated, useSpring } from './animated';
import { useIntent } from './hooks';

// N9 demo — a horizontal "core picker": a row of cards with a focus
// highlight that animates between cards via react-spring as the user
// presses ← / →. Pressing Enter pulses the focused card. Bottom line
// echoes the focused card's name. Top-left corner shows the live
// frame rate (diagnostic).

interface Item {
  name: string;
  accent: string;
}

const ITEMS: Item[] = [
  { name: 'NES', accent: '#d04040' },
  { name: 'SNES', accent: '#6070ff' },
  { name: 'Genesis', accent: '#3a3a3a' },
  { name: 'Game Boy', accent: '#80a040' },
  { name: 'Atari', accent: '#d09040' },
];

const root: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  width: 1920,
  height: 1080,
  justifyContent: 'space-between',
  alignItems: 'center',
  paddingTop: 80,
  paddingBottom: 80,
  backgroundColor: '#0a0a14',
};

const headerWrap: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  gap: 16,
};

const titleStyle: CSSProperties = {
  fontSize: 72,
  color: '#ffffff',
};

const subtitleStyle: CSSProperties = {
  fontSize: 28,
  color: '#80909a',
};

const listStyle: CSSProperties = {
  display: 'flex',
  flexDirection: 'row',
  alignItems: 'center',
  gap: 32,
};

const cardBase: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  width: 240,
  height: 240,
  alignItems: 'center',
  justifyContent: 'flex-end',
  paddingBottom: 24,
};

const cardLabelStyle: CSSProperties = {
  fontSize: 32,
  color: '#ffffff',
};

const footerWrap: CSSProperties = {
  display: 'flex',
  flexDirection: 'column',
  alignItems: 'center',
  gap: 12,
};

const selectedNameStyle: CSSProperties = {
  fontSize: 48,
  color: '#ffd060',
};

const hintStyle: CSSProperties = {
  fontSize: 24,
  color: '#506070',
};

const fpsStyle: CSSProperties = {
  position: 'absolute',
  top: 12,
  left: 12,
  fontSize: 20,
  color: '#60ff60',
};

// memo() so a focus change only re-renders the two cards whose
// `focused` flipped — the other three keep their existing useSpring
// instance and skip both render and reconciler work for the frame.
// Without this, every parent state change re-renders all 5 cards;
// at 30+ navigations the React commit cost piled up linearly inside
// dispatch_input and pushed the frame loop into the tens-of-ms range.
const Card = memo(function Card({
  item,
  focused,
  pulse,
}: {
  item: Item;
  focused: boolean;
  pulse: number; // bumps when this card is "confirmed" — drives the flash spring.
}) {
  // Background tween between the dim idle colour and the item's
  // accent colour as focus enters/leaves. `pulse` momentarily blends
  // toward white when the user confirms the focused card, then the
  // spring relaxes back to the focused/unfocused base.
  const styles = useSpring({
    backgroundColor: pulse > 0 ? '#ffffff' : focused ? item.accent : '#1a1a2a',
    config: pulse > 0
      ? { tension: 320, friction: 18 } // snappy flash to white
      : { tension: 220, friction: 26 }, // smoother return / focus shift
  });
  return (
    <animated.div style={{ ...cardBase, ...styles }}>
      <div style={cardLabelStyle}>{item.name}</div>
    </animated.div>
  );
});

export function App() {
  const [focus, setFocus] = useState(0);
  // `pulses[i]` increments each time card i is confirmed; the Card
  // component reads this to fire its flash spring.
  const [pulses, setPulses] = useState<number[]>(() => ITEMS.map(() => 0));
  const [fps, setFps] = useState(0);

  // Mirror `focus` into a ref so the `confirm` callback below can
  // read the latest value WITHOUT having `focus` as a useCallback
  // dependency. With the dep, every focus change rebuilt the
  // confirm closure, which made useIntent unsubscribe/resubscribe
  // — listener IDs grew unbounded (3, 4, 5, …) and each resubscribe
  // churned Boa GC roots, slowly inflating the per-frame budget
  // until input dispatch ran tens of ms.
  const focusRef = useRef(focus);
  focusRef.current = focus;

  useEffect(() => {
    const id = setInterval(() => setFps(gui.fps()), 250);
    return () => clearInterval(id);
  }, []);

  useIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setFocus((f) => Math.max(0, f - 1));
      }
    }, []),
  );
  useIntent(
    'navigate_right',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') {
        setFocus((f) => Math.min(ITEMS.length - 1, f + 1));
      }
    }, []),
  );
  useIntent(
    'confirm',
    useCallback((e) => {
      if (e.kind !== 'pressed') return;
      const f = focusRef.current;
      setPulses((ps) => {
        const next = ps.slice();
        next[f] = (next[f] + 1) % 1_000_000;
        return next;
      });
      // Auto-relax the pulse — the spring's first tick will see the
      // bumped value (pulse > 0), the next tick we set it back to 0
      // and the spring tweens back to the resting colour.
      setTimeout(() => {
        setPulses((ps) => {
          const next = ps.slice();
          next[f] = 0;
          return next;
        });
      }, 80);
    }, []),
  );

  return (
    <div style={root}>
      <div style={fpsStyle}>{`${fps.toFixed(1)} fps`}</div>
      <div style={headerWrap}>
        <div style={titleStyle}>menu-ui · N9 demo</div>
        <div style={subtitleStyle}>core picker</div>
      </div>
      <div style={listStyle}>
        {ITEMS.map((item, i) => (
          <Card
            key={item.name}
            item={item}
            focused={i === focus}
            pulse={pulses[i]}
          />
        ))}
      </div>
      <div style={footerWrap}>
        <div style={selectedNameStyle}>{ITEMS[focus].name}</div>
        <div style={hintStyle}>{'← / → navigate   ·   Enter confirm'}</div>
      </div>
    </div>
  );
}
