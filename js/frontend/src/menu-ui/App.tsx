import { useCallback, useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { animated, useSpring } from './animated';
import { useIntent } from './hooks';

// N8 demo: react-spring shim.
//   - The "swatch" box at top-right is an `animated.div`; pressing
//     Enter/Space cycles its background color via useSpring. The
//     spring's frame loop runs on our gui.requestAnimationFrame; per-
//     tick value application skips React entirely and lands in
//     gui.setStyle (see animated.tsx).
//   Existing N6 input bindings continue to work unchanged.

const root: CSSProperties = {
  display: 'flex',
  width: 1920,
  height: 1080,
  flexDirection: 'column',
  justifyContent: 'center',
  alignItems: 'center',
  backgroundColor: '#101028',
  gap: 16,
};

const title: CSSProperties = {
  fontSize: 96,
  color: '#ffffff',
};

const subtitle: CSSProperties = {
  fontSize: 36,
  color: '#90a0c0',
};

const stats: CSSProperties = {
  fontSize: 48,
  color: '#ffd060',
};

const hint: CSSProperties = {
  fontSize: 28,
  color: '#80909a',
  marginTop: 24,
};

const fpsStyle: CSSProperties = {
  position: 'absolute',
  top: 12,
  left: 12,
  fontSize: 24,
  color: '#60ff60',
};

const swatchStyle: CSSProperties = {
  position: 'absolute',
  top: 12,
  right: 12,
  width: 80,
  height: 32,
  backgroundColor: '#202040',
};

// Cycle through accent colors on each confirm press; useSpring picks
// up the change and tweens the background.
const ACCENT_COLORS = ['#202040', '#ff5060', '#60ffa0', '#5080ff', '#ffd060'];

export function App() {
  const [count, setCount] = useState(0);
  const [direction, setDirection] = useState<string>('-');
  const [fps, setFps] = useState(0);

  useEffect(() => {
    const id = setInterval(() => setFps(gui.fps()), 250);
    return () => clearInterval(id);
  }, []);

  // The swatch's animated background. Index walks ACCENT_COLORS as
  // count changes; useSpring interpolates the colour smoothly. Slow
  // tension/friction so the tween spans roughly 700ms — at our ~17fps
  // paint rate that's ~12 frames, which reads as a smooth fade rather
  // than a single discrete jump.
  const swatchSpring = useSpring({
    from: { backgroundColor: ACCENT_COLORS[0] },
    backgroundColor:
      ACCENT_COLORS[
        ((count % ACCENT_COLORS.length) + ACCENT_COLORS.length) %
          ACCENT_COLORS.length
      ],
    config: { tension: 80, friction: 30 },
  });

  useIntent(
    'confirm',
    useCallback((e) => {
      if (e.kind === 'pressed') setCount((c) => c + 1);
    }, []),
  );
  useIntent(
    'back',
    useCallback((e) => {
      if (e.kind === 'pressed') setCount((c) => c - 1);
    }, []),
  );
  useIntent(
    'navigate_up',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') setDirection('up');
    }, []),
  );
  useIntent(
    'navigate_down',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') setDirection('down');
    }, []),
  );
  useIntent(
    'navigate_left',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') setDirection('left');
    }, []),
  );
  useIntent(
    'navigate_right',
    useCallback((e) => {
      if (e.kind === 'pressed' || e.kind === 'repeat') setDirection('right');
    }, []),
  );

  // Note: JSX text interpolation produces multiple text nodes, and
  // our layout doesn't yet flow text inline (each text node is a
  // block-level child). Compose with a template literal so the
  // entire line is one text node.
  const statsLine = `count: ${count}   ·   nav: ${direction}`;
  const hintLine =
    'Enter/Space → +1   ·   Esc/Backspace → -1   ·   Arrows → navigate';

  return (
    <div style={root}>
      <div style={fpsStyle}>{`${fps.toFixed(1)} fps`}</div>
      <animated.div style={{ ...swatchStyle, ...swatchSpring }} />
      <div style={title}>menu-ui · N8</div>
      <div style={subtitle}>input router · live</div>
      <div style={stats}>{statsLine}</div>
      <div style={hint}>{hintLine}</div>
    </div>
  );
}
