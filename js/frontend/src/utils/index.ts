import * as fs from "1fpga:fs";
import * as osd from "1fpga:osd";
import * as system from "1fpga:system";
import { closeAllDb, resetDb } from "@/utils/sql";

export * from "./fetch_json";
export * from "./sql";
export * from "./versions";

/**
 * Reset the database and
 */
export async function resetAll(): Promise<never> {
  await resetDb();
  await closeAllDb();

  await fs.rmdir("/media/fat/1fpga");
  await osd.alert("The system will shutdown now.")

  while (true) {
    try {
      return system.shutdown();
    } catch (e) {
      // Nothing to do here, really.
    }
  }
}
