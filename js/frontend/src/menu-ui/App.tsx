import { useCallback, useState } from 'react';
import type { CSSProperties } from 'react';

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

export function App() {
  const [count, setCount] = useState(0);
  const [direction, setDirection] = useState<string>('—');

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

  return (
    <div style={root}>
      <div style={title}>menu-ui · N6</div>
      <div style={subtitle}>input router · live</div>
      <div style={stats}>
        count: {count}   ·   nav: {direction}
      </div>
      <div style={hint}>
        Enter/Space ➝ +1   ·   Esc/Backspace ➝ −1   ·   Arrows ➝ navigate
      </div>
    </div>
  );
}
