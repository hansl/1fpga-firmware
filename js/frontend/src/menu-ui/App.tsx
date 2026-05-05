import type { CSSProperties } from 'react';

const root: CSSProperties = {
  width: 1920,
  height: 1080,
  top: 0,
  left: 0,
  backgroundColor: '#202040',
};

const box: CSSProperties = {
  width: 400,
  height: 400,
  top: 340,
  left: 760,
  backgroundColor: '#ff4040',
};

export function App() {
  return (
    <div style={root}>
      <div style={box} />
    </div>
  );
}
