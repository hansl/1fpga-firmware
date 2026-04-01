// Polyfills MUST be imported before React so the scheduler sees them.
import './polyfills';

import * as React from 'react';
import * as dom from '1fpga:dom';
import * as core from '1fpga:core';
import { render } from './renderer';
import { App } from './components/App';

export async function main() {
  console.log('1FPGA React Menu starting');

  let selectedAction: string | null = null;
  const inputRef: React.MutableRefObject<((event: string) => void) | null> = { current: null };

  function handleSelect(action: string) {
    selectedAction = action;
    dom.exitRenderLoop(action);
  }

  // Render the React tree into the Rust DOM
  try {
    console.log('Creating React element...');
    const element = React.createElement(App, { onSelect: handleSelect, inputRef });
    console.log('Calling render...');
    render(element);
    console.log('Render complete, flushing jobs...');
  } catch (e) {
    console.error('React render failed:', e);
    throw e;
  }

  // Flush React's scheduled work so the DOM tree is populated before entering the render loop
  try {
    console.log('Flushing jobs...');
    dom.flushJobs();
    console.log('Jobs flushed successfully');
  } catch (e) {
    console.error('flushJobs failed:', e);
    throw e;
  }

  console.log('React render flushed, entering render loop');

  // Enter the Rust render loop. This blocks until exitRenderLoop is called.
  await dom.startRenderLoop((event: string) => {
    // Dispatch input to the React component tree
    if (inputRef.current) {
      inputRef.current(event);
    }
    // Flush React updates triggered by setState
    dom.flushJobs();
  });

  console.log('Menu selected:', selectedAction);

  if (selectedAction === 'start') {
    const c = await core.load({
      core: { type: 'Path', path: '/media/fat/memtest.rbf' },
    });
    await (c as any).loop();
  }
}
