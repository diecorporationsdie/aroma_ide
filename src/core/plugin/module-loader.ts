import type { PluginModule } from './api';
import type { PluginSourceEntry } from './source';

export interface PluginModuleLoader {
  load(entry: PluginSourceEntry): Promise<PluginModule>;
}

/**
 * Loads plugin entry as an ES module via a blob URL.
 * Limitation: single-file plugins only (relative imports are not resolvable).
 */
export class BlobModuleLoader implements PluginModuleLoader {
  async load(entry: PluginSourceEntry): Promise<PluginModule> {
    const blob = new Blob([entry.entryCode], { type: 'text/javascript' });
    const url = URL.createObjectURL(blob);
    try {
      const mod = (await import(/* @vite-ignore */ url)) as Partial<PluginModule>;
      return {
        activate:
          typeof mod.activate === 'function' ? mod.activate.bind(mod) : undefined,
        deactivate:
          typeof mod.deactivate === 'function' ? mod.deactivate.bind(mod) : undefined,
      };
    } finally {
      URL.revokeObjectURL(url);
    }
  }
}
