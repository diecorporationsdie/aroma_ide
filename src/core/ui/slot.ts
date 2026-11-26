import type { SlotHandle } from '../plugin/api';
import type { UiHost } from '../plugin/ui-host';
import type { Region, SlotOptions, ZTier } from '../types';
import type { LayoutManager } from './layout';
import { ZIndexPool } from './positioning';

class SlotImpl implements SlotHandle {
  private disposed = false;

  constructor(
    readonly id: string,
    readonly region: Region,
    readonly element: HTMLElement,
    readonly tier: ZTier,
    readonly zIndex: number,
    private readonly pluginId: string,
    private readonly host: DomUiHost,
  ) {}

  create<K extends keyof HTMLElementTagNameMap>(tag: K): HTMLElementTagNameMap[K] {
    return this.element.ownerDocument.createElement(tag);
  }

  addSlot(options: SlotOptions = {}): SlotHandle {
    return this.host.mountSlot(this.pluginId, this.region, this.element, options);
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.element.remove();
  }
}

export class DomUiHost implements UiHost {
  private counter = 0;
  private readonly zpool = new ZIndexPool();

  constructor(private readonly layout: LayoutManager) {}

  slot(pluginId: string, region: Region, options: SlotOptions = {}): Promise<SlotHandle> {
    const container = this.layout.region(region);
    return Promise.resolve(this.mountSlot(pluginId, region, container, options));
  }

  mountSlot(
    pluginId: string,
    region: Region,
    container: HTMLElement,
    options: SlotOptions = {},
  ): SlotHandle {
    const position = options.position ?? 'relative';
    const tier = options.tier ?? (region === 'overlay' ? 'overlay' : 'normal');
    const scroll = options.scroll ?? true;
    const zIndex = this.zpool.next(tier);
    const id = `${pluginId}:${region}:${++this.counter}`;

    const element = document.createElement('div');
    element.className = `ide-slot plugin-${pluginId}`;
    element.dataset.pluginId = pluginId;
    element.dataset.region = region;
    element.dataset.slotId = id;
    element.dataset.position = position;
    element.dataset.tier = tier;
    element.style.zIndex = String(zIndex);
    if (!scroll) element.style.overflow = 'hidden';
    container.appendChild(element);

    return new SlotImpl(id, region, element, tier, zIndex, pluginId, this);
  }
}
