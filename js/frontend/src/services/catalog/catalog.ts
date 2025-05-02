import * as osd from "1fpga:osd";
import type * as schemas from "@1fpga/schemas";
import { fetchJsonAndValidate, ValidationError } from "@/utils";

/**
 * Understand whether an inner object is an url or versioned url or actual
 * type.
 */
async function fetchVersioned<InnerSchema extends schemas.ZodType>(
  baseUrl: string,
  value: unknown,
  schema: InnerSchema,
): Promise<schemas.TypeOf<typeof schema> & { _url?: string }> {
  if (typeof value == "string") {
    const url = new URL(value, baseUrl).toString();
    osd.show("Fetching cores...", "URL: " + url);
    return {
      ...(await fetchJsonAndValidate<InnerSchema>(url, schema, {
        allowRetry: true,
      })),
      _url: url,
    } as InnerSchema & { _url: string };
  } else if (
    typeof value === "object" &&
    value !== null &&
    typeof (value as any).url == "string"
  ) {
    const url = new URL((value as any).url, baseUrl).toString();
    osd.show("Fetching cores...", "URL: " + url);
    return {
      ...(await fetchJsonAndValidate<InnerSchema>(url, schema, {
        allowRetry: true,
      })),
      _url: url,
    } as InnerSchema & { _url: string };
  } else if (schema.safeParse(value).success) {
    return value as unknown as InnerSchema;
  } else {
    throw new ValidationError("Invalid value for schema.");
  }
}

export type NormalizedCore = schemas.catalog.Core & {
  _url?: string;
};

export type NormalizedCores = schemas.catalog.Cores &
  Record<string, NormalizedCore> & {
    _url?: string;
  };

/**
 * A normalized, resolved Catalog with potentially its inner properties
 * replaced with
 */
export type Catalog = schemas.catalog.Catalog & {
  _url: string;
};

/**
 * A normalized catalog with cores.
 */
export type NormalizedCatalog = Omit<Catalog, "cores"> & {
  cores?: NormalizedCores;
};

export async function fetchCoreOfCores(
  cores: schemas.catalog.Cores & { _url: string },
): Promise<NormalizedCores> {
  const base = cores._url;
  const schemas = await import("@1fpga/schemas");
  const result = {
    ...cores,
  } as NormalizedCores;

  for (const [name, core] of Object.entries(cores).filter(
    ([n]) => n !== "_url",
  )) {
    result[name] = await fetchVersioned(base, core, schemas.catalog.Core);
  }

  return result;
}

/**
 * Fetch all the cores of a catalog and returns a new catalog with
 * the new cores' metadata. Will never fetch the inner cores.
 * @param catalog The catalog to fill. Returns a new object.
 */
export async function fetchCores(
  catalog: Catalog,
): Promise<NormalizedCores | undefined> {
  if (catalog.cores === undefined) {
    return undefined;
  }
  const schemas = await import("@1fpga/schemas");

  const cores = await fetchVersioned(
    catalog._url,
    catalog.cores,
    schemas.catalog.Cores,
  );

  return await fetchCoreOfCores(
    cores as schemas.catalog.Cores & { _url: string },
  );
}

// export function fetchSystems(catalog: Catalog): Promise<NormalizedSystems>;

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
      ...(await fetchJsonAndValidate(url, schemas.catalog.Catalog, {
        allowRetry: false,
      })),
      _url: url,
    } as Catalog;

    catalog.cores = await fetchCores(catalog);
    // catalog.systems = await fetchSystems(catalog);

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
