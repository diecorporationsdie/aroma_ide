import { invoke } from '@tauri-apps/api/core';
import type { PluginManifest } from '../types';
import type { PluginSource, PluginSourceEntry } from './source';

/**
 * Запись плагина из бэкенда. Поля — camelCase, как в `src-tauri/src/plugins.rs`.
 */
interface RawPluginEntry {
  readonly id: string;
  readonly manifestJson: string;
  /** Корень, из которого взят плагин: `bundled`, `user`, `dev`, … */
  readonly root: string;
  readonly source: string;
  /** `false`, если манифест не прошёл проверку в ядре. */
  readonly valid: boolean;
  readonly error: string | null;
  readonly name: string | null;
  readonly version: string | null;
  readonly entry: string | null;
  readonly style: string | null;
  readonly mtime: number | null;
}

export interface PluginDiagnostic {
  readonly id: string;
  readonly path: string;
  readonly error: string;
}

const DEFAULT_ENTRY = 'index.js';
const DEFAULT_STYLE = 'style.css';

export class FilesystemPluginSource implements PluginSource {
  private index = new Map<string, RawPluginEntry>();

  async refresh(): Promise<void> {
    const raw = await invoke<RawPluginEntry[]>('plugins_scan');
    const next = new Map<string, RawPluginEntry>();
    for (const e of raw) next.set(e.id, e);
    this.index = next;
  }

  async list(): Promise<PluginManifest[]> {
    await this.refresh();
    const out: PluginManifest[] = [];
    for (const entry of this.index.values()) {
      // Плагин с невалидным манифестом не должен валить загрузку остальных:
      // его причина видна в plugins_diagnostics.
      if (!entry.valid) continue;
      try {
        out.push(this.parseManifest(entry));
      } catch (e) {
        console.error(`[plugins] ${entry.id}: ${String(e)}`);
      }
    }
    return out;
  }

  async load(id: string): Promise<PluginSourceEntry> {
    if (!this.index.has(id)) await this.refresh();
    const raw = this.index.get(id);
    if (!raw) throw new Error(`Plugin not found: ${id}`);
    if (!raw.valid) throw new Error(`Plugin "${id}" is invalid: ${raw.error ?? 'unknown reason'}`);

    const manifest = this.parseManifest(raw);
    const entryName = raw.entry ?? manifest.entry ?? DEFAULT_ENTRY;
    const styleName = raw.style ?? manifest.style ?? DEFAULT_STYLE;

    // Корень передаётся явно: иначе манифест и код могли прийти из разных
    // папок (плагин есть и в `dev`, и в `user`).
    const entryCode = await invoke<string>('plugins_read', {
      pluginId: id,
      file: entryName,
      root: raw.root,
    });
    const styleCode = await this.tryRead(id, styleName, raw.root);

    return { manifest, entryCode, styleCode };
  }

  /** Причины, по которым плагины не загрузились. */
  async diagnostics(): Promise<PluginDiagnostic[]> {
    return invoke<PluginDiagnostic[]>('plugins_diagnostics');
  }

  /** Сбросить кэш ядра: нужно после установки или удаления плагина. */
  async rescan(): Promise<void> {
    await invoke('plugins_rescan');
    await this.refresh();
  }

  private parseManifest(raw: RawPluginEntry): PluginManifest {
    const parsed = JSON.parse(raw.manifestJson) as PluginManifest;
    if (parsed.id !== raw.id) {
      throw new Error(`Plugin id mismatch: folder "${raw.id}" vs manifest "${parsed.id}"`);
    }
    return parsed;
  }

  private async tryRead(id: string, file: string, root: string): Promise<string | undefined> {
    try {
      return await invoke<string>('plugins_read', { pluginId: id, file, root });
    } catch {
      // style.css опционален — его отсутствие не ошибка.
      return undefined;
    }
  }
}
