import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toDisposable } from '../disposable';
import type { Disposable } from '../disposable';

/**
 * Внешние процессы без PTY: сборка, линтеры, компиляторы, прошивка.
 *
 * Отличие от `PtyService`: здесь нет интерактивного терминала, но есть код
 * возврата и общий хронологический вывод stdout+stderr — ровно то, что нужно
 * панели «Вывод» при `cargo build` или `npm run build`.
 */

export interface ProcOptions {
  readonly program: string;
  readonly args?: readonly string[];
  readonly cwd?: string;
  readonly env?: Readonly<Record<string, string>>;
  /** Запустить через оболочку (`cmd /C` / `sh -c`) — для команд с пайпами. */
  readonly shell?: boolean;
  readonly stdin?: boolean;
}

export interface ProcInfo {
  readonly id: string;
  readonly pid: number;
  readonly program: string;
  /**
   * `false` — процесс завершился, но вывод ещё доступен через `replay()`.
   * Такие записи вытесняются автоматически, поэтому `list()` не растёт.
   */
  readonly alive: boolean;
}

export interface ProcHandle extends Disposable {
  readonly id: string;
  readonly pid: number;
  write(data: string): Promise<void>;
  kill(): Promise<void>;
  onData(listener: (chunk: string) => void): Disposable;
  onExit(listener: (code: number | null) => void): Disposable;
  onError(listener: (message: string) => void): Disposable;
  replay(): Promise<string>;
  /** Удобно для `await proc.done`. */
  readonly done: Promise<number | null>;
}

export interface ProcService {
  spawn(options: ProcOptions): Promise<ProcHandle>;
  list(): Promise<ProcInfo[]>;
  /** Путь к программе, если она есть в PATH. */
  which(program: string): Promise<string | null>;
}

interface ProcSpawned {
  readonly id: string;
  readonly pid: number;
}

interface ProcDataPayload {
  readonly id: string;
  readonly data: string;
}

interface ProcExitPayload {
  readonly id: string;
  readonly code: number | null;
}

interface ProcErrorPayload {
  readonly id: string;
  readonly message: string;
}

class ProcHandleImpl implements ProcHandle {
  readonly done: Promise<number | null>;
  private resolveDone!: (code: number | null) => void;
  private settled = false;

  constructor(
    readonly id: string,
    readonly pid: number,
  ) {
    this.done = new Promise<number | null>((resolve) => {
      this.resolveDone = resolve;
    });

    // Подписываемся сразу: короткоживущий процесс (например `git status`)
    // успевает завершиться раньше, чем вызывающий код успеет подписаться.
    this.onExit((code) => {
      if (this.settled) return;
      this.settled = true;
      this.resolveDone(code);
    });
  }

  write(data: string): Promise<void> {
    return invoke('proc_write', { id: this.id, data });
  }

  kill(): Promise<void> {
    return invoke('proc_kill', { id: this.id });
  }

  replay(): Promise<string> {
    return invoke<string>('proc_replay', { id: this.id });
  }

  onData(listener: (chunk: string) => void): Disposable {
    return listenScoped<ProcDataPayload>('proc:data', this.id, (p) => listener(p.data));
  }

  onExit(listener: (code: number | null) => void): Disposable {
    return listenScoped<ProcExitPayload>('proc:exit', this.id, (p) => listener(p.code));
  }

  onError(listener: (message: string) => void): Disposable {
    return listenScoped<ProcErrorPayload>('proc:error', this.id, (p) => listener(p.message));
  }

  dispose(): void {
    void this.kill();
  }
}

function listenScoped<T extends { id: string }>(
  event: string,
  id: string,
  handler: (payload: T) => void,
): Disposable {
  let unlisten: (() => void) | null = null;
  let cancelled = false;

  void listen<T>(event, (e) => {
    if (e.payload.id === id) handler(e.payload);
  }).then((fn) => {
    if (cancelled) fn();
    else unlisten = fn;
  });

  return toDisposable(() => {
    cancelled = true;
    unlisten?.();
  });
}

export class TauriProcService implements ProcService {
  private counter = 0;

  async spawn(options: ProcOptions): Promise<ProcHandle> {
    const id = `proc-${++this.counter}-${Date.now()}`;
    const spawned = await invoke<ProcSpawned>('proc_spawn', {
      id,
      program: options.program,
      args: options.args ? [...options.args] : undefined,
      cwd: options.cwd,
      env: options.env,
      shell: options.shell,
      stdin: options.stdin,
    });
    return new ProcHandleImpl(spawned.id, spawned.pid);
  }

  list(): Promise<ProcInfo[]> {
    return invoke<ProcInfo[]>('proc_list');
  }

  which(program: string): Promise<string | null> {
    return invoke<string | null>('proc_which', { program });
  }
}
