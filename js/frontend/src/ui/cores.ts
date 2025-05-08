import * as core from '1fpga:core';
import * as osd from '1fpga:osd';

import { ShowCoreMenuCommand } from '@/commands/basic';
import { db } from '@/services';
import { launchCore } from '@/services/utils';

async function selectCoreFile() {
  let f = await osd.selectFile('Select Core', '/media/fat', {
    dirFirst: false,
    extensions: ['rbf'],
  });

  if (f !== undefined) {
    db.cores.setRunning(null);
    let c = core.load({
      core: { type: 'Path', path: f },
    });

    await db.Commands.get(ShowCoreMenuCommand)?.execute(c, undefined);
    c.loop();
  }
}

export async function coresMenu() {
  const cores = await db.cores.list();

  await osd.textMenu({
    title: 'Cores',
    back: 0,
    items: [
      ...cores.map(core => ({
        label: '' + core.name,
        select: async () => {
          await launchCore(core);
        },
      })),
      '-',
      { label: 'Select File...', select: selectCoreFile },
    ],
  });
}
