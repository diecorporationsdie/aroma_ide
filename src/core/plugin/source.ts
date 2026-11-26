import type { PluginManifest } from '../types';

export interface PluginSourceEntry {
  readonly manifest: PluginManifest;
  readonly entryCode: string;
  readonly styleCode?: string;
}

export interface PluginSource {
  list(): Promise<PluginManifest[]>;
  load(id: string): Promise<PluginSourceEntry>;
}

export class InMemoryPluginSource implements PluginSource {
  private readonly entries = new Map<string, PluginSourceEntry>();

  register(entry: PluginSourceEntry): void {
    this.entries.set(entry.manifest.id, entry);
  }

  list(): Promise<PluginManifest[]> {
    return Promise.resolve([...this.entries.values()].map((e) => e.manifest));
  }

  load(id: string): Promise<PluginSourceEntry> {
    const entry = this.entries.get(id);
    if (!entry) {
      return Promise.reject(new Error(`Plugin not found: ${id}`));
    }
    return Promise.resolve(entry);
  }
}
