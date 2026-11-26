import { DisposableBase, toDisposable } from './disposable';
import type { Disposable } from './disposable';
import { isolateAsync } from './errors';

export type CommandHandler<A = unknown, R = unknown> = (args: A) => Promise<R> | R;

export interface CommandDescriptor {
  readonly id: string;
  readonly description?: string;
}

export class CommandRegistry extends DisposableBase {
  private readonly handlers = new Map<string, CommandHandler<unknown, unknown>>();
  private readonly descriptions = new Map<string, string>();

  register<A = void, R = unknown>(
    id: string,
    handler: CommandHandler<A, R>,
    description?: string,
  ): Disposable {
    if (this.handlers.has(id)) throw new Error(`Command already registered: ${id}`);
    this.handlers.set(id, handler as CommandHandler<unknown, unknown>);
    if (description) this.descriptions.set(id, description);

    return toDisposable(() => {
      this.handlers.delete(id);
      this.descriptions.delete(id);
    });
  }

  async execute<R = unknown>(id: string, args?: unknown): Promise<R | undefined> {
    const handler = this.handlers.get(id);
    if (!handler) {
      console.error(`[command] not found: ${id}`);
      return undefined;
    }
    return isolateAsync(
      async () => (await handler(args)) as R,
      (e) => console.error(`[command:${id}]`, e),
    );
  }

  has(id: string): boolean {
    return this.handlers.has(id);
  }

  list(): CommandDescriptor[] {
    return [...this.handlers.keys()].map((id) => ({
      id,
      description: this.descriptions.get(id),
    }));
  }

  dispose(): void {
    this.handlers.clear();
    this.descriptions.clear();
    super.dispose();
  }
}
