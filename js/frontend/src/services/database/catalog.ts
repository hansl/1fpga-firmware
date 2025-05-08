import { Row } from '1fpga:db';

import { NormalizedCatalog } from '@/services/remote/catalog';
import { sql } from '@/utils';

export interface CatalogRow extends Row {
  id: number;
  name: string;
  url: string;
  lastUpdateAt: string;
  version: string;
  priority: number;
  updatePending: boolean;

  json: string;
}

export async function create(json: NormalizedCatalog, priority: number) {
  const [catalog] = await sql<CatalogRow>`INSERT INTO Catalogs ${sql.insertValues({
    name: json.name,
    uniqueName: json.uniqueName,
    url: json._url,
    version: json._version ?? json.version ?? null,
    priority,
    json: JSON.stringify(json),
  })} RETURNING *`;
  return catalog;
}

export interface ListCatalogsOptions {
  updatePending?: boolean;
  url?: string;
}

export async function list({ updatePending, url }: ListCatalogsOptions = {}) {
  const rows = await sql<CatalogRow>`SELECT *
                                     FROM Catalogs
                                     WHERE ${sql.and(
                                       true,
                                       url
                                         ? sql`url =
                                         ${url}`
                                         : undefined,
                                       updatePending
                                         ? sql`update_pending =
                                         ${updatePending}`
                                         : undefined,
                                     )}
  `;
  return rows.sort((a, b) => a.priority - b.priority);
}

export async function anyUpdatePending() {
  const [{ up }] = await sql<{ up: boolean }>`SELECT COUNT(1) as up
                                              FROM Catalogs
                                              WHERE updatePending IS TRUE`;

  return up;
}

export async function exists(url: string) {
  const [{ found }] = await sql<{ found: boolean }>`SELECT 1 as found
                                                    FROM Catalogs
                                                    WHERE url = ${url}`;
  return found;
}
