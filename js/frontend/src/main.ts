// The root file being executed by 1FPGA by default.
import type * as schemas from '@1fpga/schemas';
import { stripIndents } from 'common-tags';

import * as fs from '1fpga:fs';
import * as osd from '1fpga:osd';
import * as system from '1fpga:system';
import * as video from '1fpga:video';

import production from 'consts:production';
import revision from 'consts:revision';

import { MainMenuAction } from '@/actions/main_menu';
import { StartGameAction } from '@/actions/start_game';
import * as services from '@/services';
import * as ui from '@/ui';
import { resetAll, resetDb } from '@/utils';

// Polyfill for events.
(globalThis as any).performance = {
  now: () => Date.now(),
};

async function debugMenu() {
  await osd.textMenu({
    title: 'Debug',
    back: false,
    items: [
      {
        label: 'Reset All...',
        select: async () => {
          const should = await osd.alert({
            title: 'Reset everything',
            message: 'Are you sure?',
            choices: ['No', 'Yes'],
          });
          if (should === 1) {
            await resetAll();
          }
        },
      },
      {
        label: 'Input Tester...',
        select: async () => {
          await osd.inputTester();
        },
      },
    ],
  });
}

async function mainMenu(
  u: services.user.User,
  startOn: schemas.settings.StartOnSetting,
  s: services.settings.UserSettings,
) {
  let quit = false;
  let logout = false;

  // Check the startOn option.
  switch (startOn.kind) {
    case services.settings.StartOnKind.GameLibrary:
      await ui.games.gamesMenu();
      break;
    case services.settings.StartOnKind.LastGamePlayed:
      {
        const game = await services.db.games.lastPlayedExtended();
        if (game) {
          await services.launch.game(game);
        } else {
          await ui.games.gamesMenu();
          break;
        }
      }
      break;
    case services.settings.StartOnKind.SpecificGame:
      {
        const game = await services.db.games.getExtended(startOn.game);
        if (game) {
          await services.launch.game(game);
        }
      }
      break;

    case services.settings.StartOnKind.MainMenu:
    default:
      break;
  }

  async function notImplemented(s: string) {
    await osd.alert('Not implemented yet: ' + s);
  }

  // There is no back menu, but we still need to loop sometimes (when selecting a game, for example).
  while (!(quit || logout)) {
    const nbGames = await services.db.games.count();
    const nbCores = await services.db.cores.count();
    const nbScreenshots = await services.db.screenshots.count();

    const gamesMarker = nbGames ? `(${nbGames})` : '';
    const coresMarker = nbCores ? `(${nbCores})` : '';
    const screenshotsMarker = nbScreenshots ? `(${nbScreenshots})` : '';
    const downloadMarker = (await services.db.catalog.anyUpdatePending()) ? '!' : '';

    await osd.textMenu({
      title: '',
      items: [
        {
          label: 'Game Library',
          select: async () => await ui.games.gamesMenu(),
          marker: gamesMarker,
        },
        {
          label: 'Cores',
          select: async () => await ui.cores.select(),
          marker: coresMarker,
        },
        {
          label: 'Screenshots',
          select: async () => await ui.screenshots.screenshotsMenu(),
          marker: screenshotsMarker,
        },
        '---',
        {
          label: 'Settings...',
          select: async () => await notImplemented('settingsMenu'),
        },
        ...(u.admin
          ? [
              {
                label: 'Download Center...',
                marker: downloadMarker,
                select: async () => await notImplemented('downloadCenterMenu'),
              },
            ]
          : []),
        {
          label: 'Controllers...',
          select: async () => {
            await osd.alert('Controllers', 'Not implemented yet.');
          },
        },
        '---',
        { label: 'About', select: async () => await notImplemented('about') },
        ...((await s.getDevTools())
          ? [
              '-',
              {
                label: 'Developer Tools',
                select: async () => await debugMenu(),
              },
            ]
          : []),
        '---',
        ...((await services.user.User.canLogOut())
          ? [{ label: 'Logout', select: () => (logout = true) }]
          : []),
        { label: 'Exit', select: () => (quit = true) },
        { label: 'Shutdown', select: () => system.shutdown() },
      ],
    });
  }

  if (quit) {
    return true;
  } else if (logout) {
    await services.db.Commands.logout();
    await services.user.User.logout();
  }
  return false;
}

