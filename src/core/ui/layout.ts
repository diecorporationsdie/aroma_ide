import { DisposableBase } from '../disposable';
import type { Storage, StorageNamespace } from '../storage';
import type { Region } from '../types';
import { Titlebar } from './titlebar';

export interface LayoutConfig {
  readonly sidebarWidth: number;
  readonly bottomHeight: number;
  readonly statusbarHeight: number;
}

const DEFAULT_CONFIG: LayoutConfig = {
  sidebarWidth: 260,
  bottomHeight: 200,
  statusbarHeight: 22,
};

const STORAGE_NAMESPACE = 'core.layout';
const STORAGE_KEY = 'config';

interface RegionSpec {
  readonly region: Region;
  readonly className: string;
}

const REGIONS: readonly RegionSpec[] = [
  { region: 'sidebar', className: 'ide-sidebar' },
  { region: 'main', className: 'ide-main' },
  { region: 'bottom', className: 'ide-bottom' },
  { region: 'statusbar', className: 'ide-statusbar' },
  { region: 'overlay', className: 'ide-overlay' },
];

export class LayoutManager extends DisposableBase {
  private readonly root = document.createElement('div');
  private readonly regions = new Map<Region, HTMLElement>();
  private readonly titlebar: Titlebar;
  private readonly ns: StorageNamespace;
  private config: LayoutConfig = { ...DEFAULT_CONFIG };

  constructor(storage: Storage, title: string) {
    super();
    this.ns = storage.namespace(STORAGE_NAMESPACE);
    this.titlebar = this._store.add(new Titlebar(title));
    this.buildDom();
  }

  async mount(parent: HTMLElement): Promise<void> {
    parent.replaceChildren(this.root);
    const stored = await this.ns.get<LayoutConfig>(STORAGE_KEY);
    if (stored) this.config = { ...DEFAULT_CONFIG, ...stored };
    this.applyConfig();
  }

  region(region: Region): HTMLElement {
    if (region === 'topbar') return this.titlebar.menuElement;
    const el = this.regions.get(region);
    if (!el) throw new Error(`Unknown layout region: ${region}`);
    return el;
  }

  get titlebarMenu(): HTMLElement {
    return this.titlebar.menuElement;
  }

  getConfig(): Readonly<LayoutConfig> {
    return { ...this.config };
  }

  async setConfig(patch: Partial<LayoutConfig>): Promise<void> {
    this.config = { ...this.config, ...patch };
    this.applyConfig();
    await this.ns.set(STORAGE_KEY, this.config);
  }

  dispose(): void {
    this.root.remove();
    this.regions.clear();
    super.dispose();
  }

  private buildDom(): void {
    this.root.className = 'ide-shell';
    this.root.appendChild(this.titlebar.element);
    for (const { region, className } of REGIONS) {
      const el = document.createElement('div');
      el.className = `ide-region ${className}`;
      el.dataset.region = region;
      this.regions.set(region, el);
      this.root.appendChild(el);
    }
  }

  private applyConfig(): void {
    const s = this.root.style;
    s.setProperty('--ide-sidebar-width', `${this.config.sidebarWidth}px`);
    s.setProperty('--ide-bottom-height', `${this.config.bottomHeight}px`);
    s.setProperty('--ide-statusbar-height', `${this.config.statusbarHeight}px`);
  }
}
