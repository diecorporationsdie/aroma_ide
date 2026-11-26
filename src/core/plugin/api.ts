import type { Disposable } from '../disposable';
import type { PluginManifest, Region, SlotOptions, ZTier } from '../types';
import type { CapabilityDescriptor, CapabilityHandle, CapabilityMutator } from './capability';
import type { InputHandler, OutputListener } from './io';
import type { PluginStatusResult } from './status';

export interface SlotHandle extends Disposable {
  readonly id: string;
  readonly region: Region;
  readonly element: HTMLElement;
  readonly tier: ZTier;
  readonly zIndex: number;
  create<K extends keyof HTMLElementTagNameMap>(tag: K): HTMLElementTagNameMap[K];
  addSlot(options?: SlotOptions): SlotHandle;
}

export interface PluginCommandsApi {
  register<A = void, R = unknown>(
    id: string,
    handler: (args: A) => Promise<R> | R,
    description?: string,
  ): Disposable;
  execute<R = unknown>(id: string, args?: unknown): Promise<R | undefined>;
  has(id: string): boolean;
}1

export interface PluginEventsApi {
  on<T>(event: string, listener: (payload: T) => void): Disposable;
  emit<T>(event: string, payload: T): void;
}

export interface PluginUiApi {
  slot(region: Region, options?: SlotOptions): Promise<SlotHandle>;
}

export interface PluginServicesApi {
  get<T = unknown>(name: string): Promise<T | undefined>;
}

export interface PluginStorageApi {
  get<T>(key: string, fallback?: T): Promise<T | undefined>;
  set<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<void>;
  keys(): Promise<string[]>;
}

export interface PluginIoApi {
  publish(outputId: string, value: unknown): Promise<PluginStatusResult>;
  onInput(inputId: string, handler: InputHandler): Promise<Disposable>;
  subscribe(pluginId: string, outputId: string, listener: OutputListener): Promise<Disposable>;
  inject(pluginId: string, inputId: string, value: unknown): Promise<PluginStatusResult>;
}

export interface PluginCapabilitiesApi {
  provide<T>(capabilityId: string, api: T): Promise<PluginStatusResult>;
  handle<T = unknown>(pluginId: string, capabilityId: string): Promise<CapabilityHandle<T> | null>;
  find<T = unknown>(capabilityId: string): Promise<CapabilityHandle<T> | null>;
  all<T = unknown>(capabilityId: string): Promise<CapabilityHandle<T>[]>;
  list(): Promise<CapabilityDescriptor[]>;
  inject<T = unknown, R = unknown>(
    pluginId: string,
    capabilityId: string,
    mutator: CapabilityMutator<T, R>,
  ): Promise<PluginStatusResult<R>>;
}

export interface PluginSubscriptions {
  add(disposable: Disposable): Disposable;
  readonly size: number;
}

export interface PluginContext {
  readonly manifest: Readonly<PluginManifest>;
  readonly commands: PluginCommandsApi;
  readonly events: PluginEventsApi;
  readonly ui: PluginUiApi;
  readonly services: PluginServicesApi;
  readonly storage: PluginStorageApi;
  readonly io: PluginIoApi;
  readonly capabilities: PluginCapabilitiesApi;
  readonly subscriptions: PluginSubscriptions;
}

export interface PluginModule {
  activate?(ctx: PluginContext): void | Promise<void>;
  deactivate?(): void | Promise<void>;
}
