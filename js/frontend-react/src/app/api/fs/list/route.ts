import * as fs from "node:fs/promises";
import * as path from "node:path";
import { pathOf } from "@/utils/server/filesystem";

export const GET = () => {
  return new Response(null, { status: 403 });
};

// Define the POST request handler function
export const POST = async (req: Request) => {
  async function walk(root: string, dir: string, recursive: boolean) {
    const fileNames = await fs.readdir(dir);
    const files: [string, boolean, number][] = [];
    for (const name of fileNames) {
      const p = path.relative(root, path.join(dir, name));
      console.log(p, root, path.join(dir, name));
      const stat = await fs.stat(path.join(root, p));
      const isDir = stat.isDirectory();
      files.push([p, isDir, stat.size]);

      if (isDir && recursive) {
        files.push(...(await walk(root, p, recursive)));
      }
    }

    return files;
  }

  try {
    const { dir, recursive } = await req.json();
    const p = await pathOf(dir);
    console.log("d", dir, p);

    if (!(await fs.stat(p)).isDirectory()) {
      return new Response(`Dir ${dir} not a directory`, { status: 400 });
    }

    return Response.json(await walk(p, p, !!recursive));
  } catch (e) {
    return new Response(`${e}`, { status: 500 });
  }
};
