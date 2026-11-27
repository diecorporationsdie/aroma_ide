import { DisposableBase, DisposableStore, toDisposable } from '../disposable';
import type { Disposable } from '../disposable';
import { satisfies } from '../util/semver';
import type {
  CapabilityDescriptor,
  CapabilityHandle,
  CapabilityMutator,
} from './capability';
import { PluginStatus } from './status';
import type { PluginStatusResult } from './status';

interface ProviderRecord {
  readonly pluginId: string;
  readonly capabilityId: string;
  readonly version: string;
  readonly description?: string;
  readonly api: unknown;
  readonly injects: DisposableStore;
  readonly removedListeners: Set<() => void>;
  alive: boolean;
}

function key(pluginId: string, capabilityId: string): string {
  return `${pluginId}:${capabilityId}`;
}

class CapabilityHandleImpl<T> implements CapabilityHandle<T> {
  private disposed = false;

  constructor(private readonly record: ProviderRecord) {}

  get pluginId(): string {
    return this.record.pluginId;
  }

  get capabilityId(): string {
    return this.record.capabilityId;
  }

  get version(): string {
    return this.record.version;
  }

  get api(): T {
    if (!this.alive) throw new Error(`Capability ${this.capabilityId} no longer available`);
    return this.record.api as T;
  }

  get alive(): boolean {
    return this.record.alive && !this.disposed;
  }

  onRemoved(listener: () => void): Disposable {
    this.record.removedListeners.add(listener);
    return toDisposable(() => this.record.removedListeners.delete(listener));
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.record.removedListeners.clear();
  }
}

export class CapabilityRegistry extends DisposableBase {
  private readonly providers = new Map<string, ProviderRecord>();
  private readonly byCapability = new Map<string, Set<string>>();

  provide(
    pluginId: string,
    capabilityId: string,
    version: string,
    api: unknown,
    description?: string,
  ): Disposable {
    const k = key(pluginId, capabilityId);
    if (this.providers.has(k)) {
      throw new Error(`Capability already provided: ${k}`);
    }

    const record: ProviderRecord = {
      pluginId,
      capabilityId,
      version,
      description,
      api,
      injects: new DisposableStore(),
      removedListeners: new Set(),
      alive: true,
    };
    this.providers.set(k, record);
    let set = this.byCapability.get(capabilityId);
    if (!set) {
      set = new Set();
      this.byCapability.set(capabilityId, set);
    }
    set.add(k);

    return toDisposable(() => this.remove(pluginId, capabilityId));
  }

  handle<T = unknown>(pluginId: string, capabilityId: string): CapabilityHandle<T> | null {
    const record = this.providers.get(key(pluginId, capabilityId));
    return record ? new CapabilityHandleImpl<T>(record) : null;
  }

  find<T = unknown>(capabilityId: string): CapabilityHandle<T> | null {
    const list = this.providersOf<T>(capabilityId);
    return list[0] ?? null;
  }

  providersOf<T = unknown>(capabilityId: string): CapabilityHandle<T>[] {
    const keys = this.byCapability.get(capabilityId);
    if (!keys) return [];
    const out: CapabilityHandle<T>[] = [];
    for (const k of keys) {
      const record = this.providers.get(k);
      if (record?.alive) out.push(new CapabilityHandleImpl<T>(record));
    }
    return out;
  }

  list(): CapabilityDescriptor[] {
    return [...this.providers.values()]
      .filter((r) => r.alive)
      .map((r) => ({
        pluginId: r.pluginId,
        capabilityId: r.capabilityId,
        version: r.version,
        description: r.description,
      }));
  }

  satisfiesRequirement(capabilityId: string, versionRange?: string): boolean {
    const keys = this.byCapability.get(capabilityId);
    if (!keys || keys.size === 0) return false;
    if (!versionRange) return true;
    for (const k of keys) {
      const record = this.providers.get(k);
      if (record?.alive && satisfies(record.version, versionRange)) return true;
    }
    return false;
  }

  async inject<T = unknown, R = unknown>(
    pluginId: string,
    capabilityId: string,
    mutator: CapabilityMutator<T, R>,
  ): Promise<PluginStatusResult<R>> {
    const record = this.providers.get(key(pluginId, capabilityId));
    if (!record || !record.alive) {
      return {
        code: PluginStatus.NOT_FOUND,
        message: `capability ${pluginId}:${capabilityId} not available`,
      };
    }
    try {
      const result = await mutator(record.api as T);
      let disposable: Disposable | undefined;
      if (result && typeof result === 'object' && 'dispose' in result && typeof result.dispose === 'function') {
        disposable = record.injects.add(result);
      }
      const value = result && typeof result === 'object' && 'dispose' in result
        ? undefined
        : (result as R | undefined);
      void disposable;
      return { code: PluginStatus.OK, value };
    } catch (e) {
      return { code: PluginStatus.INTERNAL_ERROR, message: String(e) };
    }
  }

  dropPlugin(pluginId: string): void {
    for (const k of [...this.providers.keys()]) {
      if (k.startsWith(`${pluginId}:`)) {
        const [pid, cid] = k.split(':');
        this.remove(pid, cid);
      }
    }
  }

  dispose(): void {
    for (const record of this.providers.values()) {
      this.markDead(record);
    }
    this.providers.clear();
    this.byCapability.clear();
    super.dispose();
  }

  private remove(pluginId: string, capabilityId: string): void {
    const k = key(pluginId, capabilityId);
    const record = this.providers.get(k);
    if (!record) return;
    this.providers.delete(k);
    this.byCapability.get(capabilityId)?.delete(k);
    if (this.byCapability.get(capabilityId)?.size === 0) {
      this.byCapability.delete(capabilityId);
    }
    this.markDead(record);
  }

  private markDead(record: ProviderRecord): void {
    if (!record.alive) return;
    record.alive = false;
    for (const l of [...record.removedListeners]) {
      try {
        l();
      } catch (e) {
        console.error('[capability] removed listener failed', e);
      }
    }
    record.removedListeners.clear();
    try {
      record.injects.dispose();
    } catch (e) {
      console.error('[capability] injects dispose failed', e);
    }
  }
}
