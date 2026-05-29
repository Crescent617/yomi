import {
  readDir,
  readTextFile,
  readFile,
  writeTextFile,
  stat,
  exists,
} from "@tauri-apps/plugin-fs";
import type { FileSystemProvider, FileEntry, FileStat } from "./provider";

export class LocalFSProvider implements FileSystemProvider {
  async listDir(path: string): Promise<FileEntry[]> {
    const entries = await readDir(path);
    return entries.map((e) => ({
      name: e.name,
      path: `${path}/${e.name}`,
      isDirectory: e.isDirectory,
      isFile: e.isFile,
    }));
  }

  async readFile(path: string): Promise<string> {
    return readTextFile(path);
  }

  async readFileBytes(path: string): Promise<Uint8Array> {
    return readFile(path);
  }

  async writeFile(path: string, content: string): Promise<void> {
    return writeTextFile(path, content);
  }

  async stat(path: string): Promise<FileStat> {
    const info = await stat(path);
    return {
      isFile: info.isFile,
      isDirectory: info.isDirectory,
      size: info.size,
      modifiedAt: new Date(info.mtime ?? 0),
    };
  }

  async exists(path: string): Promise<boolean> {
    return exists(path);
  }
}
