import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { toDisposable } from '../disposable';
import type { Disposable } from '../disposable';

/**
 * Последовательные порты: прошивка плат и обмен с контроллерами.
 *
 * Данные — байты, а не текст: протоколы МК бинарные. Поэтому на вход и выход
 * идут `Uint8Array`, а через IPC — base64.
 */

export interface PortInfo {
  readonly name: string;
  readonly kind: string;
  readonly manufacturer: string | null;
  readonly product: string | null;
  readonly serialNumber: string | null;
  readonly vid: number | null;
  readonly pid: number | null;
}

export interface SerialOptions {
  readonly baudRate?: number;
  readonly dataBits?: 5 | 6 | 7 | 8;
  readonly parity?: 'none' | 'odd' | 'even';
  readonly stopBits?: 1 | 2;
  readonly flowControl?: 'none' | 'hardware' | 'software';
  readonly timeoutMs?: number;
}

export interface SerialHandle extends Disposable {
  readonly id: string;
  readonly portName: string;
  write(bytes: Uint8Array): Promise<void>;
  /** DTR/RTS: сброс платы в загрузчик. */
  setSignals(signals: { dtr?: boolean; rts?: boolean }): Promise<void>;
  close(): Promise<void>;
  onData(listener: (bytes: Uint8Array) => void): Disposable;
  onExit(listener: () => void): Disposable;
  onError(listener: (message: string) => void): Disposable;
}

export interface SerialService {
  listPorts(): Promise<PortInfo[]>;
  open(portName: string, options?: SerialOptions): Promise<SerialHandle>;
  list(): Promise<string[]>;
}

interface SerialDataPayload {
  readonly id: string;
  readonly data: string;
  readonly binary?: boolean;
}

interface SerialExitPayload {
  readonly id: string;
  readonly code: number | null;
}

interface SerialErrorPayload {
  readonly id: string;
  readonly message: string;
}

/** base64 → байты без промежуточной строки символов. */
function fromBase64(input: string): Uint8Array {
  const binary = atob(input);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

/** байты → base64 кусками: `String.fromCharCode(...big)` падает на больших данных. */
function toBase64(bytes: Uint8Array): string {
  const CHUNK = 0x8000;
  let binary = '';
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

class SerialHandleImpl implements SerialHandle {
  constructor(
    readonly id: string,
    readonly portName: string,
  ) {}

  write(bytes: Uint8Array): Promise<void> {
    return invoke('serial_write', { id: this.id, data: toBase64(bytes) });
  }

  setSignals(signals: { dtr?: boolean; rts?: boolean }): Promise<void> {
    return invoke('serial_set_signals', {
      id: this.id,
      dtr: signals.dtr,
      rts: signals.rts,
    });
  }

  close(): Promise<void> {
    return invoke('serial_close', { id: this.id });
  }

  onData(listener: (bytes: Uint8Array) => void): Disposable {
    return listenScoped<SerialDataPayload>('serial:data', this.id, (p) => {
      listener(fromBase64(p.data));
    });
  }

  onExit(listener: () => void): Disposable {
    return listenScoped<SerialExitPayload>('serial:exit', this.id, () => listener());
  }

  onError(listener: (message: string) => void): Disposable {
    return listenScoped<SerialErrorPayload>('serial:error', this.id, (p) =>
      listener(p.message),
    );
  }

  dispose(): void {
    void this.close();
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

export class TauriSerialService implements SerialService {
  private counter = 0;

  listPorts(): Promise<PortInfo[]> {
    return invoke<PortInfo[]>('serial_list_ports');
  }

  async open(portName: string, options: SerialOptions = {}): Promise<SerialHandle> {
    const id = `serial-${++this.counter}-${Date.now()}`;
    await invoke('serial_open', { id, portName, options });
    return new SerialHandleImpl(id, portName);
  }

  list(): Promise<string[]> {
    return invoke<string[]>('serial_list');
  }
}
