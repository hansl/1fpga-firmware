// Type definitions for the `1fpga:fs` module.

/**
 * File system functions.
 */
declare module '1fpga:fs' {
  export function writeFile(path: string, data: string): Promise<void>;
  export function writeFile(path: string, data: Uint8Array): Promise<void>;
  export function writeFile(path: string, data: ArrayBuffer): Promise<void>;

  export function readFile(path: string): Promise<ArrayBuffer>;

  export function readTextFile(path: string): Promise<string>;

  export function deleteFile(path: string): Promise<void>;

  export function readDir(path: string): Promise<string[]>;

  /** One entry of a directory listing (see {@link readDirEntries}). */
  export interface DirEntry {
    name: string;
    /** True when the entry is a directory. */
    dir: boolean;
    /** File size in bytes (0 for directories). */
    size: number;
  }

  /**
   * List a directory with per-entry type and size in one call —
   * avoids a follow-up `isDir`/`fileSize` per entry when scanning.
   */
  export function readDirEntries(path: string): Promise<DirEntry[]>;

  export function isFile(path: string): Promise<boolean>;

  /**
   * Create a directory.
   * @param path The path to the directory.
   * @param all Whether to create all directories in the path.
   */
  export function mkdir(path: string, all?: boolean): Promise<void>;

  /**
   * Delete a directory.
   * @param path
   * @param recursive
   */
  export function rmdir(path: string, recursive?: boolean): Promise<void>;

  export function isDir(path: string): Promise<boolean>;

  export function findAllFiles(
    root: string,
    options?: { extensions?: string[] },
  ): Promise<string[]>;

  /**
   * Get the SHA-256 hash of a file, in hexadecimal.
   * @param path The path to the file.
   */
  export function sha256(path: string): Promise<string>;

  /**
   * Get the SHA-256 hash of multiple files.
   * @param path The paths to the files.
   */
  export function sha256(path: string[]): Promise<string[]>;

  export function fileSize(path: string): Promise<number>;
  export function fileSize(path: string[]): Promise<number[]>;
}
