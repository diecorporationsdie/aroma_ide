import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toDisposable } from '../disposable';
import type { Disposable } from '../disposable';

/**
 * Поиск по проекту и наблюдение за файлами.
 *
 * Оба вызова живут в ядре, потому что в браузере их нет: обход 40 000 файлов
 * через IPC из JS был бы в десятки раз медленнее, а `fs.watch` недоступен
 * вовсе.
 */

export interface SearchOptions {
  readonly query: string;
  readonly roots: readonly string[];
  readonly regex?: boolean;
  readonly caseSensitive?: boolean;
  readonly wholeWord?: boolean;
  readonly include?: readonly string[];
  readonly exclude?: readonly string[];
  readonly maxResults?: number;
  readonly maxFileSize?: number;
  readonly maxFiles?: number;
  readonly contextLines?: number;
}

export interface SearchMatch {
  readonly path: string;
  /** 1-based. */
  readonly line: number;
  /** 1-based, в символах. */
  readonly column: number;
  readonly text: string;
  readonly matchStart: number;
  readonly matchEnd: number;
  readonly before: readonly string[];
  readonly after: readonly string[];
}

export interface SearchResult {
  readonly matches: readonly SearchMatch[];
  /** `true` — найдено больше, чем поместилось в лимит. */
  readonly truncated: boolean;
  readonly filesScanned: number;
  readonly filesMatched: number;
  readonly elapsedMs: number;
  readonly errors: readonly string[];
}

export interface FsChange {
  readonly kind: 'created' | 'modified' | 'removed' | 'accessed' | 'any' | 'other';
  readonly path: string;
}

export interface FsChangedPayload {
  readonly id: string;
  readonly changes: readonly FsChange[];
}

export interface WatchOptions {
  readonly recursive?: boolean;
  readonly debounceMs?: number;
}

export interface WatchInfo {
  readonly id: string;
  readonly path: string;
  readonly recursive: boolean;
}

export interface FsToolsService {
  search(options: SearchOptions): Promise<SearchResult>;
  /** Подписка на изменения каталога. Возвращает `Disposable` для отписки. */
  watch(path: string, listener: (changes: readonly FsChange[]) => void, options?: WatchOptions): Promise<Disposable>;
  watchList(): Promise<WatchInfo[]>;
}

export class TauriFsToolsService implements FsToolsService {
  private counter = 0;

  search(options: SearchOptions): Promise<SearchResult> {
    return invoke<SearchResult>('fs_search', {
      options: {
        query: options.query,
        roots: [...options.roots],
        regex: options.regex ?? false,
        caseSensitive: options.caseSensitive ?? false,
        wholeWord: options.wholeWord ?? false,
        include: options.include ? [...options.include] : [],
        exclude: options.exclude ? [...options.exclude] : [],
        maxResults: options.maxResults,
        maxFileSize: options.maxFileSize,
        maxFiles: options.maxFiles,
        contextLines: options.contextLines,
      },
    });
  }

  async watch(
    path: string,
    listener: (changes: readonly FsChange[]) => void,
    options: WatchOptions = {},
  ): Promise<Disposable> {
    const id = `watch-${++this.counter}-${Date.now()}`;
    await invoke<WatchInfo>('fs_watch', { id, path, options });

    const pending = listen<FsChangedPayload>('fs:changed', (e) => {
      if (e.payload.id === id) listener(e.payload.changes);
    });

    return toDisposable(() => {
      void invoke('fs_unwatch', { id }).catch(() => {});
      void pending.then((unlisten) => unlisten()).catch(() => {});
    });
  }

  watchList(): Promise<WatchInfo[]> {
    return invoke<WatchInfo[]>('fs_watch_list');
  }
}
