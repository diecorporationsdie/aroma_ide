import type { PluginStatusResult } from './status';

export type OutputListener = (value: unknown) => void;

export type InputHandler = (
  value: unknown,
) => PluginStatusResult | void | Promise<PluginStatusResult | void>;

export interface IORegistryApi {
  declareOutput(pluginId: string, outputId: string): { dispose(): void };
  publish(pluginId: string, outputId: string, value: unknown): PluginStatusResult;
  subscribe(pluginId: string, outputId: string, listener: OutputListener): { dispose(): void };
  inject(pluginId: string, inputId: string, value: unknown): Promise<PluginStatusResult>;
  hasOutput(pluginId: string, outputId: string): boolean;
  hasInput(pluginId: string, inputId: string): boolean;
  dropPlugin(pluginId: string): void;
}
