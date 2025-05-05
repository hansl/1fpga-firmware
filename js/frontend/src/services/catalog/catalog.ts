import * as osd from "1fpga:osd";
import type * as schemas from "@1fpga/schemas";
import { fetchJsonAndValidate, sql, ValidationError } from "@/utils";
import { Row } from "1fpga:db";

export interface CatalogRow extends Row {
  id: number;
  name: string;
  url: string;
  lastUpdated: string;
  version: string;
  priority: number;
  updatePending: boolean;

  json: string;
}

/**
 * Understand whether an inner object is a URL or versioned URL or actual
 * type. If this is a URL, the `_url` will be filled with the resolved
 * URL fetched, and if it's a versioned URL, the version will also be
 * set as the `_version` property.
 */
async function fetchVersioned<InnerSchema extends schemas.ZodType>(
  baseUrl: string | undefined,
  value: unknown,
  schema: InnerSchema,
): Promise<Normalized<schemas.TypeOf<typeof schema>>> {
  if (typeof value == "string") {
    const url = new URL(value, baseUrl).toString();
    osd.show("Fetching cores...", "URL: " + url);
    return {
      ...(await fetchJsonAndValidate<InnerSchema>(url, schema, {
        allowRetry: true,
      })),
      _url: url,
    } as Normalized<InnerSchema>;
  } else if (
    typeof value === "object" &&
    value !== null &&
    typeof (value as any).url == "string"
  ) {
    const url = new URL((value as any).url, baseUrl).toString();
    const version = `${(value as any).version}`;
    osd.show("Fetching cores...", "URL: " + url);
    return {
      ...(await fetchJsonAndValidate<InnerSchema>(url, schema, {
        allowRetry: true,
      })),
      _url: url,
      _version: version,
    } as Normalized<InnerSchema>;
  } else if (schema.safeParse(value).success) {
    return value as unknown as InnerSchema;
  } else {
    throw new ValidationError("Invalid value for schema.");
  }
}

type Normalized<T> = T & {
  _url?: string;
  _version?: string;
};

export function denormalize<N extends Normalized<T>, T>(n: N): Omit<T, "_url" | "_version"> {
  const { _url, _version, ...d } = n;
  return d;
}

export type NormalizedSystem = Normalized<schemas.catalog.System>;

export type NormalizedSystems = Normalized<
  Record<string, NormalizedSystem>
>;

export type NormalizedCore = Normalized<schemas.catalog.Core>;

export type NormalizedCores = Normalized<
  Record<string, NormalizedCore>
>;

/**
 * A normalized catalog with cores.
 */
export type NormalizedCatalog = Normalized<schemas.catalog.Catalog> & {
  cores?: NormalizedCores;
  systems?: NormalizedSystems;
};

async function fetchInnerOfRecord<T, O>(
  record: Normalized<T>,
  schema: schemas.ZodType,
): Promise<O> {
  const base = record._url;
  const result = {
    ...record,
  } as O;

  for (const [name, value] of Object.entries(record)) {
    if (name === "_url" || name === "_version") {
      continue;
    }

    (result as any)[name] = await fetchVersioned(base, value, schema);
  }

  return result;
}

async function fetchInner<T extends schemas.ZodType>(
  baseUrl: string,
  record: unknown,
  schema: schemas.ZodType,
  innerSchema?: schemas.ZodType,
): Promise<Normalized<T> | undefined> {
  if (record === undefined) {
    return undefined;
  }

  const inner = await fetchVersioned(baseUrl, record, schema);
  if (innerSchema) {
    return await fetchInnerOfRecord(inner, innerSchema);
  }
}

export async function fetchAndNormalizeCatalog(
  url: string,
): Promise<NormalizedCatalog> {
  // Normalize the URL.
  url = new URL(url).toString();
  osd.show("Fetching catalog...", "URL: " + url);

  if (!url.startsWith("https://") && !url.startsWith("http://")) {
    url = "https://" + url;
  }

  const schemas = await import("@1fpga/schemas");
  try {
    let catalog = {
      ...(await fetchJsonAndValidate<schemas.catalog.Catalog>(
        url,
        schemas.catalog.Catalog,
        {
          allowRetry: false,
        },
      )),
      _url: url,
    } as unknown as Normalized<schemas.catalog.Catalog>;

    catalog.cores = await fetchInner(
      url,
      catalog.cores,
      schemas.catalog.Cores,
      schemas.catalog.Core,
    );
    catalog.systems = await fetchInner(
      url,
      catalog.systems,
      schemas.catalog.Systems,
      schemas.catalog.System,
    );
    catalog.releases = await fetchInner(
      url,
      catalog.releases,
      schemas.catalog.Releases,
    );

    // At this point all internals have been normalized.
    return catalog as NormalizedCatalog;
  } catch (e) {
    if (e instanceof ValidationError) {
      throw e;
    }

    console.error("Error fetching catalog:", (e as any)?.message || e);

    // If this is http, try with https.
    // If this doesn't end with `catalog.json`, try adding it.
    if (!url.endsWith("/catalog.json")) {
      return fetchAndNormalizeCatalog(new URL("catalog.json", url).toString());
    } else if (url.startsWith("http://")) {
      return fetchAndNormalizeCatalog(url.replace(/^http:\/\//, "https://"));
    } else {
      throw e;
    }
  }
}

export async function fetchAndCreateCatalogDbEntry(
  url: string,
  priority: number = 0,
): Promise<ICatalog> {
  const json = await fetchAndNormalizeCatalog(url);
  await sql`INSERT INTO catalogs ${sql.insertValues({
      name: json.name,
      url: json._url,
      last_updated: json.lastUpdated || null,
      version: json.version,
      priority,
      json: JSON.stringify(json),
  })}`;

  const [catalog] = await sql<CatalogRow>`SELECT *
                                          FROM catalogs
                                          WHERE url = ${url}
                                          LIMIT 1`;
  return catalog;
}


export interface ListCatalogsOptions {
  updatePending?: boolean;
  url?: string;
}

export async function listCatalogsFromDb(
  options: ListCatalogsOptions = {},
): Promise<CatalogRow[]> {
  const rows = await sql<CatalogRow>`SELECT *
                                     FROM catalogs
                                     WHERE ${sql.and(
                                             true,
                                             options.url
                                                     ? sql`url =
                                                     ${options.url}`
                                                     : undefined,
                                             options.updatePending
                                                     ? sql`update_pending =
                                                     ${options.updatePending}`
                                                     : undefined,
                                     )}
  `;
  return rows.sort((a, b) => a.priority - b.priority);
}
