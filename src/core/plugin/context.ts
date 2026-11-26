import { DisposableStore } from '../disposable';
import type { Disposable } from '../disposable';
import type { CommandRegistry } from '../command-registry';
import type { EventBus } from '../event-bus';
import type { Storage, StorageNamespace } from '../storage';
import type { ServiceRegistry } from '../services';
import type { PluginManifest, Region, SlotOptions } from '../types';
import type {
  PluginCapabilitiesApi,
  PluginCommandsApi,
  PluginContext,
  PluginEventsApi,
  PluginIoApi,
  PluginServicesApi,
  PluginStorageApi,
  PluginSubscriptions,
  PluginUiApi,
  SlotHandle,
} from './api';
import type { CapabilityDescriptor, CapabilityHandle, CapabilityMutator } from './capability';
import type { CapabilityRegistry } from './capability-registry';
import type { InputHandler, OutputListener } from './io';
import type { IORegistry } from './io-registry';
import { PluginStatus } from './status';
import type { PluginStatusResult } from './status';
import type { UiHost } from './ui-host';

export interface PluginContextDeps {
  readonly commands: CommandRegistry;
  readonly events: EventBus;
  readonly storage: Storage;
  readonly services: ServiceRegistry;
  readonly ui: UiHost;
  readonly io: IORegistry;
  readonly capabilities: CapabilityRegistry;
}

class SubscriptionsImpl implements PluginSubscriptions {
  constructor(private readonly store: DisposableStore) {}

  add(disposable: Disposable): Disposable {
    return this.store.add(disposable);
  }

  get size(): number {
    return this.store.size;
  }
}

class CommandsApiImpl implements PluginCommandsApi {
  constructor(private readonly registry: CommandRegistry) {}

  register<A = void, R = unknown>(
    id: string,
    handler: (args: A) => Promise<R> | R,
    description?: string,
  ): Disposable {
    return this.registry.register(id, handler, description);
  }

  execute<R = unknown>(id: string, args?: unknown): Promise<R | undefined> {
    return this.registry.execute<R>(id, args);
  }

  has(id: string): boolean {
    return this.registry.has(id);
  }
}

class EventsApiImpl implements PluginEventsApi {
  constructor(private readonly bus: EventBus) {}

  on<T>(event: string, listener: (payload: T) => void): Disposable {
    return this.bus.on(event, listener);
  }

  emit<T>(event: string, payload: T): void {
    this.bus.emit(event, payload);
  }
}

class ServicesApiImpl implements PluginServicesApi {
  constructor(private readonly registry: ServiceRegistry) {}

  get<T = unknown>(name: string): Promise<T | undefined> {
    return Promise.resolve(this.registry.get<T>(name));
  }
}

class StorageApiImpl implements PluginStorageApi {
  constructor(private readonly ns: StorageNamespace) {}

  get<T>(key: string, fallback?: T): Promise<T | undefined> {
    return this.ns.get(key, fallback);
  }

  set<T>(key: string, value: T): Promise<void> {
    return this.ns.set(key, value);
  }

  delete(key: string): Promise<void> {
    return this.ns.delete(key);
  }

  keys(): Promise<string[]> {
    return this.ns.keys();
  }
}

class UiApiImpl implements PluginUiApi {
  constructor(
    private readonly host: UiHost,
    private readonly pluginId: string,
  ) {}

  slot(region: Region, options: SlotOptions = {}): Promise<SlotHandle> {
    return this.host.slot(this.pluginId, region, options);
  }
}

class IoApiImpl implements PluginIoApi {
  constructor(
    private readonly registry: IORegistry,
    private readonly manifest: Readonly<PluginManifest>,
  ) {}

  publish(outputId: string, value: unknown): Promise<PluginStatusResult> {
    if (!(this.manifest.outputs ?? []).some((o) => o.id === outputId)) {
      return Promise.resolve({
        code: PluginStatus.BAD_REQUEST,
        message: `output "${outputId}" not declared in manifest`,
      });
    }
    return Promise.resolve(this.registry.publish(this.manifest.id, outputId, value));
  }

