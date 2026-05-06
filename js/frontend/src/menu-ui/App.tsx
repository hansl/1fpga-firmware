import type { CSSProperties } from 'react';

// N3 demo: a flexbox-centered red box. The root is a flex container
// sized to the framebuffer; the child has a fixed size and centers via
// `justifyContent` / `alignItems` instead of hardcoded `top`/`left`.

const root: CSSProperties = {
  display: 'flex',
  width: 1920,
  height: 1080,
  justifyContent: 'center',
  alignItems: 'center',
  backgroundColor: '#202040',
};

const box: CSSProperties = {
  width: 400,
  height: 400,
  backgroundColor: '#ff4040',
};

export function App() {
  return (
    <div style={root}>
      <div style={box} />
    </div>
  );
}
