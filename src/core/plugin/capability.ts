import type { Disposable } from '../disposable';

export interface CapabilityDescriptor {
  readonly pluginId: string;
  readonly capabilityId: string;
  readonly version: string;
  readonly description?: string;
}

export interface CapabilityHandle<T = unknown> extends Disposable {
  readonly pluginId: string;
  readonly capabilityId: string;
  readonly version: string;
  readonly api: T;
  readonly alive: boolean;
  onRemoved(listener: () => void): Disposable;
}

export type CapabilityMutator<T = unknown, R = unknown> = (
  api: T,
) => R | Disposable | void | Promise<R | Disposable | void>;