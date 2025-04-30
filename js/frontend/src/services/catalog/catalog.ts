import * as osd from "1fpga:osd";
import type * as schemas from "@1fpga/schemas";
import { fetchJsonAndValidate, ValidationError } from "@/utils";

export type Cores = schemas.catalog.Cores & {
  _url: string;
};

/**
 * A normalized, resolved Catalog with potentially its inner properties
 * replaced with
 */
export type Catalog = schemas.catalog.Catalog & {
  _url: string;
};

/**
 * Fetch all the cores of a catalog and returns a new catalog with
 * the new cores' metadata. Will never fetch the inner cores.
 * @param catalog The catalog to fill. Returns a new object.
 */
export async function fetchCores(catalog: Catalog): Promise<Catalog> {
  if (catalog.cores === undefined) {
    return catalog;
  }
  const schemas = await import("@1fpga/schemas");

  let cores = catalog.cores;
  if (typeof cores === "string") {
    const coresUrl = new URL(cores, catalog._url).toString();
    cores = {
      ...(await fetchJsonAndValidate<schemas.catalog.Cores>(
        coresUrl,
        schemas.catalog.Cores,
        { allowRetry: true },
      )),
      _url: coresUrl,
    } as Cores;
  } else if ("url" in cores && "version" in cores) {
    const coresUrl = new URL(cores.url, catalog._url).toString();
    cores = {
      ...(await fetchJsonAndValidate<schemas.catalog.Cores>(
        coresUrl,
        schemas.catalog.Cores,
        { allowRetry: true },
      )),
      _url: coresUrl,
    } as Cores;
  } else {
    // Cores already in.
    return catalog;
  }

  return {
    ...catalog,
    cores,
  };
}

export interface FetchCatalogOptions {
  /**
   * Also fetch and normalize systems.
   */
  systems?: boolean;
  cores?: boolean;
}

export async function fetchCatalog(
  url: string,
  options: FetchCatalogOptions = {},
) {
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
        { allowRetry: false },
      )),
      _url: url,
    } as Catalog;

    if (options.cores) {
      catalog = await fetchCores(catalog);
    }

    return catalog;
  } catch (e) {
    if (e instanceof ValidationError) {
      throw e;
    }

    console.error("Error fetching catalog:", (e as any)?.message || e);

    // If this is http, try with https.
    // If this doesn't end with `catalog.json`, try adding it.
    if (!url.endsWith("/catalog.json")) {
      return fetchCatalog(new URL("catalog.json", url).toString(), options);
    } else if (url.startsWith("http://")) {
      return fetchCatalog(url.replace(/^http:\/\//, "https://"), options);
    } else {
      throw e;
    }
  }
}
