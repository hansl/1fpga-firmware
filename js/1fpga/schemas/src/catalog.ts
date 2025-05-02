import * as zod from "zod";
import { Base64, Hex, ShortName, Tag, UrlOrRelative, Version } from "./utils";

export const File = zod.object({
  url: UrlOrRelative,
  type: zod.string().describe("A mimetype for the file."),
  size: zod.number().describe("The size (in bytes) of the file."),
  sha256: zod
    .union([Hex.length(64), Base64.length(44)])
    .describe("The SHA256 hash of the file, in hexadecimal or base64."),
  signature: Hex.or(Base64)
    .describe("The signature of the file, in hexa or base64.")
    .optional(),
});
export type File = zod.TypeOf<typeof File>;

export const Release = zod.object({
  files: zod.array(File),
  version: Version,
  tags: zod.array(Tag).optional(),
});
export type Release = zod.TypeOf<typeof Release>;

export const Core = zod.object({
  name: zod.string().describe("Name of the core"),
  uniqueName: ShortName.describe("Unique short name of the core"),
  tags: zod.array(Tag).optional(),
  links: zod
    .object({
      homepage: zod.url().optional(),
      github: zod.url().optional(),
    })
    .catchall(zod.url()),
  description: zod.string().optional(),
  icon: UrlOrRelative.optional(),
  image: UrlOrRelative.optional(),
  releases: zod.array(Release),
  systems: ShortName.or(zod.array(ShortName)),
});
export type Core = zod.TypeOf<typeof Core>;

export const Cores = zod
  .object({
    _url: zod.never(),
  })
  .catchall(
    UrlOrRelative.or(
      zod.object({
        url: UrlOrRelative,
        version: Version,
      }),
    ),
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
