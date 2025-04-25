import * as db from "1fpga:db";
import { Catalog } from "@/services";


async function loadSystemDb(catalog: Catalog, system: string): Promise<db.Db> {
  throw 1;
}

export async function identifyBatch(path: string, catalogs: Catalog[]): Promise<void> {

}
