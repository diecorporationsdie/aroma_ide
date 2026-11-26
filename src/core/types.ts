export type Region =
  | 'topbar'
  | 'sidebar'
  | 'main'
  | 'bottom'
  | 'statusbar'
  | 'overlay';

export type ZTier = 'background' | 'normal' | 'raised' | 'overlay' | 'modal' | 'system';

export type SlotPosition = 'relative' | 'fixed';

export interface SlotOptions {
  readonly position?: SlotPosition;
  readonly tier?: ZTier;
  readonly scroll?: boolean;
}

export interface IOSlotDescriptor {
  readonly id: string;
  readonly description?: string;
}

export interface CapabilityDeclaration {
  readonly id: string;
  readonly version: string;
  readonly description?: string;
}

export interface CapabilityRequirement {
  readonly id: string;
  readonly version?: string;
  readonly optional?: boolean;
}

export interface PluginManifest {
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly description?: string;
  readonly entry?: string;
  readonly style?: string;
  readonly trust?: 'trusted' | 'sandboxed';
  readonly inputs?: readonly IOSlotDescriptor[];
  readonly outputs?: readonly IOSlotDescriptor[];
  readonly provides?: readonly CapabilityDeclaration[];
  readonly requires?: readonly CapabilityRequirement[];
}
