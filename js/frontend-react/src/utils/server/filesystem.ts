"use server";

import path from "node:path";
import fs from "node:fs/promises";

export async function pathOf(p: string): Promise<string> {
  p = path.isAbsolute(p) ? path.join("./.next/fs", p) : path.join("./.next/fs/media/fat", p);
  try {
    return await fs.realpath(p);
  } catch (err) {
    return p;
  }
}
