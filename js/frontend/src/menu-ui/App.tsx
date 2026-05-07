import { useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties } from 'react';
import * as gui from '1fpga:gui';

import { useAnimationFrame, useIntent } from './hooks';

// N7 demo: requestAnimationFrame.
//   - The "pulse" box at top-right cycles its background color via
//     gui.setStyle on every frame. The hook uses RAF directly and
//     never touches React state, so per-frame updates don't trigger
//     reconciler work.
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

const pulseStyle: CSSProperties = {
  position: 'absolute',
  top: 12,
  right: 12,
  width: 80,
  height: 32,
  backgroundColor: '#202040',
};

// HSL→RGB without bringing in a library. h is 0..1, s/l are 0..1.
function hslHex(h: number, s: number, l: number): string {
  const a = s * Math.min(l, 1 - l);
  const f = (n: number): number => {
    const k = (n + h * 12) % 12;
    return l - a * Math.max(-1, Math.min(k - 3, 9 - k, 1));
  };
  const toHex = (x: number): string =>
    Math.round(Math.max(0, Math.min(1, x)) * 255)
      .toString(16)
      .padStart(2, '0');
  return `#${toHex(f(0))}${toHex(f(8))}${toHex(f(4))}`;
}

export function App() {
  const [count, setCount] = useState(0);
  const [direction, setDirection] = useState<string>('-');
  const [fps, setFps] = useState(0);

  useEffect(() => {
    const id = setInterval(() => setFps(gui.fps()), 250);
    return () => clearInterval(id);
  }, []);

  // RAF-driven pulse: cycle the pulse box's background through the
  // hue wheel without going through React. Period = 4 seconds.
  const pulseRef = useRef<gui.NodeId | null>(null);
  useAnimationFrame((now) => {
    const id = pulseRef.current;
    if (id == null) return;
    const hue = ((now / 4000) % 1 + 1) % 1;
    gui.setStyle(id, { backgroundColor: hslHex(hue, 0.7, 0.5) });
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
      <div
        style={pulseStyle}
        ref={(node) => {
          // Our reconciler returns the host NodeId (a number), not a
          // real HTMLDivElement; cast through unknown to keep TS happy.
          pulseRef.current = node as unknown as gui.NodeId | null;
        }}
      />
      <div style={title}>menu-ui · N7</div>
      <div style={subtitle}>input router · live</div>
      <div style={stats}>{statsLine}</div>
      <div style={hint}>{hintLine}</div>
    </div>
  );
}
