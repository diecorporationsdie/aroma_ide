export class PluginError extends Error {
  constructor(
    readonly pluginId: string,
    message: string,
    readonly cause?: unknown,
  ) {
    super(`[plugin:${pluginId}] ${message}`);
    this.name = 'PluginError';
  }
}

export function isolate<T>(
  fn: () => T,
  onError: (e: unknown) => void = (e) => console.error(e),
): T | undefined {
  try {
    return fn();
  } catch (e) {
    onError(e);
    return undefined;
  }
}

export async function isolateAsync<T>(
  fn: () => Promise<T>,
  onError: (e: unknown) => void = (e) => console.error(e),
): Promise<T | undefined> {
  try {
    return await fn();
  } catch (e) {
    onError(e);
    return undefined;
  }
}
