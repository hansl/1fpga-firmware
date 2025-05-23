import * as core from '1fpga:core';
import { CoreSettingPage } from '1fpga:core';
import * as osd from '1fpga:osd';

import * as db from '@/services/database';
import * as games from '@/ui/games';

enum SettingReturn {
  Continue,
  ReturnContinue,
  Back,
  Quit,
}

export async function coreSettingsMenu(
  core: core.OneFpgaCore,
  pageLabel?: string,
): Promise<SettingReturn> {
  let shouldReturn = SettingReturn.Continue;
  while (shouldReturn === SettingReturn.Continue) {
    let menu: core.CoreSettingPage | core.CoreSettings = core.settings;

    if (pageLabel !== undefined) {
      menu =
        menu.items.find(
          (item): item is CoreSettingPage => item.kind === 'page' && item.label === pageLabel,
        ) ?? menu;
    }

    shouldReturn = await osd.textMenu<SettingReturn>({
      title: 'Core Settings',
      back: SettingReturn.ReturnContinue,
      items: [
        ...(await Promise.all(
          menu.items.map(item => {
            switch (item.kind) {
              case 'page':
                return {
                  label: item.label,
                  marker: '>',
                  select: async () => {
                    return await coreSettingsMenu(core, item.label);
                  },
                };
              case 'separator':
                return '-';
              case 'label':
                return {
                  label: item.label,
                  selectable: item.selectable,
                };
              case 'file':
                return {
                  label: item.label,
                  marker: item.extensions.join(','),
                  select: async () => {
                    let path = await osd.selectFile(item.label, '/media/fat', {
                      extensions: item.extensions,
                    });
                    if (path) {
                      core.fileSelect(item.id, path);
                      return false;
                    }
                  },
                };
              case 'trigger':
                return {
                  label: item.label,
                  marker: '!',
                  select: () => {
                    core.trigger(item.id);
                  },
                };
              case 'bool':
                return {
                  label: item.label,
                  marker: item.value ? '[X]' : '[ ]',
                  select: (menuItem: osd.TextMenuItem<SettingReturn>) => {
                    item.value = core.boolSelect(item.id, !item.value);
                    menuItem.marker = item.value ? '[X]' : '[ ]';
                  },
                };
              case 'int':
                return {
                  label: item.label,
                  marker: item.choices[item.value],
                  select: (menuItem: osd.TextMenuItem<SettingReturn>) => {
                    item.value = core.intSelect(item.id, (item.value + 1) % item.choices.length);
                    menuItem.marker = item.choices[item.value];
                  },
                };
            }
          }),
        )),
      ],
    });

    if (shouldReturn === undefined) {
      shouldReturn = SettingReturn.Continue;
    }
  }

  return shouldReturn === SettingReturn.ReturnContinue ? SettingReturn.Continue : shouldReturn;
}

const isKindFile = (item: core.CoreSettingsItem): item is core.CoreSettingFileSelect =>
  item.kind === 'file';

export async function coreOsdMenu(
  oneFpgaCore: core.OneFpgaCore,
  coreRow: db.cores.CoreRow | null,
): Promise<core.OsdResult> {
  let menu = oneFpgaCore.settings;

  let fileMenus = menu.items.filter(isKindFile);

  const canLoadGame = coreRow && fileMenus.length > 0;
  const [maybeSystem] = canLoadGame ? await db.systems.find({ core: coreRow }) : [];

  return await osd.textMenu({
    title: 'Core Menu',
    back: false,
    items: [
      ...(canLoadGame && maybeSystem
        ? [
            {
              label: 'Load Game...',
              select: async () => {
                const game = await games.pickGame({
                  title: 'Load Game',
                  system: maybeSystem?.uniqueName,
                });

                if (game?.romPath) {
                  oneFpgaCore.fileSelect(0, game.romPath);
                  return false;
                }
              },
            },
          ]
        : []),
      ...(await Promise.all(
        fileMenus.map(item => ({
          label: item.label,
          select: async () => {
            let path = await osd.selectFile(item.label, '/media/fat', {
              extensions: item.extensions,
            });
            if (path) {
              oneFpgaCore.fileSelect(item.id, path);
              return false;
            }
          },
        })),
      )),
      '-',
      {
        label: 'Core Settings...',
        select: async () => {
          switch (await coreSettingsMenu(oneFpgaCore)) {
            case SettingReturn.Back:
              return undefined;
            case SettingReturn.Quit:
              return true;
            default:
              return undefined;
          }
        },
      },
      {
        label: 'Reset Core',
        select: () => {
          oneFpgaCore.reset();
          return false;
        },
      },
      '-',
      {
        label: 'Quit',
        select: () => true,
      },
    ],
  });
}
