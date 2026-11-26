import { invoke } from '@tauri-apps/api/core';
import type { PluginManifest } from '../types';
import type { PluginSource, PluginSourceEntry } from './source';

interface RawPluginEntry {
  id: string;
  manifest_json: string;
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
    return [...this.index.values()].map((e) => this.parseManifest(e));
  }

  async load(id: string): Promise<PluginSourceEntry> {
    if (!this.index.has(id)) await this.refresh();
    const raw = this.index.get(id);
    if (!raw) throw new Error(`Plugin not found: ${id}`);

    const manifest = this.parseManifest(raw);
    const entryName = manifest.entry ?? DEFAULT_ENTRY;
    const styleName = manifest.style ?? DEFAULT_STYLE;

    const entryCode = await invoke<string>('plugins_read', { pluginId: id, file: entryName });
    const styleCode = await this.tryRead(id, styleName);

    return { manifest, entryCode, styleCode };
  }

  private parseManifest(raw: RawPluginEntry): PluginManifest {
    const parsed = JSON.parse(raw.manifest_json) as PluginManifest;
    if (parsed.id !== raw.id) {
      throw new Error(`Plugin id mismatch: folder "${raw.id}" vs manifest "${parsed.id}"`);
    }
    return parsed;
  }

  private async tryRead(id: string, file: string): Promise<string | undefined> {
    try {
      return await invoke<string>('plugins_read', { pluginId: id, file });
    } catch {
      return undefined;
    }
  }
}
