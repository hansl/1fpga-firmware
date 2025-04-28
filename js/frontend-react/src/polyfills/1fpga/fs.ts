export async function writeFile(
  path: string,
  data: string | Uint8Array | ArrayBuffer,
): Promise<void> {
  if (data instanceof ArrayBuffer) {
    data = new Uint8Array(data);
  } else if (typeof data === "string") {
    data = new TextEncoder().encode(data);
  }

  let bytes = Array.from(data, (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");

  const result = await fetch("/api/fs/write", {
    method: "POST",
    body: JSON.stringify({
      path,
      bytes,
    }),
  });

  if (!result.ok) {
    throw new Error(`Failed to writeFile: ${result.statusText}`);
  }
}

// export function readFile(path: string): Promise<ArrayBuffer>;
//
// export function readTextFile(path: string): Promise<string>;

export async function deleteFile(path: string): Promise<void> {
  const result = await fetch("/api/fs/rm", {
    method: "POST",
    body: JSON.stringify({ path }),
  });

  if (!result.ok) {
    throw new Error(`Failed to writeFile: ${result.statusText}`);
  }
}

// export function readDir(path: string): Promise<string[]>;
//
// export function isFile(path: string): Promise<boolean>;

export async function mkdir(path: string, all?: boolean): Promise<void> {
  const result = await fetch("/api/fs/mkdir", {
    method: "POST",
    body: JSON.stringify({ path, all }),
  });

  if (!result.ok) {
    throw new Error(`Failed to writeFile: ${result.statusText}`);
  }
}

export async function rmdir(path: string, recursive?: boolean): Promise<void> {
  const result = await fetch("/api/fs/rmdir", {
    method: "POST",
    body: JSON.stringify({ path, recursive }),
  });

  if (!result.ok) {
    throw new Error(`Failed to writeFile: ${result.statusText}`);
  }
}

export function isDir(path: string): Promise<boolean> {
  throw new Error("Not implemented");
}

export async function findAllFiles(
  root: string,
  options?: { extensions?: string[] },
): Promise<string[]> {
  throw new Error("Not implemented");
}

export function sha256(path: string): Promise<string>;
export function sha256(path: string[]): Promise<string[]>;
export function sha256(path: string | string[]): Promise<string | string[]> {
  throw new Error("Not implemented");
}

export function fileSize(path: string): Promise<number>;
export function fileSize(path: string[]): Promise<number[]>;
export function fileSize(path: string | string[]): Promise<number | number[]> {
  throw new Error("Not implemented");
}
