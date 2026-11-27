import { invoke } from '@tauri-apps/api/core';

/**
 * Типы соответствуют структурам Rust в `src-tauri/src/fs/mod.rs`.
 * Поля — camelCase, `mtime` в миллисекундах UNIX epoch.
 */

export interface DirEntry {
  readonly name: string;
  readonly path: string;
  readonly isDir: boolean;
  readonly isFile: boolean;
  readonly isSymlink: boolean;
  readonly isHidden: boolean;
  readonly size: number;
  readonly mtime: number | null;
  /** Устаревшее snake_case-поле: ядро отдаёт его для старых плагинов. */
  readonly is_dir?: boolean;
  /** Устаревшее snake_case-поле. */
  readonly is_symlink?: boolean;
}

export interface Stat {
  readonly isFile: boolean;
  readonly isDir: boolean;
  readonly isSymlink: boolean;
  readonly size: number;
  readonly mtime: number | null;
  readonly created: number | null;
  readonly readonly: boolean;
  /** Устаревшее snake_case-поле: ядро отдаёт его для старых плагинов. */
  readonly is_file?: boolean;
  /** Устаревшее snake_case-поле. */
  readonly is_dir?: boolean;
  /** Устаревшее snake_case-поле. */
  readonly is_symlink?: boolean;
}

export interface TextFile {
  readonly content: string;
  readonly size: number;
  readonly mtime: number | null;
  /** `utf-8` | `utf-8-bom` | `utf-8-lossy` — если файл пришлось чинить. */
  readonly encoding: string;
  /** `\n` | `\r\n` — доминирующий перевод строки. */
  readonly eol: string;
}

export interface BinaryFile {
  readonly base64: string;
  readonly size: number;
  readonly mtime: number | null;
}

export interface AppDirs {
  readonly home: string | null;
  readonly config: string | null;
  readonly data: string | null;
  readonly localData: string | null;
  readonly cache: string | null;
  readonly log: string | null;
  readonly resource: string | null;
  readonly exe: string | null;
  readonly cwd: string | null;
  readonly desktop: string | null;
  readonly documents: string | null;
  readonly temp: string;
}

export interface FsService {
  readFile(path: string): Promise<string>;
  /** Как `readFile`, но с кодировкой, EOL и mtime — редактору это нужно. */
  readText(path: string): Promise<TextFile>;
  readBinary(path: string): Promise<BinaryFile>;
  writeFile(path: string, contents: string): Promise<Stat>;
  writeBinary(path: string, base64: string): Promise<Stat>;
  appendFile(path: string, contents: string): Promise<void>;
  readDir(path: string): Promise<DirEntry[]>;
  stat(path: string): Promise<Stat | null>;
  exists(path: string): Promise<boolean>;
  mkdir(path: string, recursive?: boolean): Promise<void>;
  remove(path: string, recursive?: boolean): Promise<void>;
  rename(from: string, to: string, overwrite?: boolean): Promise<void>;
  copy(from: string, to: string, overwrite?: boolean): Promise<void>;
  canonicalize(path: string): Promise<string>;
  homeDir(): Promise<string>;
  appDirs(): Promise<AppDirs>;
  reveal(path: string): Promise<void>;
  open(path: string): Promise<void>;
}

export class TauriFsService implements FsService {
  readFile(path: string): Promise<string> {
    return invoke<string>('fs_read_file', { path });
  }

  readText(path: string): Promise<TextFile> {
    return invoke<TextFile>('fs_read_text', { path });
  }

  readBinary(path: string): Promise<BinaryFile> {
    return invoke<BinaryFile>('fs_read_binary', { path });
  }

  writeFile(path: string, contents: string): Promise<Stat> {
    return invoke<Stat>('fs_write_file', { path, contents });
  }

  writeBinary(path: string, base64: string): Promise<Stat> {
    return invoke<Stat>('fs_write_binary', { path, base64 });
  }

  appendFile(path: string, contents: string): Promise<void> {
    return invoke('fs_append_file', { path, contents });
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

  remove(path: string, recursive = true): Promise<void> {
    return invoke('fs_remove', { path, recursive });
  }

  rename(from: string, to: string, overwrite = true): Promise<void> {
    return invoke('fs_rename', { from, to, overwrite });
  }

  copy(from: string, to: string, overwrite = true): Promise<void> {
    return invoke('fs_copy', { from, to, overwrite });
  }

  canonicalize(path: string): Promise<string> {
    return invoke<string>('fs_canonicalize', { path });
  }

  homeDir(): Promise<string> {
    return invoke<string>('fs_home_dir');
  }

  appDirs(): Promise<AppDirs> {
    return invoke<AppDirs>('fs_app_dirs');
  }

  reveal(path: string): Promise<void> {
    return invoke('fs_reveal', { path });
  }

  open(path: string): Promise<void> {
    return invoke('fs_open', { path });
  }
}
