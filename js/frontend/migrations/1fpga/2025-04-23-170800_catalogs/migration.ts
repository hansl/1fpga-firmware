import * as osd from "1fpga:osd";
import { SqlTag } from "@sqltags/core";
import { oneLine } from "common-tags";

export async function post(
  _: SqlTag<unknown, unknown>,
  { initial }: { initial: boolean },
) {
  if (initial) {
    return;
  }

  await osd.alert(
    "Catalogs must be deleted",
    oneLine`
      The catalog format changed significantly with the latest release.
      We have to remove all catalogs from your local installation.
      You will need to add them again and reidentify your games.
      This should be now much quicker.
  `,
  );
}
