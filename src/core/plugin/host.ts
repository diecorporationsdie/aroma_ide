import { DisposableBase, toDisposable } from '../disposable';
import type { Disposable } from '../disposable';
import { PluginError } from '../errors';
import type { PluginManifest } from '../types';
import { satisfies } from '../util/semver';
import type { PluginModule } from './api';
import type { CapabilityRegistry } from './capability-registry';
import { PluginContextImpl } from './context';
import type { PluginContextDeps } from './context';
import type { PluginModuleLoader } from './module-loader';
import { PluginRegistry } from './registry';
import type { PluginSource } from './source';

export interface PluginHostDeps {
  readonly source: PluginSource;
  readonly loader: PluginModuleLoader;
  readonly contextDeps: PluginContextDeps;
}

export type ActivationEvent =
  | { readonly type: 'start'; readonly manifest: PluginManifest }
  | { readonly type: 'ok'; readonly manifest: PluginManifest }
  | { readonly type: 'error'; readonly manifest: PluginManifest; readonly error: unknown }
  | {
      readonly type: 'skipped';
      readonly manifest: PluginManifest;
      readonly reasons: readonly string[];
    };

export type ActivationProgress = (event: ActivationEvent) => void;

interface ActivePlugin {
  readonly manifest: PluginManifest;
  readonly module: PluginModule;
  readonly context: PluginContextImpl;
  readonly disposables: Disposable[];
}

interface ActivationOrder {
  readonly ordered: PluginManifest[];
  readonly missing: Map<string, string[]>;
}

export class PluginHost extends DisposableBase {
  private readonly registry = this._store.add(new PluginRegistry());
  private readonly active = new Map<string, ActivePlugin>();

  constructor(private readonly deps: PluginHostDeps) {
    super();
  }

  get capabilities(): CapabilityRegistry {
    return this.deps.contextDeps.capabilities;
  }

  async activate(id: string): Promise<void> {
    if (this.active.has(id)) throw new PluginError(id, 'already active');

    // Кэш источника сбрасываем: за время между загрузками плагин могли
    // переустановить или отредактировать.
    const manifest = (await this.deps.source.list()).find((m) => m.id === id);
    if (!manifest) throw new PluginError(id, 'not found in source');

    this.assertRequirements(manifest);

    const entry = await this.deps.source.load(id);
    if (entry.manifest.trust === 'sandboxed') {
      throw new PluginError(id, 'sandboxed plugins are not supported yet');
    }

    const module = await this.deps.loader.load(entry);
    const context = new PluginContextImpl(entry.manifest, this.deps.contextDeps);
    const disposables: Disposable[] = [];

    if (entry.styleCode) disposables.push(this.injectStyle(id, entry.styleCode));
    for (const output of entry.manifest.outputs ?? []) {
      disposables.push(this.deps.contextDeps.io.declareOutput(id, output.id));
    }

    try {
      await module.activate?.(context);
    } catch (e) {
      for (const d of disposables) this.safeDispose(d);
      context.dispose();
      this.deps.contextDeps.io.dropPlugin(id);
      this.capabilities.dropPlugin(id);
      throw new PluginError(id, 'activate failed', e);
    }

    this.active.set(id, { manifest: entry.manifest, module, context, disposables });
    this.registry.register(id, () => void this.doDeactivate(id));
  }

  async deactivate(id: string): Promise<void> {
    if (!this.active.has(id)) return;
    await this.doDeactivate(id);
    this.registry.unregister(id);
    this.cascadeDependents(id);
  }

  /**
   * Перезагрузка одного плагина: выгрузить модуль и загрузить заново.
   *
   * Нужна для горячей перезагрузки при правке `index.js`. Зависимые плагины
   * не трогаем: их capabilities на время перезагрузки исчезают, но
   * `cascadeDependents` вызывается только при явном удалении.
   */
  async reload(id: string): Promise<void> {
    if (!this.active.has(id)) {
      await this.activate(id);
      return;
    }
    await this.doDeactivate(id);
    this.registry.unregister(id);
    await this.activate(id);
  }

  /** Перезагрузить несколько плагинов, сохранив порядок зависимостей. */
  async reloadMany(ids: readonly string[]): Promise<void> {
    const manifests = await this.deps.source.list();
    const { ordered } = this.orderByDependencies(manifests);
    const wanted = new Set(ids);
    for (const manifest of ordered) {
      if (!wanted.has(manifest.id)) continue;
      try {
        await this.reload(manifest.id);
      } catch (e) {
        console.error(`[plugin:${manifest.id}] reload failed`, e);
      }
    }
  }

  isActive(id: string): boolean {
    return this.active.has(id);
  }

  listActive(): string[] {
    return this.registry.list();
  }

  listAvailable(): Promise<PluginManifest[]> {
    return this.deps.source.list();
  }

