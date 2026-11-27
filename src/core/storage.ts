import { invoke } from '@tauri-apps/api/core';

export interface StorageNamespace {
  get<T>(key: string, fallback?: T): Promise<T | undefined>;
  set<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<void>;
  keys(): Promise<string[]>;
}

export interface StorageBackend {
  read(namespace: string): Promise<Record<string, unknown>>;
  write(namespace: string, data: Record<string, unknown>): Promise<void>;
}

export class MemoryStorageBackend implements StorageBackend {
  private readonly data = new Map<string, Record<string, unknown>>();

  read(namespace: string): Promise<Record<string, unknown>> {
    return Promise.resolve({ ...(this.data.get(namespace) ?? {}) });
  }

  write(namespace: string, data: Record<string, unknown>): Promise<void> {
    this.data.set(namespace, { ...data });
    return Promise.resolve();
  }
}

/**
 * Кэш на namespace.
 *
 * Важно: `set`/`delete` пишут в кэш синхронно и отправляют точечный вызов
 * `storage_set`/`storage_delete`, а не перезаписывают весь объект. Иначе два
 * плагина, пишущих в соседние ключи одного namespace, затирали бы друг друга.
 */
export class Storage {
  private readonly cache = new Map<string, Record<string, unknown>>();
  private readonly pending = new Map<string, Promise<void>>();

  constructor(private readonly backend: StorageBackend) {}

  namespace(name: string): StorageNamespace {
    const load = async (): Promise<Record<string, unknown>> => {
      let bucket = this.cache.get(name);
      if (!bucket) {
        bucket = await this.backend.read(name);
        this.cache.set(name, bucket);
      }
      return bucket;
    };

    /** Записи одного namespace сериализуются: порядок не должен зависеть от сети. */
    const enqueue = (task: () => Promise<void>): Promise<void> => {
      const prev = this.pending.get(name) ?? Promise.resolve();
      const next = prev.then(task, task);
      this.pending.set(
        name,
        next.catch(() => {}),
      );
      return next;
    };

    const flush = (): Promise<void> =>
      enqueue(async () => {
        const bucket = await load();
        await this.backend.write(name, bucket);
      });

    return {
      async get<T>(key: string, fallback?: T): Promise<T | undefined> {
        const bucket = await load();
        return key in bucket ? (bucket[key] as T) : fallback;
      },
      async set<T>(key: string, value: T): Promise<void> {
        const bucket = await load();
        bucket[key] = value;
        await flush();
      },
      async delete(key: string): Promise<void> {
        const bucket = await load();
        delete bucket[key];
        await flush();
      },
      async keys(): Promise<string[]> {
        return Object.keys(await load());
      },
    };
  }
}

export class TauriStorageBackend implements StorageBackend {
  async read(namespace: string): Promise<Record<string, unknown>> {
    try {
      const data = await invoke<unknown>('storage_read', { namespace });
      if (data && typeof data === 'object' && !Array.isArray(data)) {
        return data as Record<string, unknown>;
      }
      return {};
    } catch (e) {
      // Битый файл бэкенд уже отложил в `.corrupt-*.json`; IDE должна
      // запуститься, а не упасть на настройках.
      console.warn('[storage] read failed', namespace, e);
      return {};
    }
  }

  async write(namespace: string, data: Record<string, unknown>): Promise<void> {
    try {
      await invoke('storage_write', { namespace, data, merge: false });
    } catch (e) {
      console.warn('[storage] write failed', namespace, e);
    }
  }
}
