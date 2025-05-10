import * as core from '1fpga:core';

import * as db from './database';

/**
 * Launch the core, and the core loop. Does not show the menu.
 */
export async function launchCore(coreRow: db.cores.CoreRow) {
  const path = coreRow.rbfPath;
  if (!path) {
    throw new Error('Core does not have an RBF path');
  }

  try {
    console.log(`Starting core: ${JSON.stringify(coreRow)}`);
    db.cores.setRunning(coreRow);
    let c = core.load({
      core: { type: 'Path', path },
    });

    const settings = await (
      await import('@/services/settings/user')
    ).UserSettings.forLoggedInUser();

    c.volume = await settings.defaultVolume();
    await c.loop();
  } finally {
    db.cores.setRunning(null);
  }
}

export async function launchGame(gameRow: db.games.ExtendedGamesRow) {
  console.log('Launching game: ', JSON.stringify(db));

  // Insert last played time at.
  await db.games.setLastPlayedAt(gameRow, new Date());

  const settings = await (await import('@/services/settings/user')).UserSettings.forLoggedInUser();

  try {
    db.cores.setRunning(await db.cores.getById(gameRow.coresId));
    db.games.setRunning(gameRow);

    const c = core.load({
      core: { type: 'Path', path: gameRow.rbfPath },
      ...(gameRow.romPath ? { game: { type: 'RomPath', path: gameRow.romPath } } : {}),
    });

    if (c) {
      console.log('Starting core: ' + c.name);
      c.volume = await settings.defaultVolume();
      c.on('saveState', async (savestate: Uint8Array, screenshot: Image) => {
        const ss = db.savestates.create(gameRow, savestate, screenshot);
        console.log('Saved state: ', JSON.stringify(ss));
      });
      await c.loop();
    }
  } finally {
    db.games.setRunning(null);
    db.cores.setRunning(null);
  }
}
