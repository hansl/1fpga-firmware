import { useCallback, useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useIntent } from './hooks';

// N6 demo: input handling.
//   Enter / Space  → confirm (counter ++)
//   Escape / Back  → back (counter --)
//   Arrow keys     → navigate (last direction shown)
// Plus the existing image + text from N4/N5.

const root: CSSProperties = {
  display: 'flex',
  width: 1920,
  height: 1080,
  flexDirection: 'column',
  justifyContent: 'center',
  alignItems: 'center',
  // backgroundColor removed temporarily to confirm the FPGA fence
  // bottleneck is the 1080p full-screen fill. Restore once we know.
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

export function App() {
  const [count, setCount] = useState(0);
  const [direction, setDirection] = useState<string>('-');
  const [fps, setFps] = useState(0);

  useEffect(() => {
    const id = setInterval(() => setFps(gui.fps()), 250);
    return () => clearInterval(id);
  }, []);

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
      <div style={title}>menu-ui · N6</div>
      <div style={subtitle}>input router · live</div>
      <div style={stats}>{statsLine}</div>
      <div style={hint}>{hintLine}</div>
    </div>
  );
}
