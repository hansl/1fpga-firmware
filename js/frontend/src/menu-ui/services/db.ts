// Database bootstrap: one shared handle to the 1fpga database with
// the schema guaranteed applied.
//
// Every service goes through getDb() — the first caller triggers
// load + migration; everyone else awaits the same promise. Migration
// state lives in PRAGMA user_version (0 on a fresh file), advanced
// once per entry in MIGRATIONS.

import * as db from '1fpga:db';

import { MIGRATIONS, SCHEMA_VERSION } from './schema';

let handle: Promise<db.Db> | null = null;

async function openAndMigrate(): Promise<db.Db> {
  const d = await db.load('1fpga');
  const row = await d.queryOne<{ user_version: number }>('PRAGMA user_version');
  const version = row?.user_version ?? 0;
  if (version < SCHEMA_VERSION) {
    console.log(`db: migrating schema v${version} -> v${SCHEMA_VERSION}`);
    for (let v = version; v < SCHEMA_VERSION; v++) {
      await d.executeRaw(MIGRATIONS[v]);
      await d.executeRaw(`PRAGMA user_version = ${v + 1}`);
    }
  }
  return d;
}

/** The shared 1fpga database, schema applied. */
export function getDb(): Promise<db.Db> {
  if (!handle) {
    handle = openAndMigrate().catch((e) => {
      // Allow a retry on the next call rather than caching failure
      // forever (e.g. SD card briefly unavailable at boot).
      handle = null;
      throw e;
    });
  }
  return handle;
}

/** Read a JSON value from GlobalStorage; null when absent/malformed. */
export async function globalGet<T>(key: string): Promise<T | null> {
  const d = await getDb();
  const row = await d.queryOne<{ value: string }>(
    'SELECT value FROM GlobalStorage WHERE key = ?',
    [key],
  );
  if (!row) return null;
  try {
    return JSON.parse(row.value) as T;
  } catch {
    console.warn(`globalGet(${key}): malformed JSON value`);
    return null;
  }
}

/** Write a JSON value to GlobalStorage (upsert). */
export async function globalSet(key: string, value: unknown): Promise<void> {
  const d = await getDb();
  await d.execute(
    `INSERT INTO GlobalStorage (key, value) VALUES (?, ?)
     ON CONFLICT(key) DO UPDATE SET value = excluded.value, updatedAt = CURRENT_TIMESTAMP`,
    [key, JSON.stringify(value)],
  );
}
