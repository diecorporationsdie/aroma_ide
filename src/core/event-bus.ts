import { DisposableBase, toDisposable } from './disposable';
import type { Disposable } from './disposable';
import { isolate } from './errors';

export type Listener<T> = (payload: T) => void;

export class EventBus extends DisposableBase {
  private readonly listeners = new Map<string, Set<Listener<unknown>>>();

  on<T>(event: string, listener: Listener<T>): Disposable {
    let set = this.listeners.get(event);
    if (!set) {
      set = new Set();
      this.listeners.set(event, set);
    }
    set.add(listener as Listener<unknown>);

    return toDisposable(() => {
      set.delete(listener as Listener<unknown>);
      if (set.size === 0) this.listeners.delete(event);
    });
  }

  emit<T>(event: string, payload: T): void {
    const set = this.listeners.get(event);
    if (!set) return;
    for (const listener of [...set]) {
      isolate(
        () => listener(payload as unknown),
        (e) => console.error(`[event:${event}]`, e),
      );
    }
  }

  dispose(): void {
    this.listeners.clear();
    super.dispose();
  }
}
