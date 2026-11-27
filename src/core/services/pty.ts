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

export interface PtyInfo {
  readonly id: string;
  readonly pid: number | null;
  readonly cols: number;
  readonly rows: number;
  /**
   * `false` — процесс завершился, но вывод ещё доступен через `replay()`.
   * Такие сессии вытесняются автоматически, поэтому `list()` не растёт.
   */
  readonly alive: boolean;
}

export interface PtyProcess extends Disposable {
  readonly id: string;
  readonly pid: number | null;
  write(data: string): Promise<void>;
  resize(cols: number, rows: number): Promise<void>;
  kill(): Promise<void>;
  onData(listener: (chunk: string) => void): Disposable;
  onExit(listener: (code: number | null) => void): Disposable;
  onError(listener: (message: string) => void): Disposable;
  /** Последние байты вывода: нужно при перемонтировании вкладки терминала. */
  replay(): Promise<string>;
}

export interface PtyService {
  spawn(options?: PtyOptions): Promise<PtyProcess>;
  list(): Promise<PtyInfo[]>;
}

interface PtySpawned {
  readonly id: string;
  readonly pid: number | null;
}

interface PtyDataPayload {
  readonly id: string;
  readonly data: string;
}

interface PtyExitPayload {
  readonly id: string;
  readonly code: number | null;
}

interface PtyErrorPayload {
  readonly id: string;
  readonly message: string;
}

/**
 * События `pty:data` идут через общую шину на бэкенде: данные приходят
 * пакетами, а не по символу, и UTF-8 уже собран в бэкенде — разорванный
 * многобайтовый символ не превращается в «кракозябру».
 */
class PtyProcessImpl implements PtyProcess {
  constructor(
    readonly id: string,
    readonly pid: number | null,
  ) {}

  write(data: string): Promise<void> {
    return invoke('pty_write', { id: this.id, data });
  }

  resize(cols: number, rows: number): Promise<void> {
    return invoke('pty_resize', { id: this.id, cols, rows });
  }

  kill(): Promise<void> {
    return invoke('pty_kill', { id: this.id });
  }

  replay(): Promise<string> {
    return invoke<string>('pty_replay', { id: this.id });
  }

  onData(listener: (chunk: string) => void): Disposable {
    return listenScoped<PtyDataPayload>('pty:data', this.id, (p) => listener(p.data));
  }

  onExit(listener: (code: number | null) => void): Disposable {
    return listenScoped<PtyExitPayload>('pty:exit', this.id, (p) => listener(p.code));
  }

  onError(listener: (message: string) => void): Disposable {
    return listenScoped<PtyErrorPayload>('pty:error', this.id, (p) => listener(p.message));
  }

  dispose(): void {
    void this.kill();
  }
}

/**
 * Подписка с фильтром по id процесса.
 *
 * Возвращает `Disposable` синхронно, хотя `listen` асинхронный: если отписаться
 * до того, как промис разрешится, отписка всё равно произойдёт — иначе
 * перезагрузка плагина оставляла бы слушателей навсегда.
 */
function listenScoped<T extends { id: string }>(
  event: string,
  id: string,
  handler: (payload: T) => void,
): Disposable {
  let unlisten: UnlistenFn | null = null;
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

export class TauriPtyService implements PtyService {
  private counter = 0;

  async spawn(options: PtyOptions = {}): Promise<PtyProcess> {
    const id = `pty-${++this.counter}-${Date.now()}`;
    const spawned = await invoke<PtySpawned>('pty_spawn', {
      id,
      shell: options.shell,
      args: options.args ? [...options.args] : undefined,
      cwd: options.cwd,
      env: options.env,
      cols: options.cols,
      rows: options.rows,
    });
    return new PtyProcessImpl(spawned.id, spawned.pid);
  }

  list(): Promise<PtyInfo[]> {
    return invoke<PtyInfo[]>('pty_list');
  }
}
