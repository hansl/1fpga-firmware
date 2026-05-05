// menu-ui entry — N1 smoke test.
//
// Builds a single full-screen red <div> via the 1fpga:gui host
// module, hands it to `gui.run`, and lets the Rust frame loop paint
// it. Subsequent milestones replace this with a React reconciler
// driving real components.

import { appendChild, createInstance, run } from '1fpga:gui';

export async function main(): Promise<void> {
  console.log('menu-ui: main() entered');

  const root = createInstance('div', {
    style: {
      width: 1920,
      height: 1080,
      top: 0,
      left: 0,
      backgroundColor: '#202040',
    },
  });
  console.log('menu-ui: root created', root);

  const box = createInstance('div', {
    style: {
      width: 400,
      height: 400,
      top: 340,
      left: 760,
      backgroundColor: '#ff4040',
    },
  });
  console.log('menu-ui: box created', box);

  appendChild(root, box);
  console.log('menu-ui: appendChild done; calling run');

  run(root);
  console.log('menu-ui: run() returned');
}
