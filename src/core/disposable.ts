export interface Disposable {
  dispose(): void;
}

export class DisposableStore implements Disposable {
  private readonly items = new Set<Disposable>();
  private disposed = false;

  get isDisposed(): boolean {
    return this.disposed;
  }

  get size(): number {
    return this.items.size;
  }

  add<T extends Disposable>(item: T): T {
    if (this.disposed) {
      item.dispose();
      throw new Error('DisposableStore is already disposed');
    }
    this.items.add(item);
    return item;
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    for (const item of this.items) {
      try {
        item.dispose();
      } catch (e) {
        console.error('[disposable] dispose failed', e);
      }
    }
    this.items.clear();
  }
}

export function toDisposable(fn: () => void): Disposable {
  let done = false;
  return {
    dispose() {
      if (done) return;
      done = true;
      fn();
    },
  };
}

export abstract class DisposableBase implements Disposable {
  protected readonly _store = new DisposableStore();

  dispose(): void {
    this._store.dispose();
  }
}