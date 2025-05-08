// The root file being executed by 1FPGA by default.
import { stripIndents } from 'common-tags';

import * as fs from '1fpga:fs';
import * as osd from '1fpga:osd';
import * as video from '1fpga:video';

import production from 'consts:production';
import revision from 'consts:revision';

import { db, settings, user } from '@/services';
import { login } from '@/ui/login';
import { resetAll } from '@/utils';

// Polyfill for events.
(globalThis as any).performance = {
  now: () => Date.now(),
};

// async function debugMenu() {
//   await osd.textMenu({
//     title: 'Debug',
//     back: false,
//     items: [
//       {
//         label: 'Reset All...',
//         select: async () => {
//           const should = await osd.alert({
//             title: 'Reset everything',
//             message: 'Are you sure?',
//             choices: ['No', 'Yes'],
//           });
//           if (should === 1) {
//             await resetAll();
//           }
//         },
//       },
//       {
//         label: 'Input Tester...',
//         select: async () => {
//           await osd.inputTester();
//         },
//       },
//     ],
//   });
// }

//
// async function mainMenu(user: User, startOn: settings.StartOnSetting, settings: UserSettings) {
//   let quit = false;
//   let logout = false;
//
//   // Check the startOn option.
//   switch (startOn.kind) {
//     case StartOnKind.GameLibrary:
//       await gamesMenu();
//       break;
//     case StartOnKind.LastGamePlayed:
//       {
//         const game = await Games.lastPlayed();
//         if (game) {
//           await game.launch();
//         } else {
//           await gamesMenu();
//           break;
//         }
//       }
//       break;
//     case StartOnKind.SpecificGame:
//       {
//         const game = await Games.byId(startOn.game);
//         if (game) {
//           await game.launch();
//         }
//       }
//       break;
//
//     case StartOnKind.MainMenu:
//     default:
//       break;
//   }
//
//   // There is no back menu, but we still need to loop sometimes (when selecting a game, for example).
//   while (!(quit || logout)) {
//     const nbGames = await Games.count({ mergeByGameId: true });
//     const nbCores = await Core.count();
//     const nbScreenshots = await Screenshot.count();
//
//     const gamesMarker = nbGames > 0 ? `(${nbGames})` : '';
//     const coresMarker = nbCores > 0 ? `(${nbCores})` : '';
//     const screenshotsMarker = nbScreenshots > 0 ? `(${nbScreenshots})` : '';
//     const downloadMarker = (await db.catalog.anyUpdatePending()) ? '!' : '';
//
//     await osd.textMenu({
//       title: '',
//       items: [
//         {
//           label: 'Game Library',
//           select: async () => await gamesMenu(),
//           marker: gamesMarker,
//         },
//         {
//           label: 'Cores',
//           select: async () => await coresMenu(),
//           marker: coresMarker,
//         },
//         {
//           label: 'Screenshots',
//           select: async () => await screenshotsMenu(),
//           marker: screenshotsMarker,
//         },
//         '---',
//         {
//           label: 'Settings...',
//           select: async () => await settingsMenu(),
//         },
//         ...(user.admin
//           ? [
//               {
//                 label: 'Download Center...',
//                 marker: downloadMarker,
//                 select: async () => await downloadCenterMenu(),
//               },
//             ]
//           : []),
//         {
//           label: 'Controllers...',
//           select: async () => {
//             await osd.alert('Controllers', 'Not implemented yet.');
//           },
//         },
//         '---',
//         { label: 'About', select: about },
//         ...((await settings.getDevTools())
//           ? [
//               '-',
//               {
//                 label: 'Developer Tools',
//                 select: async () => await debugMenu(),
//               },
//             ]
//           : []),
//         '---',
//         ...((await User.canLogOut()) ? [{ label: 'Logout', select: () => (logout = true) }] : []),
//         { label: 'Exit', select: () => (quit = true) },
//         { label: 'Shutdown', select: () => system.shutdown() },
//       ],
//     });
//   }
//
//   if (quit) {
//     return true;
//   } else if (logout) {
//     await Commands.logout();
//     await User.logout();
//   }
//   return false;
// }

/**
 * Initialize the application.
 */
async function initAll() {
  // Before setting commands (to avoid commands to interfere with the login menu),
  // we need to initialize the user.
  let loggedInUser = await login();

  if (loggedInUser === null) {
    // Run first time setup.
    await (await import('./ui/wizards/first-time-setup')).firstTimeSetup();
    loggedInUser = await user.User.login(undefined, true);

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
    await db.Commands.login(loggedInUser, true);
  } else {
    await db.Commands.login(loggedInUser, false);
  }

  const [s, global] = await Promise.all([
    settings.UserSettings.init(loggedInUser),
    settings.GlobalSettings.init(),
  ]);

  return { user: loggedInUser, settings: s, global };
}

/**
 * Main function of the frontend.
 * @returns `true` if the application should exit.
 */
async function mainInner(): Promise<boolean> {
  const { user, settings, global } = await initAll();

  console.log(user, settings, global);

  return true;
  // if (!user || !settings || !global) {
  //   const choice = await osd.alert({
  //     title: 'Error',
  //     message: 'Could not initialize the application. Do you want to reset everything?',
  //     choices: ['Reset', 'Exit'],
  //   });
  //   if (choice === 1) {
  //     // Clear the database.
  //     await resetDb();
  //     return true;
  //   } else {
  //     return false;
  //   }
  // }
  //
  // let startOn = await settings.startOn();
  //
  // console.log('Starting on:', JSON.stringify(startOn));
  // console.log('Date: ', new Date());
  //
  // let action = undefined;
  //
  // while (true) {
  //   try {
  //     if (action === undefined) {
  //       return await mainMenu(user, startOn, settings);
  //     } else if (action instanceof StartGameAction) {
  //       await action.game.launch();
  //     }
  //     action = undefined;
  //     startOn = { kind: StartOnKind.MainMenu };
  //   } catch (e: any) {
  //     action = undefined;
  //     startOn = { kind: StartOnKind.MainMenu };
  //     if (e instanceof StartGameAction) {
  //       // Set the action for the next round.
  //       action = e;
  //     } else if (e instanceof MainMenuAction) {
  //       // There is a quirk here that if the StartOn is GameLibrary, we will go back
  //       // to the game library instead of the main menu.
  //       switch ((await settings.startOn()).kind) {
  //         case StartOnKind.GameLibrary:
  //           startOn = { kind: StartOnKind.GameLibrary };
  //       }
  //     } else {
  //       // Rethrow to show the user the actual error.
  //       let choice = await osd.alert({
  //         title: 'An error happened',
  //         message: (e as Error)?.message ?? JSON.stringify(e),
  //         choices: ['Restart', 'Quit'],
  //       });
  //       if (choice === 1) {
  //         return true;
  //       }
  //     }
  //   }
  // }
}

export async function main() {
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
