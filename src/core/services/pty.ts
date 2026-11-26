import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { toDisposable } from '../disposable';
import type { Disposable } from '../disposable';

export interface PtyOptions {
  readonly shell?: string;
  readonly args?: readonly string[];
  readonly cwd?: string;
  readonly env?: Readonly<Record<string, string>>;
  readonly cols?: number;
  readonly rows?: number;
}

export interface PtyProcess extends Disposable {
  readonly id: string;
  write(data: string): Promise<void>;
  resize(cols: number, rows: number): Promise<void>;
  kill(): Promise<void>;
  onData(listener: (chunk: string) => void): Disposable;
  onExit(listener: (code: number | null) => void): Disposable;
}

export interface PtyService {
  spawn(options?: PtyOptions): Promise<PtyProcess>;
}

interface PtyDataPayload {
  readonly id: string;
  readonly data: string;
}

interface PtyExitPayload {
  readonly id: string;
  readonly code: number | null;
}

class PtyProcessImpl implements PtyProcess {
  constructor(readonly id: string) {}

  write(data: string): Promise<void> {
    return invoke('pty_write', { id: this.id, data });
  }

  resize(cols: number, rows: number): Promise<void> {
    return invoke('pty_resize', { id: this.id, cols, rows });
  }

  kill(): Promise<void> {
    return invoke('pty_kill', { id: this.id });
  }

  onData(listener: (chunk: string) => void): Disposable {
    const pending: Promise<UnlistenFn> = listen<PtyDataPayload>('pty:data', (e) => {
      if (e.payload.id === this.id) listener(e.payload.data);
    });
    return toDisposable(() => {
      void pending.then((u) => u()).catch(() => {});
    });
  }

  onExit(listener: (code: number | null) => void): Disposable {
    const pending: Promise<UnlistenFn> = listen<PtyExitPayload>('pty:exit', (e) => {
      if (e.payload.id === this.id) listener(e.payload.code);
    });
    return toDisposable(() => {
      void pending.then((u) => u()).catch(() => {});
    });
  }

  dispose(): void {
    void this.kill();
  }
}

export class TauriPtyService implements PtyService {
  private counter = 0;

  async spawn(options: PtyOptions = {}): Promise<PtyProcess> {
    const id = `pty-${++this.counter}-${Date.now()}`;
    await invoke('pty_spawn', {
      id,
      shell: options.shell,
      args: options.args ? [...options.args] : undefined,
      cwd: options.cwd,
      env: options.env,
      cols: options.cols,
      rows: options.rows,
    });
    return new PtyProcessImpl(id);
  }
}
