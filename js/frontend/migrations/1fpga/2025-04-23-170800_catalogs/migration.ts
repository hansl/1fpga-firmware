import * as osd from "1fpga:osd";
import { resetAll } from "@/utils";
import { SqlTag } from "@sqltags/core";
import { oneLine } from "common-tags";

export async function pre(
  sql: SqlTag<unknown, unknown>,
  { initial }: { initial: boolean },
): Promise<void> {}

export async function post(
  _: SqlTag<unknown, unknown>,
  { initial }: { context: unknown; initial: boolean },
) {
  console.log(initial);
  if (initial) {
    return;
  }
  await osd.alert(
    "Catalog Must be deleted",
    oneLine`
      The catalog format changed significantly with the latest release.
      We have to remove all catalogs from your local installation, and you will
      restart the installation process. This should be now much quicker.
  `,
  );

  await resetAll();
}
