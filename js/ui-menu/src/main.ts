import * as dom from '1fpga:dom';
import * as core from '1fpga:core';

// Polyfill for Boa compatibility
(globalThis as any).performance = {
  now: () => Date.now(),
};

export async function main() {
  console.log('1FPGA React Menu starting (direct DOM test)');

  const root = dom.getRootNode();
  console.log('Root node:', root);

  // Build DOM tree directly (no React, testing the pipeline)
  // Background image
  const bg = dom.createElement('image');
  dom.setProp(bg, 'src', 'embedded:background');
  dom.setProp(bg, 'position', 'absolute');
  dom.setProp(bg, 'top', 0);
  dom.setProp(bg, 'left', 0);
  dom.setProp(bg, 'width', 640);
  dom.setProp(bg, 'height', 480);
  dom.appendChild(root, bg);

  // Container for menu
  const container = dom.createElement('view');
  dom.setProp(container, 'flexDirection', 'column');
  dom.setProp(container, 'width', 640);
  dom.setProp(container, 'height', 480);
  dom.setProp(container, 'justifyContent', 'center');
  dom.setProp(container, 'alignItems', 'center');
  dom.appendChild(root, container);

  // Menu box
  const menuBox = dom.createElement('view');
  dom.setProp(menuBox, 'flexDirection', 'column');
  dom.setProp(menuBox, 'backgroundColor', [0, 0, 0]);
  dom.setProp(menuBox, 'opacity', 0.85);
  dom.setProp(menuBox, 'padding', 24);
  dom.setProp(menuBox, 'gap', 8);
  dom.setProp(menuBox, 'alignItems', 'center');
  dom.appendChild(container, menuBox);

  // Start button (focused)
  const startBtn = dom.createElement('view');
  dom.setProp(startBtn, 'backgroundColor', [80, 80, 220]);
  dom.setProp(startBtn, 'padding', 12);
  dom.setProp(startBtn, 'width', 200);
  dom.setProp(startBtn, 'alignItems', 'center');
  dom.appendChild(menuBox, startBtn);

  const startText = dom.createText('Start');
  dom.appendChild(startBtn, startText);

  // Quit button
  const quitBtn = dom.createElement('view');
  dom.setProp(quitBtn, 'padding', 12);
  dom.setProp(quitBtn, 'width', 200);
  dom.setProp(quitBtn, 'alignItems', 'center');
  dom.appendChild(menuBox, quitBtn);

  const quitText = dom.createText('Quit');
  dom.appendChild(quitBtn, quitText);

  dom.requestRender();
  console.log('DOM tree built, entering render loop');

  let focused = 0;
  const items = ['start', 'quit'];

  // Enter the Rust render loop
  await dom.startRenderLoop((event: string) => {
    if (event === 'up' && focused > 0) {
      focused--;
      dom.setProp(startBtn, 'backgroundColor', focused === 0 ? [80, 80, 220] : undefined);
      dom.setProp(quitBtn, 'backgroundColor', focused === 1 ? [80, 80, 220] : undefined);
    } else if (event === 'down' && focused < items.length - 1) {
      focused++;
      dom.setProp(startBtn, 'backgroundColor', focused === 0 ? [80, 80, 220] : undefined);
      dom.setProp(quitBtn, 'backgroundColor', focused === 1 ? [80, 80, 220] : undefined);
    } else if (event === 'select') {
      dom.exitRenderLoop(items[focused]);
    }
  });

  console.log('Exited render loop');
}
