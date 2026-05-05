// menu-ui entry — N1 smoke test.
//
// Builds a single full-screen red <div> via the 1fpga:gui host
// module, hands it to `gui.run`, and lets the Rust frame loop paint
// it. Subsequent milestones replace this with a React reconciler
// driving real components.

import { appendChild, createInstance, run } from '1fpga:gui';

export async function main(): Promise<void> {
  const root = createInstance('div', {
    style: {
      width: 1920,
      height: 1080,
      top: 0,
      left: 0,
      backgroundColor: '#202040',
    },
  });

  // Inner box: 400x400 red, centred-ish, to confirm append + child
  // positioning paint.
  const box = createInstance('div', {
    style: {
      width: 400,
      height: 400,
      top: 340,
      left: 760,
      backgroundColor: '#ff4040',
    },
  });
  appendChild(root, box);

  run(root);
}