  onInput(inputId: string, handler: InputHandler): Promise<Disposable> {
    if (!(this.manifest.inputs ?? []).some((i) => i.id === inputId)) {
      return Promise.reject(new Error(`input "${inputId}" not declared in manifest`));
    }
    return Promise.resolve(this.registry.declareInput(this.manifest.id, inputId, handler));
  }

  subscribe(pluginId: string, outputId: string, listener: OutputListener): Promise<Disposable> {
    try {
      return Promise.resolve(this.registry.subscribe(pluginId, outputId, listener));
    } catch (e) {
      return Promise.reject(e instanceof Error ? e : new Error(String(e)));
    }
  }

  inject(pluginId: string, inputId: string, value: unknown): Promise<PluginStatusResult> {
    return this.registry.inject(pluginId, inputId, value);
  }
}

class CapabilitiesApiImpl implements PluginCapabilitiesApi {
  constructor(
    private readonly registry: CapabilityRegistry,
    private readonly manifest: Readonly<PluginManifest>,
  ) {}

  provide<T>(capabilityId: string, api: T): Promise<PluginStatusResult> {
    const decl = (this.manifest.provides ?? []).find((p) => p.id === capabilityId);
    if (!decl) {
      return Promise.resolve({
        code: PluginStatus.BAD_REQUEST,
        message: `capability "${capabilityId}" not declared in manifest`,
      });
    }
    try {
      this.registry.provide(this.manifest.id, capabilityId, decl.version, api, decl.description);
      return Promise.resolve({ code: PluginStatus.OK });
    } catch (e) {
      return Promise.resolve({ code: PluginStatus.CONFLICT, message: String(e) });
    }
  }

  handle<T = unknown>(pluginId: string, capabilityId: string): Promise<CapabilityHandle<T> | null> {
    return Promise.resolve(this.registry.handle<T>(pluginId, capabilityId));
  }

  find<T = unknown>(capabilityId: string): Promise<CapabilityHandle<T> | null> {
    return Promise.resolve(this.registry.find<T>(capabilityId));
  }

  all<T = unknown>(capabilityId: string): Promise<CapabilityHandle<T>[]> {
    return Promise.resolve(this.registry.providersOf<T>(capabilityId));
  }

  list(): Promise<CapabilityDescriptor[]> {
    return Promise.resolve(this.registry.list());
  }

  inject<T = unknown, R = unknown>(
    pluginId: string,
    capabilityId: string,
    mutator: CapabilityMutator<T, R>,
  ): Promise<PluginStatusResult<R>> {
    return this.registry.inject<T, R>(pluginId, capabilityId, mutator);
  }
}

export class PluginContextImpl implements PluginContext {
  readonly manifest: Readonly<PluginManifest>;
  readonly commands: PluginCommandsApi;
  readonly events: PluginEventsApi;
  readonly ui: PluginUiApi;
  readonly services: PluginServicesApi;
  readonly storage: PluginStorageApi;
  readonly io: PluginIoApi;
  readonly capabilities: PluginCapabilitiesApi;
  readonly subscriptions: PluginSubscriptions;

  private readonly store = new DisposableStore();

  constructor(manifest: PluginManifest, deps: PluginContextDeps) {
    this.manifest = Object.freeze({ ...manifest });
    this.commands = new CommandsApiImpl(deps.commands);
    this.events = new EventsApiImpl(deps.events);
    this.ui = new UiApiImpl(deps.ui, manifest.id);
    this.services = new ServicesApiImpl(deps.services);
    this.storage = new StorageApiImpl(deps.storage.namespace(`plugin.${manifest.id}`));
    this.io = new IoApiImpl(deps.io, this.manifest);
    this.capabilities = new CapabilitiesApiImpl(deps.capabilities, this.manifest);
    this.subscriptions = new SubscriptionsImpl(this.store);
  }

  dispose(): void {
    this.store.dispose();
  }
}
