import type { CSSProperties } from 'react';

// N5 demo: stacked column with two text labels and a PNG image.
// The image lives at /media/fat/menu_ui_test.png — copy any PNG
// there, or run `just deploy-menu-ui-test-png` to use the bundled
// docs asset.

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

const image: CSSProperties = {
  // Width / height come from the decoded PNG's intrinsic size when
  // omitted; explicit values would override.
  marginTop: 24,
};

export function App() {
  return (
    <div style={root}>
      <div style={title}>Hello, 1FPGA!</div>
      <div style={subtitle}>menu-ui · N5 · text + images</div>
      <img src="/media/fat/menu_ui_test.png" style={image} />
    </div>
  );
}
