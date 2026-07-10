// Local content scanner: populate the catalog tables from what is
// actually installed on the SD card.
//
// MiSTer's on-disk layout is the source of truth until the remote
// catalog services are ported: cores live as `Name_YYYYMMDD.rbf`
// under the visibility-grouped top-level dirs (_Console, _Computer,
// _Other, _Utility), arcade sets as .mra files under _Arcade, and
// game files under /media/fat/games/<SystemName>/.
//
// The scan writes ordinary catalog rows (Catalogs/'local', Systems,
// Cores, CoresSystems, CoresTags) so everything downstream — and the
// future catalog downloader — reads one shape. Group membership is
// recorded as a tag per core ('console', 'computer', …).
//
// Perf note: db promises settle one drain-pass per UI tick, so this
// deliberately issues a FIXED, SMALL number of batched statements
// (executeRaw + executeMany with subselect-resolved foreign keys)
// instead of per-row awaits — a full rescan is a handful of ticks
// regardless of how many cores are installed.

import * as fs from '1fpga:fs';

import { getDb } from './db';

export const FAT_ROOT = '/media/fat';
export const GAMES_ROOT = `${FAT_ROOT}/games`;

/** Top-level core dirs and the group tag their cores receive. */
const CORE_DIRS: Array<[string, string]> = [
  ['_Console', 'console'],
  ['_Computer', 'computer'],
  ['_Other', 'other'],
  ['_Utility', 'utility'],
];

/** `Name_20250903.rbf` → ['Name', '20250903']. */
const RBF_RE = /^(.+)_(\d{8})\.rbf$/i;

export interface ScanSummary {
  cores: number;
}

interface FoundCore {
  uniqueName: string;
  rbfPath: string;
  tag: string;
}

async function findCores(): Promise<FoundCore[]> {
  const found: FoundCore[] = [];
  for (const [dir, tag] of CORE_DIRS) {
    let entries: fs.DirEntry[];
    try {
      entries = await fs.readDirEntries(`${FAT_ROOT}/${dir}`);
    } catch {
      continue; // Dir absent on this card — fine.
    }
    for (const e of entries) {
      if (e.dir || e.name.startsWith('.')) continue;
      const m = RBF_RE.exec(e.name);
      if (!m) continue;
      found.push({
        uniqueName: m[1],
        rbfPath: `${FAT_ROOT}/${dir}/${e.name}`,
        tag,
      });
    }
  }
  return found;
}

/**
 * Scan the SD card and upsert the catalog tables. Idempotent —
 * re-running updates rbf paths and picks up added/renamed cores.
 */
export async function scanLocalContent(): Promise<ScanSummary> {
  const d = await getDb();
  const cores = await findCores();

  // Static setup: the 'local' catalog, the group tags, and the
  // synthetic Arcade system (its "games" are .mra files; there is no
  // single arcade core — each mra names its own).
  await d.executeRaw(`
    INSERT INTO Catalogs (name, uniqueName, url, json)
    SELECT 'Local Files', 'local', 'local://', '{}'
    WHERE NOT EXISTS (SELECT 1 FROM Catalogs WHERE uniqueName = 'local');
    INSERT OR IGNORE INTO Tags (name)
    VALUES ('console'), ('computer'), ('other'), ('utility'), ('arcade');
    INSERT INTO Systems (catalogsId, name, uniqueName)
    SELECT (SELECT id FROM Catalogs WHERE uniqueName = 'local'), 'Arcade', 'Arcade'
    WHERE NOT EXISTS (SELECT 1 FROM Systems WHERE uniqueName = 'Arcade');
  `);

  if (cores.length > 0) {
    // One system per core (MiSTer cores are 1:1 with their system;
    // the catalog port refines this mapping later).
    await d.executeMany(
      `INSERT INTO Systems (catalogsId, name, uniqueName)
       SELECT (SELECT id FROM Catalogs WHERE uniqueName = 'local'), ?, ?
       WHERE true
       ON CONFLICT(uniqueName) DO NOTHING`,
      cores.map((c) => [c.uniqueName, c.uniqueName]),
    );
    await d.executeMany(
      `INSERT INTO Cores (catalogsId, name, uniqueName, rbfPath)
       SELECT (SELECT id FROM Catalogs WHERE uniqueName = 'local'), ?, ?, ?
       WHERE true
       ON CONFLICT(uniqueName) DO UPDATE SET rbfPath = excluded.rbfPath`,
      cores.map((c) => [c.uniqueName, c.uniqueName, c.rbfPath]),
    );
    await d.executeMany(
      `INSERT OR IGNORE INTO CoresSystems (coresId, systemsId)
       SELECT c.id, s.id FROM Cores c, Systems s
       WHERE c.uniqueName = ? AND s.uniqueName = ?`,
      cores.map((c) => [c.uniqueName, c.uniqueName]),
    );
    await d.executeMany(
      `INSERT OR IGNORE INTO CoresTags (coresId, tagsId)
       SELECT c.id, t.id FROM Cores c, Tags t
       WHERE c.uniqueName = ? AND t.name = ?`,
      cores.map((c) => [c.uniqueName, c.tag]),
    );
  }

  await d.execute(
    `INSERT INTO GlobalStorage (key, value) VALUES ('scan.lastRun', ?)
     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updatedAt = CURRENT_TIMESTAMP`,
    [JSON.stringify({ at: new Date().toISOString(), cores: cores.length })],
  );

  console.log(`scan: ${cores.length} cores registered`);
  return { cores: cores.length };
}
