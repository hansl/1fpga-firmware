// The library: what the UI actually renders — systems for the Home
// carousel, browsable entries for a Collection screen.
//
// ensureBooted() is the single boot gate: schema applied, local scan
// done, memoized so every caller (router boot resolution, screens)
// awaits the same work.

import * as fs from '1fpga:fs';

import { getDb } from './db';
import { GAMES_ROOT, FAT_ROOT, scanLocalContent } from './scan';

export interface SystemCard {
  id: number;
  name: string;
  uniqueName: string;
  /** Group tag: console | computer | other | utility | arcade. */
  tag: string;
  rbfPath: string | null;
  /** Directory whose contents are this system's game list, when one
   *  exists on the card. */
  gamesPath: string | null;
}

export interface LibEntry {
  name: string;
  dir: boolean;
  size: number;
}

let booted: Promise<void> | null = null;

/** Schema + local scan, exactly once per process. */
export function ensureBooted(): Promise<void> {
  if (!booted) {
    booted = (async () => {
      await getDb();
      await scanLocalContent();
    })().catch((e) => {
      booted = null;
      throw e;
    });
  }
  return booted;
}

const TAG_ORDER = ['console', 'arcade', 'computer', 'other', 'utility'];

/**
 * Systems for the Home carousel: consoles first, then arcade,
 * computers, the rest; alphabetical within a group. `gamesPath` is
 * resolved against the card so the UI can distinguish "browsable
 * collection" from "core only".
 */
export async function listSystems(): Promise<SystemCard[]> {
  await ensureBooted();
  const d = await getDb();
  const { rows } = await d.query<{
    id: number;
    name: string;
    uniqueName: string;
    rbfPath: string | null;
    tag: string | null;
  }>(
    `SELECT Systems.id         AS id,
            Systems.name       AS name,
            Systems.uniqueName AS uniqueName,
            Cores.rbfPath      AS rbfPath,
            Tags.name          AS tag
     FROM Systems
              LEFT JOIN CoresSystems ON CoresSystems.systemsId = Systems.id
              LEFT JOIN Cores ON Cores.id = CoresSystems.coresId
              LEFT JOIN CoresTags ON CoresTags.coresId = Cores.id
              LEFT JOIN Tags ON Tags.id = CoresTags.tagsId
     GROUP BY Systems.id
     ORDER BY Systems.name COLLATE NOCASE`,
  );

  // Resolve games dirs in one parallel wave (fs promises settle
  // immediately; Promise.all keeps it a single tick).
  const paths = rows.map((r) =>
    r.uniqueName === 'Arcade' ? `${FAT_ROOT}/_Arcade` : `${GAMES_ROOT}/${r.uniqueName}`,
  );
  const present = await Promise.all(paths.map((p) => fs.isDir(p)));

  const cards: SystemCard[] = rows.map((r, i) => ({
    id: r.id,
    name: r.name,
    uniqueName: r.uniqueName,
    tag: r.tag ?? (r.uniqueName === 'Arcade' ? 'arcade' : 'other'),
    rbfPath: r.rbfPath,
    gamesPath: present[i] ? paths[i] : null,
  }));
  cards.sort((a, b) => {
    const ta = TAG_ORDER.indexOf(a.tag);
    const tb = TAG_ORDER.indexOf(b.tag);
    if (ta !== tb) return ta - tb;
    return a.name.localeCompare(b.name);
  });
  return cards;
}

export async function getSystem(uniqueName: string): Promise<SystemCard | null> {
  const all = await listSystems();
  return all.find((s) => s.uniqueName === uniqueName) ?? null;
}

/**
 * Browsable entries of a directory inside a system's game tree:
 * directories first, then files, alphabetical; hidden files and
 * MiSTer housekeeping dropped.
 */
export async function listEntries(path: string): Promise<LibEntry[]> {
  const entries = await fs.readDirEntries(path);
  const visible = entries.filter((e) => !e.name.startsWith('.'));
  visible.sort((a, b) => {
    if (a.dir !== b.dir) return a.dir ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  return visible;
}
