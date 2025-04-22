import * as db from "1fpga:db";
import * as net from "1fpga:net";
import osd from "1fpga:osd";

export async function fetchDbAndValidate<T>(
  name: string,
  url: string,
  validate?: (db: db.Db) => boolean | Promise<boolean>,
  options?: {
    allowRetry?: boolean;
  },
): Promise<db.Db> {
  while (true) {
    try {
      const p = await net.download(url, `/media/fat/1fpga/catalogs/${name.replace(/[^a-zA-Z0-9._@-]/g, "_")}`);

      let database = await db.loadPath(p);
      if (validate) {
        if (await validate(database)) {
          return database;
        } else {
          throw new Error(`Database did not validate.`);
        }
      }
    } catch (e) {
      if (!(options?.allowRetry ?? true)) {
        console.warn(`Error fetching JSON: ${e}`);
        throw e;
      }

      let message = (e as any)?.message ?? `${e}`;
      if (message.toString() == "[object Object]" || !message) {
        message = JSON.stringify(e);
      }

      const choice = await osd.alert({
        title: "Error fetching JSON",
        message: `URL: ${url}\n\n${(e as any)?.message ?? JSON.stringify(e)}\n`,
        choices: ["Retry fetching", "Cancel"],
      });

      if (choice === 1) {
        throw e;
      }
    }
  }
}
