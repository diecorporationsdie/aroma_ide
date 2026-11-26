import { DisposableBase, toDisposable } from '../disposable';
import type { Disposable } from '../disposable';
import type { InputHandler, OutputListener } from './io';
import { PluginStatus } from './status';
import type { PluginStatusResult } from './status';

interface OutputBucket {
  readonly listeners: Set<OutputListener>;
}

export class IORegistry extends DisposableBase {
  private readonly inputs = new Map<string, InputHandler>();
  private readonly outputs = new Map<string, OutputBucket>();

  declareOutput(pluginId: string, outputId: string): Disposable {
    const k = this.key(pluginId, outputId);
    if (this.outputs.has(k)) throw new Error(`Output already declared: ${k}`);
    this.outputs.set(k, { listeners: new Set() });
    return toDisposable(() => this.outputs.delete(k));
  }

  declareInput(pluginId: string, inputId: string, handler: InputHandler): Disposable {
    const k = this.key(pluginId, inputId);
    if (this.inputs.has(k)) throw new Error(`Input already declared: ${k}`);
    this.inputs.set(k, handler);
    return toDisposable(() => this.inputs.delete(k));
  }

  publish(pluginId: string, outputId: string, value: unknown): PluginStatusResult {
    const k = this.key(pluginId, outputId);
    const bucket = this.outputs.get(k);
    if (!bucket) return { code: PluginStatus.NOT_FOUND, message: `output not declared: ${k}` };

    let delivered = 0;
    for (const listener of [...bucket.listeners]) {
      try {
        listener(value);
        delivered++;
      } catch (e) {
        console.error(`[io:${k}] listener failed`, e);
      }
    }
    if (delivered === 0) return { code: PluginStatus.ACCEPTED, deliveredTo: 0 };
    return { code: PluginStatus.OK, deliveredTo: delivered };
  }

  subscribe(pluginId: string, outputId: string, listener: OutputListener): Disposable {
    const k = this.key(pluginId, outputId);
    const bucket = this.outputs.get(k);
    if (!bucket) throw new Error(`output not declared: ${k}`);
    bucket.listeners.add(listener);
    return toDisposable(() => bucket.listeners.delete(listener));
  }

  async inject(pluginId: string, inputId: string, value: unknown): Promise<PluginStatusResult> {
    const k = this.key(pluginId, inputId);
    const handler = this.inputs.get(k);
    if (!handler) return { code: PluginStatus.NOT_FOUND, message: `input not declared: ${k}` };
    try {
      const result = await handler(value);
      return result ?? { code: PluginStatus.OK };
    } catch (e) {
      console.error(`[io:${k}] input handler failed`, e);
      return { code: PluginStatus.INTERNAL_ERROR, message: String(e) };
    }
  }

  hasOutput(pluginId: string, outputId: string): boolean {
    return this.outputs.has(this.key(pluginId, outputId));
  }

  hasInput(pluginId: string, inputId: string): boolean {
    return this.inputs.has(this.key(pluginId, inputId));
  }

  dropPlugin(pluginId: string): void {
    const prefix = `${pluginId}:`;
    for (const k of [...this.inputs.keys()]) {
      if (k.startsWith(prefix)) this.inputs.delete(k);
    }
    for (const k of [...this.outputs.keys()]) {
      if (k.startsWith(prefix)) this.outputs.delete(k);
    }
  }

  dispose(): void {
    this.inputs.clear();
    this.outputs.clear();
    super.dispose();
  }

  private key(pluginId: string, slot: string): string {
    return `${pluginId}:${slot}`;
  }
}
