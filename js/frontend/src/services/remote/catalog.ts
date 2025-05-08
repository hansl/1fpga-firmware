import type * as schemas from '@1fpga/schemas';

import * as osd from '1fpga:osd';

import { ValidationError, fetchJsonAndValidate } from '@/utils';

/**
 * A list of catalogs that are officially known.
 */
export enum WellKnownCatalogs {
  // The basic stable 1FPGA catalog.
  OneFpga = 'https://catalog.1fpga.cloud/',

  // The BETA 1FPGA catalog (not yet available).
  OneFpgaBeta = 'https://catalog.1fpga.cloud/beta.json',

  // Only exists in development mode.
  LocalTest = 'http://localhost:8080/catalog.json',
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
  if (typeof value == 'string') {
    const url = new URL(value, baseUrl).toString();
    osd.show('Fetching cores...', 'URL: ' + url);
    return {
      ...(await fetchJsonAndValidate<InnerSchema>(url, schema, {
        allowRetry: true,
      })),
      _url: url,
    } as Normalized<InnerSchema>;
  } else if (typeof value === 'object' && value !== null && typeof (value as any).url == 'string') {
    const url = new URL((value as any).url, baseUrl).toString();
    const version = `${(value as any).version}`;
    osd.show('Fetching cores...', 'URL: ' + url);
    return {
      ...(await fetchJsonAndValidate<InnerSchema>(url, schema, {
        allowRetry: true,
      })),
      _url: url,
      _version: version,
    } as Normalized<InnerSchema>;
  } else if (schema.safeParse(value).success) {
    return { _url: baseUrl, ...(value as any) } as InnerSchema;
  } else {
    throw new ValidationError('Invalid value for schema.');
  }
}

type Normalized<T> = T & {
  _url?: string;
  _version?: string;
};

export function denormalize<T extends { _url?: string; _version?: string }>(
  n: T,
): Omit<T, '_url' | '_version'> {
  const { _url, _version, ...d } = n;
  return d;
}

export type NormalizedGamesDb = Normalized<schemas.catalog.GamesDb>;

export type NormalizedSystem = Normalized<schemas.catalog.System> & {
  gamesDb?: NormalizedGamesDb;
};

export type NormalizedSystems = Normalized<Record<string, NormalizedSystem>>;

export type NormalizedCore = Normalized<schemas.catalog.Core>;

export type NormalizedCores = Normalized<Record<string, NormalizedCore>>;

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

  for (const [name, value] of Object.entries(denormalize(record))) {
    (result as any)[name] = await fetchVersioned(base, value, schema);
  }

  return result;
}

async function fetchInner<T extends schemas.ZodType>(
  baseUrl: string,
  record: unknown,
  schema: T,
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

export async function fetchAndNormalizeCatalog(url: string): Promise<NormalizedCatalog> {
  // Normalize the URL.
  url = new URL(url).toString();
  osd.show('Fetching catalog...', 'URL: ' + url);

  if (!url.startsWith('https://') && !url.startsWith('http://')) {
    url = 'https://' + url;
  }

  const schemas = await import('@1fpga/schemas');
  try {
    let catalog = {
      ...(await fetchJsonAndValidate<schemas.catalog.Catalog>(url, schemas.catalog.Catalog, {
        allowRetry: false,
      })),
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
    catalog.releases = await fetchInner(url, catalog.releases, schemas.catalog.Releases);

    // At this point all internals have been normalized.
    return catalog as NormalizedCatalog;
  } catch (e) {
    if (e instanceof ValidationError) {
      throw e;
    }

    console.error('Error fetching catalog:', (e as any)?.message || e);

    // If this is http, try with https.
    // If this doesn't end with `catalog.json`, try adding it.
    if (!url.endsWith('/catalog.json')) {
      return fetchAndNormalizeCatalog(new URL('catalog.json', url).toString());
    } else if (url.startsWith('http://')) {
      return fetchAndNormalizeCatalog(url.replace(/^http:\/\//, 'https://'));
    } else {
      throw e;
    }
  }
}
