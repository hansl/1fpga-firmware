import * as fs from "node:fs/promises";
import * as path from "node:path";
import { pathOf } from "@/utils/server/filesystem";

export const GET = () => {
  return new Response(null, { status: 403 });
};

// Define the POST request handler function
export const POST = async (req: Request) => {
  try {
    const { dir } = await req.json();
    const p = await pathOf(dir);

    if (!(await fs.stat(p)).isDirectory()) {
      return new Response(`Dir ${dir} not a directory`, { status: 400 });
    }

    const fileNames = await fs.readdir(p);
    const files: [string, boolean, number][] = [];
    for (const name of fileNames) {
      const stat = await fs.stat(path.join(p, name));
      files.push([name, stat.isDirectory(), stat.size]);
    }

    return Response.json(files);
  } catch (e) {
    return new Response(`${e}`, { status: 500 });
  }
};
