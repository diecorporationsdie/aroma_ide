import { DisposableBase, toDisposable } from '../disposable';
import type { Disposable } from '../disposable';

export class PluginRegistry extends DisposableBase {
  private readonly active = new Map<string, () => void>();

  register(id: string, dispose: () => void): Disposable {
    if (this.active.has(id)) {
      throw new Error(`Plugin already active: ${id}`);
    }
    this.active.set(id, dispose);
    return toDisposable(() => this.unregister(id));
  }

  unregister(id: string): void {
    const dispose = this.active.get(id);
    if (!dispose) return;
    this.active.delete(id);
    try {
      dispose();
    } catch (e) {
      console.error(`[plugin:${id}] unregister failed`, e);
    }
  }

  has(id: string): boolean {
    return this.active.has(id);
  }

  list(): string[] {
    return [...this.active.keys()];
  }

  dispose(): void {
    for (const id of [...this.active.keys()]) {
      this.unregister(id);
    }
    super.dispose();
  }
}
