import * as zod from "zod";
import { UrlOrRelative, Version } from "./utils";

export const Cores = zod
  .object({
    _url: zod.never(),
  })
  .catchall(
    zod.union([
      UrlOrRelative,
      zod.object({
        url: UrlOrRelative,
        version: Version,
      }),
    ]),
  )
  .describe("A list of all cores and their definition files.");

export type Cores = zod.TypeOf<typeof Cores>;

export const Catalog = zod
  .object({
    name: zod.string().min(3).max(64).describe("Name of the catalog."),
    cores: zod
      .union([
        Cores,
        zod.string(),
        zod.object({
          url: UrlOrRelative,
          version: Version,
        }),
      ])
      .describe("List of cores in this catalog.")
      .optional(),
    systems: zod
      .union([
        UrlOrRelative,
        zod.object({
          url: UrlOrRelative,
          version: Version,
        }),
      ])
      .describe("List of systems in this catalog.")
      .optional(),
    releases: zod
      .union([
        UrlOrRelative,
        zod.object({
          url: UrlOrRelative,
          version: Version,
        }),
      ])
      .describe("List of releases in this catalog.")
      .optional(),
    lastUpdated: zod.iso
      .datetime()
      .optional()
      .describe("Date of the last update to this catalog."),
    version: Version,
  })
  .describe("Definition of a catalog.");

export type Catalog = zod.TypeOf<typeof Catalog>;
