import { invoke } from '@tauri-apps/api/core';

export interface DirEntry {
  readonly name: string;
  readonly path: string;
  readonly isDir: boolean;
  readonly isSymlink: boolean;
}

export interface Stat {
  readonly isFile: boolean;
  readonly isDir: boolean;
  readonly isSymlink: boolean;
  readonly size: number;
  readonly mtime: number | null;
}

export interface FsService {
  readFile(path: string): Promise<string>;
  writeFile(path: string, contents: string): Promise<void>;
  readDir(path: string): Promise<DirEntry[]>;
  stat(path: string): Promise<Stat | null>;
  exists(path: string): Promise<boolean>;
  mkdir(path: string, recursive?: boolean): Promise<void>;
  remove(path: string): Promise<void>;
  rename(from: string, to: string): Promise<void>;
  homeDir(): Promise<string>;
}

export class TauriFsService implements FsService {
  readFile(path: string): Promise<string> {
    return invoke<string>('fs_read_file', { path });
  }

  writeFile(path: string, contents: string): Promise<void> {
    return invoke('fs_write_file', { path, contents });
  }

  readDir(path: string): Promise<DirEntry[]> {
    return invoke<DirEntry[]>('fs_read_dir', { path });
  }

  stat(path: string): Promise<Stat | null> {
    return invoke<Stat | null>('fs_stat', { path });
  }

  exists(path: string): Promise<boolean> {
    return invoke<boolean>('fs_exists', { path });
  }

  mkdir(path: string, recursive = false): Promise<void> {
    return invoke('fs_mkdir', { path, recursive });
  }

  remove(path: string): Promise<void> {
    return invoke('fs_remove', { path });
  }

  rename(from: string, to: string): Promise<void> {
    return invoke('fs_rename', { from, to });
  }

  homeDir(): Promise<string> {
    return invoke<string>('fs_home_dir');
  }
}