/**
 * Initialize the application.
 */
async function initAll() {
  // Before setting commands (to avoid commands to interfere with the login menu),
  // we need to initialize the user.
  let loggedInUser = await ui.login.login();

  if (loggedInUser === null) {
    // Run first time setup.
    await (await import('./ui/wizards/first-time-setup')).firstTimeSetup();
    loggedInUser = await services.user.User.login(undefined, true);

    if (loggedInUser === null) {
      await osd.alert(
        'Error',
        stripIndents`
          Could not log in after initial setup. This is a bug.

          Please report this issue to the developers.

          The application will now exit.
        `,
      );
      return {};
    }
    await services.db.Commands.login(loggedInUser, true);
  } else {
    await services.db.Commands.login(loggedInUser, false);
  }

  const [s, global] = await Promise.all([
    services.settings.UserSettings.init(loggedInUser),
    services.settings.GlobalSettings.init(),
  ]);

  return { user: loggedInUser, settings: s, global };
}

/**
 * Main function of the frontend.
 * @returns `true` if the application should exit.
 */
async function mainInner(): Promise<boolean> {
  const { user, settings, global } = await initAll();

  if (!user || !settings || !global) {
    const choice = await osd.alert({
      title: 'Error',
      message: 'Could not initialize the application. Do you want to reset everything?',
      choices: ['Reset', 'Exit'],
    });
    if (choice === 1) {
      // Clear the database.
      await resetDb();
      return true;
    } else {
      return false;
    }
  }

  let startOn = await settings.startOn();

  console.log('Starting on:', JSON.stringify(startOn));
  console.log('Date: ', new Date());

  let action = undefined;

  while (true) {
    try {
      if (action === undefined) {
        return await mainMenu(user, startOn, settings);
      } else if (action instanceof StartGameAction) {
        await services.launch.game(action.game);
      }
      action = undefined;
      startOn = { kind: services.settings.StartOnKind.MainMenu };
    } catch (e: any) {
      action = undefined;
      startOn = { kind: services.settings.StartOnKind.MainMenu };
      if (e instanceof StartGameAction) {
        // Set the action for the next round.
        action = e;
      } else if (e instanceof MainMenuAction) {
        // There is a quirk here that if the StartOn is GameLibrary, we will go back
        // to the game library instead of the main menu.
        switch ((await settings.startOn()).kind) {
          case services.settings.StartOnKind.GameLibrary:
            startOn = { kind: services.settings.StartOnKind.GameLibrary };
        }
      } else {
        // Rethrow to show the user the actual error.
        let choice = await osd.alert({
          title: 'An error happened',
          message: (e as Error)?.message ?? JSON.stringify(e),
          choices: ['Restart', 'Quit'],
        });
        if (choice === 1) {
          return true;
        }
      }
    }
  }
}

export async function main() {
  osd.show('1FPGA Booting Up', 'Please wait...');

  console.log(`Build: "${revision}" (${production ? 'production' : 'development'})`);
  console.log('1FPGA started. ONE_FPGA =', JSON.stringify(ONE_FPGA));
  let quit = false;

  // Log the last time this was started.
  await fs.writeFile('1fpga.start', new Date().toISOString());

  const start = Date.now();
  const resolution = video.getResolution();
  let image = await Image.embedded('background');

  if (resolution) {
    console.log('Resolution:', resolution.width, 'x', resolution.height);
    const imageAr = image.width / image.height;
    const resolutionAr = resolution.width / resolution.height;
    if (imageAr > resolutionAr) {
      resolution.width = resolution.height * imageAr;
    } else if (imageAr < resolutionAr) {
      resolution.height = resolution.width / imageAr;
    }
    image = image.resize(resolution.width, resolution.height);
  }

  image.sendToBackground({ position: 'center', clear: true });
  console.log('Background set in', Date.now() - start, 'ms');

  while (!quit) {
    quit = await mainInner();
  }
}
