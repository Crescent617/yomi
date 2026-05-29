export interface FileEntry {
  name: string;
  path: string;
  isDirectory: boolean;
  isFile: boolean;
  size?: number;
  modifiedAt?: Date;
}

export interface FileStat {
  isFile: boolean;
  isDirectory: boolean;
  size: number;
  modifiedAt: Date;
}

export interface FileSystemProvider {
  listDir(path: string): Promise<FileEntry[]>;
  readFile(path: string): Promise<string>;
  readFileBytes(path: string): Promise<Uint8Array>;
  writeFile(path: string, content: string): Promise<void>;
  stat(path: string): Promise<FileStat>;
  exists(path: string): Promise<boolean>;
}
