import { oneLine } from 'common-tags';

import * as oneFpgaCore from '1fpga:core';

import { sql } from '@/utils';

import { User } from '../user';
import type { GamesId } from './games_identification';

export interface GamesRow {
  id: number;
  name: string;
  gamesId: number;
  coresId: number;
  path: string;
  size: number;
  sha256: string;
}

/**
 * A games row with more information than is in the Games table.
 */
export interface ExtendedGamesRow {
  id: number;
  name: string;
  romPath: string | null;
  rbfPath: string;
  systemName: string;
  coresId: number;
  favorite: boolean | null;
  lastPlayedAt: Date | null;
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
  mergeByGameId?: boolean;
  includeUnplayed?: boolean;
  includeUnfavorites?: boolean;
  system?: string;
}

const GAMES_FIELDS = oneLine`
  Games.id AS id,
  Games.path AS romPath,
  IFNULL(Cores2.rbfPath, cores.rbfPath) AS rbfPath,
  IFNULL(GamesIdentification.name, Games.name) AS name,
  Systems.uniqueName AS systemName,
  UserGames.favorite,
  UserGames.lastPlayedAt,
  IFNULL(UserGames.coresId, CoresSystems.coresId) AS coresId
`;

const GAMES_FROM_JOIN = oneLine`
  games
    LEFT JOIN GamesIdentification ON Games.gamesId = GamesIdentification.id
    LEFT JOIN systems AS systems_2 ON GamesIdentification.systemsId = systems_2.id
    LEFT JOIN cores AS Cores2 ON games.coresId = Cores2.id
    LEFT JOIN cores_systems ON Cores2.id = CoresSystems.coresId OR games.coresId = CoresSystems.coresId OR systems_2.id = cores_systems.systemsId
    LEFT JOIN cores ON games.coresId = cores.id OR cores_systems.coresId = cores.id
    LEFT JOIN systems ON GamesIdentification.systemsId = systems.id OR cores_systems.systemsId = systems.id
    LEFT JOIN UserGames ON UserGames.gamesId = games.id
`;

const GROUP_BY_GAME_ID = oneLine`
    GROUP BY IFNULL(GamesIdentification.id, cores.rbfPath)
`;

function buildSqlQuery(options: GamesListOptions) {
  return sql`
    SELECT ${sql.raw(GAMES_FIELDS)}
    FROM ${sql.raw(GAMES_FROM_JOIN)}
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

export async function count(options: GamesListOptions) {
  const [{ count }] = await sql<{ count: number }>`
    SELECT COUNT(*) as count
    FROM (${buildSqlQuery(options)})
  `;
  return count;
}

export async function getExtended(id: number) {
  const [row] = await sql<ExtendedGamesRow>`
    SELECT ${sql.raw(GAMES_FIELDS)}
    FROM ${sql.raw(GAMES_FROM_JOIN)}
    WHERE Games.id = ${id}
  `;
  return row;
}

export async function lastPlayedExtended() {
  const [row] = await sql<ExtendedGamesRow>`
    SELECT ${sql.raw(GAMES_FIELDS)}
    FROM ${sql.raw(GAMES_FROM_JOIN)}
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
    SELECT ${sql.raw(GAMES_FIELDS)}
    FROM ${sql.raw(GAMES_FROM_JOIN)}
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
