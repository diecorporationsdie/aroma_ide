export const PluginStatus = {
  OK: 200,
  ACCEPTED: 202,
  NO_CONTENT: 204,
  BAD_REQUEST: 400,
  NOT_FOUND: 404,
  CONFLICT: 409,
  DEPENDENCY_MISSING: 424,
  INTERNAL_ERROR: 500,
  NOT_AVAILABLE: 503,
} as const;

export type PluginStatusCode = (typeof PluginStatus)[keyof typeof PluginStatus];

export interface PluginStatusResult<T = unknown> {
  readonly code: PluginStatusCode;
  readonly message?: string;
  readonly value?: T;
  readonly deliveredTo?: number;
}

export function ok<T = unknown>(value?: T): PluginStatusResult<T> {
  return value === undefined ? { code: PluginStatus.OK } : { code: PluginStatus.OK, value };
}

export function accepted<T = unknown>(value?: T): PluginStatusResult<T> {
  return value === undefined
    ? { code: PluginStatus.ACCEPTED }
    : { code: PluginStatus.ACCEPTED, value };
}

export function fail(
  code: PluginStatusCode,
  message: string,
): PluginStatusResult<never> {
  return { code, message };
}
