// The root file being executed by 1FPGA by default.
import * as fs from '1fpga:fs';
import * as osd from '1fpga:osd';

import production from 'consts:production';
import revision from 'consts:revision';

import { run } from './components/test';

// Polyfill for events.
(globalThis as any).performance = {
  now: () => Date.now(),
};

export async function main() {
  osd.show('1FPGA Booting Up', 'Please wait...');
  console.log(`Build: "${revision}" (${production ? 'production' : 'development'})`);
  console.log('1FPGA started. ONE_FPGA =', JSON.stringify(ONE_FPGA));
  let quit = false;

  await run();

  // await new Promise(r => setTimeout(r, 10000));
  // console.log('running...');
  //
  // // Log the last time this was started.
  // await fs.writeFile('1fpga.start', new Date().toISOString());
  //
  // const { mainInner } = await import('@/ui/main');
  // while (!quit) {
  //   quit = await mainInner();
  // }
}
