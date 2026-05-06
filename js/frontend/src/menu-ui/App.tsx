import type { CSSProperties } from 'react';

// N4 demo: a flex column centered on the framebuffer with two text
// labels at different sizes/colors. Validates per-glyph paint, color
// tinting, and font-size atlas caching.

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

const accent: CSSProperties = {
  width: 120,
  height: 4,
  backgroundColor: '#ff4040',
  marginTop: 24,
};

export function App() {
  return (
    <div style={root}>
      <div style={title}>Hello, 1FPGA!</div>
      <div style={subtitle}>menu-ui · N4 · text rendering</div>
      <div style={accent} />
    </div>
  );
}
