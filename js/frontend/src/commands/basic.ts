import * as core from '1fpga:core';

import { db } from '@/services';
import { Commands, CoreCommandImpl } from '@/services/database/commands';
import * as screenshots from '@/services/database/screenshots';
import { coreOsdMenu } from '@/ui/menus/core_osd';

export class ShowCoreMenuCommand extends CoreCommandImpl {
  key = 'showCoreMenu';
  label = "Show the core's menu";
  category = 'Core';
  default = ["'F12'", 'Guide'];

  // This is used to prevent the menu from being shown multiple times.
  shown = false;

  async execute(core: core.OneFpgaCore) {
    if (!this.shown) {
      try {
        this.shown = true;
        const coreDb = db.cores.running();
        let error = undefined;
        core.showOsd(async () => {
          try {
            return await coreOsdMenu(core, coreDb);
          } catch (e) {
            error = e;
            return true;
          }
        });
        if (error) {
          throw error;
        }
      } finally {
        this.shown = false;
      }
    }
  }
}

export class QuitCoreCommand extends CoreCommandImpl {
  key = 'quitCore';
  label = 'Quit to the main menu';
  category = 'Core';
  default = "'F10'";

  execute(core: core.OneFpgaCore) {
    core.quit();
  }
}

export class ShowDebugLogCommand extends CoreCommandImpl {
  key = 'showDebugLog';
  label = 'Show a debug log';
  category = 'Developer';
  default = "Ctrl + 'D'";

  execute() {
    console.log('Debug log.');
  }
}

export class ScreenshotCommand extends CoreCommandImpl {
  key = 'screenshot';
  label = 'Take a screenshot';
  category = 'Core';
  default = "'SysReq'";

  async execute(core: core.OneFpgaCore) {
    // Verify we have a logged-in user.
    const game = db.games.running();
    if (!game) {
      console.error('No game running.');
      return;
    }
    try {
      await screenshots.create(game, core.screenshot());
      console.log('Saved screenshot');
    } catch (e) {
      console.error('Failed to take a screenshot.', e);
    }
  }
}

export async function init() {
  await Commands.register(ShowCoreMenuCommand);
  await Commands.register(QuitCoreCommand);
  await Commands.register(ShowDebugLogCommand);
  await Commands.register(ScreenshotCommand);
}
