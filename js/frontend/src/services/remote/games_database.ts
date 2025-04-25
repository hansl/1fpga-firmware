import * as osd from "1fpga:osd";
import * as db from "1fpga:db";
import { RemoteSystem } from "./catalog";
import { fetchDbAndValidate } from "@/utils/fetch_db";

/**
 * The Game identification database downloaded from a catalog.
 */
export class RemoteGamesDb {
  public static async fetch(url: string, system: RemoteSystem) {
    const u = new URL(url, system.url).toString();

    osd.show(
      "Fetching games database...",
      `Catalog "${system.catalog.name}"\nSystem "${system.name}"\nURL: ${u}`,
    );

    // Dynamic loading to allow for code splitting.
    const db = await fetchDbAndValidate(
      u,
      undefined,
      {
        path: `/media/fat/1fpga/catalogs/${system.catalog.name}/${system.name}`
      }
    );
    console.log('done');

    return new RemoteGamesDb(u, db, system);
  }

  private constructor(
    public readonly url: string,
    private readonly db: db.Db,
    public readonly system: RemoteSystem,
  ) {
  }
}