  async activateAll(onProgress?: ActivationProgress): Promise<void> {
    const manifests = await this.deps.source.list();
    const { ordered, missing } = this.orderByDependencies(manifests);

    for (const [id, reasons] of missing) {
      const manifest = manifests.find((m) => m.id === id);
      if (manifest) onProgress?.({ type: 'skipped', manifest, reasons });
      console.warn(`[plugin:${id}] skipped activation: ${reasons.join('; ')}`);
    }

    for (const m of ordered) {
      onProgress?.({ type: 'start', manifest: m });
      try {
        await this.activate(m.id);
        onProgress?.({ type: 'ok', manifest: m });
      } catch (e) {
        onProgress?.({ type: 'error', manifest: m, error: e });
        console.error(`[plugin:${m.id}] activation failed`, e);
      }
    }
  }

  async shutdown(): Promise<void> {
    const ids = this.listActive();
    for (const id of [...ids].reverse()) await this.deactivate(id);
  }

  dispose(): void {
    for (const id of this.listActive()) void this.deactivate(id);
    super.dispose();
  }

  private assertRequirements(manifest: PluginManifest): void {
    const missing: string[] = [];
    for (const req of manifest.requires ?? []) {
      if (req.optional) continue;
      if (!this.capabilities.satisfiesRequirement(req.id, req.version)) {
        missing.push(`${req.id}${req.version ? `@${req.version}` : ''}`);
      }
    }
    if (missing.length > 0) {
      throw new PluginError(manifest.id, `missing required capabilities: ${missing.join(', ')}`);
    }
  }

  private orderByDependencies(manifests: PluginManifest[]): ActivationOrder {
    const byId = new Map(manifests.map((m) => [m.id, m]));
    const providers = new Map<string, string[]>();
    for (const m of manifests) {
      for (const p of m.provides ?? []) {
        const arr = providers.get(p.id) ?? [];
        arr.push(m.id);
        providers.set(p.id, arr);
      }
    }

    const visiting = new Set<string>();
    const visited = new Set<string>();
    const ordered: PluginManifest[] = [];
    const missing = new Map<string, string[]>();

    const visit = (id: string): void => {
      if (visited.has(id)) return;
      if (visiting.has(id)) {
        throw new Error(`Cyclic plugin dependency detected at "${id}"`);
      }
      const manifest = byId.get(id);
      if (!manifest) return;

      visiting.add(id);

      const unresolved: string[] = [];
      for (const req of manifest.requires ?? []) {
        if (req.optional) continue;
        const candidates = (providers.get(req.id) ?? []).filter((pid) => {
          const provider = byId.get(pid);
          const decl = provider?.provides?.find((p) => p.id === req.id);
          if (!decl) return false;
          return req.version ? satisfies(decl.version, req.version) : true;
        });
        if (candidates.length === 0) {
          unresolved.push(`${req.id}${req.version ? `@${req.version}` : ''}`);
          continue;
        }
        for (const pid of candidates) visit(pid);
      }

      visiting.delete(id);
      visited.add(id);

      if (unresolved.length > 0) {
        missing.set(id, unresolved);
        return;
      }
      ordered.push(manifest);
    };

    for (const m of manifests) {
      try {
        visit(m.id);
      } catch (e) {
        missing.set(m.id, [String(e)]);
      }
    }

    return { ordered, missing };
  }

  private cascadeDependents(removedId: string): void {
    const goneCapabilities = new Set<string>();
    const removed = this.active.get(removedId);
    if (removed) for (const p of removed.manifest.provides ?? []) goneCapabilities.add(p.id);

    const affected: string[] = [];
    for (const [id, plugin] of this.active) {
      if (id === removedId) continue;
      for (const req of plugin.manifest.requires ?? []) {
        if (req.optional) continue;
        if (!goneCapabilities.has(req.id)) continue;
        if (!this.capabilities.satisfiesRequirement(req.id, req.version)) {
          affected.push(id);
          break;
        }
      }
    }

    this.deps.contextDeps.events.emit('plugin:dependencyLost', {
      provider: removedId,
      dependents: affected,
    });

    for (const id of affected) {
      console.warn(`[plugin:${id}] deactivating: lost required capability from ${removedId}`);
      void this.deactivate(id);
    }
  }

  private async doDeactivate(id: string): Promise<void> {
    const plugin = this.active.get(id);
    if (!plugin) return;
    this.active.delete(id);

    try {
      await plugin.module.deactivate?.();
    } catch (e) {
      console.error(`[plugin:${id}] deactivate failed`, e);
    }

    for (const d of plugin.disposables) this.safeDispose(d);
    plugin.context.dispose();
    this.deps.contextDeps.io.dropPlugin(id);
    this.capabilities.dropPlugin(id);
  }

  private safeDispose(d: Disposable): void {
    try {
      d.dispose();
    } catch (e) {
      console.error('[plugin] dispose failed', e);
    }
  }

  private injectStyle(id: string, css: string): Disposable {
    const el = document.createElement('style');
    el.dataset.pluginId = id;
    el.textContent = css;
    document.head.appendChild(el);
    return toDisposable(() => el.remove());
  }
}