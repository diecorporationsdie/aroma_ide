import type { Region, SlotOptions } from '../types';
import type { SlotHandle } from './api';

export interface UiHost {
  slot(pluginId: string, region: Region, options?: SlotOptions): Promise<SlotHandle>;
}

export class NullUiHost implements UiHost {
  private counter = 0;

  slot(pluginId: string, region: Region, _options?: SlotOptions): Promise<SlotHandle> {
    const element = document.createElement('div');
    const id = `null:${pluginId}:${region}:${++this.counter}`;
    return Promise.resolve({
      id,
      region,
      element,
      tier: 'normal',
      zIndex: 0,
      create: <K extends keyof HTMLElementTagNameMap>(tag: K) =>
        element.ownerDocument.createElement(tag),
      addSlot: () => {
        throw new Error('NullUiHost does not support nested slots');
      },
      dispose: () => element.remove(),
    });
  }
}
