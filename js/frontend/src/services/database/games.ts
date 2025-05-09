import * as oneFpgaCore from '1fpga:core';
import { Row } from '1fpga:db';

import { CoreRow } from '@/services/database/cores';
import { NormalizedCore, NormalizedSystem } from '@/services/remote/catalog';
import { sql } from '@/utils';

import { User } from '../user';
import type { GamesId } from './games_identification';

/**
 * A Row from the Games table.
 */
export interface GamesRow extends Row {
  id: number;
  name: string;
  gamesId: number;
  coresId: number;
  path: string;
  size: number;
  sha256: string;
}

/**
 * A Row from the ExtendedGamesView which links games with their systems and
 * other useful information.
 */
export interface ExtendedGamesRow extends Row {
  id: number;
  name: string;
  romPath: string | null;
  rbfPath: string;
  systemName: string;
  coresId: number;
  favorite: boolean | null;
  lastPlayedAt: string | null;
}

export enum GameSortOrder {
  NameAsc = 'name ASC',
  NameDesc = 'name DESC',
  SystemAsc = 'systems.uniqueName ASC',
  LastPlayed = 'UserGames.lastPlayedAt DESC',
  Favorites = 'UserGames.favorite DESC, UserGames.lastPlayedAt DESC',
}

export interface GamesListOptions {
  /**
   * The sort order.
   */
  sort?: GameSortOrder;

  limit?: number;

  index?: number;

  /**
   * Merge games with the same game identification.
   */
  includeUnplayed?: boolean;
  includeUnfavorites?: boolean;
  system?: string;
}

function buildSqlQuery(options: GamesListOptions) {
  return sql`
    SELECT *
    FROM ExtendedGamesView
    WHERE ${sql.and(
      true,
      options.system
        ? sql`systems.uniqueName =
        ${options.system}`
        : undefined,
      (options.includeUnplayed ?? true) ? undefined : sql`UserGames.lastPlayedAt IS NOT NULL`,
      (options.includeUnfavorites ?? true) ? undefined : sql`UserGames.favorite = true`,
    )}
    ORDER BY ${sql.raw(options.sort ?? GameSortOrder.NameAsc)}
    LIMIT ${options.limit ?? 100} OFFSET ${options.index ?? 0}
  `;
}

export async function count(options: GamesListOptions = {}) {
  const [{ count }] = await sql<{ count: number }>`
    SELECT COUNT(*) as count
    FROM (${buildSqlQuery(options)})
  `;
  return count;
}

export async function getExtended(id: number) {
  const [row] = await sql<ExtendedGamesRow>`
    SELECT *
    FROM ExtendedGamesView
    WHERE Games.id = ${id}
  `;
  return row;
}

export async function lastPlayedExtended() {
  const [row] = await sql<ExtendedGamesRow>`
    SELECT *
    FROM ExtendedGamesView
    WHERE UserGames.lastPlayedAt IS NOT NULL
    ORDER BY UserGames.lastPlayedAt DESC
    LIMIT 1
  `;
  return row ?? null;
}

/**
 * Return the first game we can find.
 */
export async function first() {
  const [row] = await sql<ExtendedGamesRow>`
    SELECT *
    FROM ExtendedGamesView
    LIMIT 1
  `;
  return row ?? null;
}

export async function listExtended(options: GamesListOptions = {}) {
  const games = await sql<ExtendedGamesRow>`${buildSqlQuery(options)}`;
  const total = await count(options);

  return { total, games: games };
}

export async function setFavorite(row: ExtendedGamesRow, favorite: boolean) {
  if (row.favorite !== favorite) {
    await sql`INSERT INTO UserGames
                ${sql.insertValues({
                  userId: User.loggedInUser(true).id,
                  gamesId: row.id,
                  favorite,
                })}
              ON CONFLICT
    DO
    UPDATE SET favorite = excluded.favorite`;
  }
  row.favorite = favorite;
}

let runningGame: ExtendedGamesRow | null = null;

export function running() {
  return runningGame;
}

export function setRunning(g: ExtendedGamesRow | null) {
  runningGame = g;
}

export async function setLastPlayedAt(g: ExtendedGamesRow, lastPlayedAt: Date) {
  await sql`INSERT INTO UserGames
              ${sql.insertValues({
                userId: User.loggedInUser(true).id,
                gamesId: g.id,
                lastPlayedAt: lastPlayedAt.toString(),
              })}
            ON CONFLICT
  DO
  UPDATE SET lastPlayedAt = excluded.lastPlayedAt`;
}

export async function createIdentifiedGame(g: GamesId): Promise<GamesRow> {
  console.log('Creating game', g);
  const [row] = await sql<GamesRow>`
    INSERT INTO Games ${sql.insertValues({
      name: g.title,
      systemsId: g.systemsId,
      path: g.path,
      size: g.size,
      sha256: g.sha256,
    })}
      RETURNING *
  `;

  // Create tags and regions
  for (const name of JSON.parse(g.regions)) {
    const [{ id }] = await sql<{ id: number }>`INSERT INTO Regions ${sql.insertValues({ name })}
                                               ON CONFLICT
    DO UPDATE SET name = excluded.name RETURNING id`;

    await sql`INSERT INTO GamesRegions ${sql.insertValues({
      gamesId: row.id,
      regionsId: id,
    })}`;
  }
  for (const name of JSON.parse(g.tags)) {
    const [{ id }] = await sql<{ id: number }>`INSERT INTO Tags ${sql.insertValues({ name })}
                                               ON CONFLICT
    DO UPDATE SET name = excluded.name RETURNING id`;

    await sql`INSERT INTO GamesTags ${sql.insertValues({
      gamesId: row.id,
      tagsId: id,
    })}`;
  }

  return row;
}

export async function createCoreGame(
  core: NormalizedCore,
  coreRow: CoreRow,
  s: NormalizedSystem[],
) {
  const [row] = await sql<GamesRow>`
    INSERT INTO Games
      ${sql.insertValues({
        name: core.name,
        coresId: coreRow.id,
      })}
      RETURNING *
  `;
  console.log(core.name, row);

  return row;
}

//   async launch() {
//     console.log('Launching game: ', JSON.stringify(this.row_));
//
//     // Insert last played time at.
//     await sql`INSERT INTO UserGames
//                 ${sql.insertValues({
//                   userId: User.loggedInUser(true).id,
//                   gamesId: this.id,
//                   lastPlayedAt: '' + new Date(),
//                 })}
//               ON CONFLICT
//     DO
//     UPDATE SET lastPlayedAt = excluded.lastPlayedAt`;
//
//     const settings = await (
//       await import('@/services/settings/user')
//     ).UserSettings.forLoggedInUser();
//
//     try {
//       Core.setRunning(await Core.getById(this.row_.coresId));
//       Games.runningGame = this;
//       const core = oneFpgaCore.load({
//         core: { type: 'Path', path: this.row_.rbfPath },
//         ...(this.row_.romPath ? { game: { type: 'RomPath', path: this.row_.romPath } } : {}),
//       });
//
//       if (core) {
//         console.log('Starting core: ' + core.name);
//         core.volume = await settings.defaultVolume();
//         core.on('saveState', async (savestate: Uint8Array, screenshot: Image) => {
//           const ss = SaveState.create(this, savestate, screenshot);
//           console.log('Saved state: ', JSON.stringify(ss));
//         });
//         core.loop();
//       }
//     } finally {
//       Games.runningGame = null;
//       Core.setRunning(null);
//     }
//   }
// }
