import * as zod from "zod";
import {
  File,
  Links,
  ShortName,
  Tag,
  UrlOrRelative,
  UrlVersionedType,
  Version,
} from "./utils";

export const GamesDb = File.extend({
  version: Version.optional(),
  links: Links.optional(),
});

export const System = zod.object({
  name: zod.string(),
  uniqueName: ShortName,
  description: zod.string(),
  icon: UrlOrRelative.optional(),
  image: UrlOrRelative.optional(),
  gamesDb: UrlVersionedType(GamesDb).optional(),
  cores: zod.object().optional(),
  tags: zod.array(Tag).optional(),
  links: Links.optional(),
});
export type System = zod.TypeOf<typeof System>;

export const Systems = zod.object({}).catchall(UrlVersionedType(System));
export type Systems = zod.TypeOf<typeof Systems>;

export const Release = zod.object({
  files: zod.array(File),
  version: Version.optional(),
  tags: zod.array(Tag).optional(),
});
export type Release = zod.TypeOf<typeof Release>;

export const Releases = zod
  .object({})
  .catchall(UrlVersionedType(zod.array(Release)));
export type Releases = zod.TypeOf<typeof Release>;

export const Core = zod.object({
  name: zod.string().describe("Name of the core"),
  uniqueName: ShortName.describe("Unique short name of the core"),
  tags: zod.array(Tag).optional(),
  links: Links.optional(),
  description: zod.string().optional(),
  icon: UrlOrRelative.optional(),
  image: UrlOrRelative.optional(),
  releases: zod.array(Release),
  systems: ShortName.or(zod.array(ShortName)),
});
export type Core = zod.TypeOf<typeof Core>;

export const Cores = zod
  .object({})
  .catchall(UrlVersionedType(Core))
  .describe("A list of all cores and their definition files.");

export type Cores = zod.TypeOf<typeof Cores>;

export const Catalog = zod
  .object({
    name: zod.string().min(3).max(64).describe("Name of the catalog."),
    cores: UrlVersionedType(Cores)
      .describe("List of cores in this catalog.")
      .optional(),
    systems: UrlVersionedType(Systems)
      .describe("List of systems in this catalog.")
      .optional(),
    releases: UrlVersionedType(Releases)
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
