import * as osd from "./osd";
import { isOnline, register } from "@/hooks";

export function registerHandlers() {
  osd.registerHandlers();

  register("net.isOnline", async () => {
    return isOnline();
  });
}
