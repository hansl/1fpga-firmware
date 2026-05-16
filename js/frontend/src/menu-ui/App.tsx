import { memo, useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useIntent, useTween } from './hooks';

// N9 demo — a horizontal "core picker": a row of cards with a focus
// highlight that moves between cards as the user presses ← / →.
// Pressing Enter pulses the focused card. Bottom line echoes the
// focused core's name. Background image + per-system icons load from
// /media/fat/menu_ui_assets (deployed via `just deploy-menu-ui-assets`).

interface Item {
  name: string;
  accent: string;
  icon: string;
}

const ASSETS = '/media/fat/menu_ui_assets';

const ITEMS: Item[] = [
  { name: 'NES',      accent: '#d04040', icon: `${ASSETS}/nes.png` },
  { name: 'SNES',     accent: '#6070ff', icon: `${ASSETS}/snes.png` },
  { name: 'Genesis',  accent: '#3a3a3a', icon: `${ASSETS}/genesis.png` },
  { name: 'Game Boy', accent: '#80a040', icon: `${ASSETS}/gameboy.png` },
  { name: 'Atari',    accent: '#d09040', icon: `${ASSETS}/atari.png` },
];

const root: CSSProperties = {
  position: 'relative',
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

const bgImageStyle: CSSProperties = {
  position: 'absolute',
  top: 0,
  left: 0,
  width: 1920,
  height: 1080,
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
  justifyContent: 'space-between',
  paddingTop: 28,
  paddingBottom: 24,
};

const cardIconStyle: CSSProperties = {
  width: 128,
  height: 128,
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

const Card = memo(function Card({
  item,
  focused,
  flashing,
}: {
  item: Item;
  focused: boolean;
  flashing: boolean;
}) {
  const bg = flashing ? '#ffffff' : focused ? item.accent : '#1a1a2a';
  // Unfocused cards dim to 50% so focus is unambiguous. Opacity is
  // multiplicative down the subtree, so the icon + label dim too.
  const targetOpacity = flashing ? 1 : focused ? 1 : 0.5;
  // Focused card scales up to 1.08; unfocused stays at 1.0. Scale
  // inherits multiplicatively, so the icon + label grow with the
  // card around its center. Animate via the host-side tween manager
  // so React only commits when focus state actually flips.
  const targetScale = focused ? 1.08 : 1.0;
  const ref = useRef<gui.NodeId | null>(null);
  useTween(
    ref,
    { opacity: targetOpacity, scale: targetScale },
    { duration: 180, easing: 'easeOut' },
  );
  return (
    <div
      ref={ref}
      style={{
        ...cardBase,
        backgroundColor: bg,
        opacity: targetOpacity,
        scale: targetScale,
      }}
    >
      <img src={item.icon} style={cardIconStyle} />
      <div style={cardLabelStyle}>{item.name}</div>
    </div>
  );
});

export function App() {
  const [focus, setFocus] = useState(0);
  // -1 = nothing flashing. Set to focused index on confirm,
  // back to -1 after one paint via setTimeout(0).
  const [flashIdx, setFlashIdx] = useState(-1);
  const [fps, setFps] = useState(0);

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
      setFocus((f) => {
        setFlashIdx(f);
        setTimeout(() => setFlashIdx(-1), 120);
        return f;
      });
    }, []),
  );

  return (
    <div style={root}>
      <img src={`${ASSETS}/bg.png`} style={bgImageStyle} />
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
            flashing={i === flashIdx}
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
