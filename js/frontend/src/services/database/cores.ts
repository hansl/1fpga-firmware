import * as core from '1fpga:core';
import { Row } from '1fpga:db';

import { CatalogRow } from '@/services/database/catalog';
import { NormalizedCatalog, NormalizedCore } from '@/services/remote/catalog';
import { compareVersions, sql } from '@/utils';

export interface CoreRow extends Row {
  id: number;
  catalogsId: number;
  name: string;
  uniqueName: string;
  rbfPath: string | null;
}

let runningCore: CoreRow | null = null;

// export function pathForAsset(core: RemoteCore, version: string) {
//   return `/media/fat/1fpga/cores/${core.uniqueName}/${version}`;
// }

export function running() {
  return runningCore;
}

export function setRunning(core: CoreRow | null) {
  runningCore = core;
}

export async function create(catalog: CatalogRow, core: NormalizedCore, rbfPath: string | null) {
  const [row] = await sql<CoreRow>`INSERT INTO Cores ${sql.insertValues({
    catalogsId: catalog.id,
    name: core.name,
    uniqueName: core.uniqueName,
    rbfPath,
  })} RETURNING *`;

  for (const s of Array.isArray(core.systems) ? core.systems : [core.systems]) {
    const [{ id: systemsId }] = await sql<{ id: number }>`SELECT id
                                                          FROM Systems
                                                          WHERE uniqueName = ${s}`;
    await sql`INSERT INTO CoresSystems ${sql.insertValues({
      coresId: row.id,
      systemsId,
    })}`;
  }

  return row;
}

export async function getById(id: number): Promise<CoreRow | null> {
  const [row] = await sql<CoreRow>`SELECT *
                                   FROM Cores
                                   WHERE id = ${id}
                                   LIMIT 1`;
  return row;
}

export interface LookupCoreOptions {
  system?: { id: number };
  catalog?: { id: number };
}

function whereClause({ system, catalog }: LookupCoreOptions) {
  return sql.and(
    true,
    system
      ? sql`systemsId =
      ${system.id}`
      : undefined,
    catalog
      ? sql`catalogsId =
      ${catalog.id}`
      : undefined,
  );
}

export async function count(options: LookupCoreOptions = {}): Promise<number> {
  const [{ count }] = await sql<{ count: number }>`SELECT COUNT(*) as count
                                                   FROM Cores
                                                   WHERE ${whereClause(options)}`;

  return count;
}

export async function list(options: LookupCoreOptions = {}): Promise<CoreRow[]> {
  const rows = await sql<CoreRow>`SELECT *
                                  FROM Cores
                                  WHERE ${whereClause(options)}`;

  return rows;
}

/**
 * Launch the core, and the core loop. Does not show the menu.
 */
export async function launch(coreRow: CoreRow) {
  if (!coreRow.rbfPath) {
    throw new Error('Core does not have an RBF path');
  }

  try {
    console.log(`Starting core: ${JSON.stringify(coreRow)}`);
    setRunning(coreRow);
    let c = core.load({
      core: { type: 'Path', path: coreRow.rbfPath },
    });
    const settings = await (
      await import('@/services/settings/user')
    ).UserSettings.forLoggedInUser();
    c.volume = await settings.defaultVolume();
    c.loop();
  } finally {
    setRunning(null);
  }
}
