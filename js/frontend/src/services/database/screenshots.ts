import * as fs from '1fpga:fs';

import { sql } from '@/utils';

import { User } from '../user';
import { ExtendedGamesRow } from './games';

interface ScreenshotRow {
  id: number;
  gameId: number;
  path: string;
  createdAt: Date;
}

export interface LookupScreenshotOptions {
  game?: ExtendedGamesRow;
}

export async function list(game?: ExtendedGamesRow): Promise<ScreenshotRow[]> {
  const rows = await sql<ScreenshotRow>`SELECT *
                                        FROM Screenshots ${
                                          game
                                            ? sql`WHERE gameId =
                                            ${game.id}`
                                            : undefined
                                        }`;

  return rows;
}

/**
 * Save a screenshot to the database.
 * @param game
 * @param screenshot
 */
export async function create(
  game: {
    id: number;
    name: string;
    systemName: string;
  },
  screenshot: Image,
): Promise<ScreenshotRow> {
  const user = User.loggedInUser(true);
  const dir = `/media/fat/1fpga/screenshots/${user.username}/${game.systemName}`;
  const path = `${dir}/${game.name} ${Date.now()}.png`;
  await fs.mkdir(dir, true);

  await screenshot.save(path);

  const [row] = await sql<ScreenshotRow>`INSERT INTO Screenshots ${sql.insertValues({
    gamesId: game.id,
    path,
    usersId: user.id,
  })}
                                           RETURNING *`;

  return row;
}

export async function count() {
  const user = User.loggedInUser(true);
  const [{ count }] = await sql<{ count: number }>`SELECT COUNT(*) as count
                                                   FROM Screenshots
                                                   WHERE usersId = ${user.id}`;

  return count;
}
