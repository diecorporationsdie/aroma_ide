import { DisposableBase, toDisposable } from '../disposable';
import type { Disposable } from '../disposable';

export class ServiceRegistry extends DisposableBase {
  private readonly services = new Map<string, unknown>();

  register<T>(name: string, instance: T): Disposable {
    if (this.services.has(name)) {
      throw new Error(`Service already registered: ${name}`);
    }
    this.services.set(name, instance);
    return toDisposable(() => this.services.delete(name));
  }

  get<T = unknown>(name: string): T | undefined {
    return this.services.get(name) as T | undefined;
  }

  has(name: string): boolean {
    return this.services.has(name);
  }

  dispose(): void {
    this.services.clear();
    super.dispose();
  }
}
