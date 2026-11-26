import { getCurrentWindow } from '@tauri-apps/api/window';
import { DisposableBase } from '../disposable';

type WindowAction = 'minimize' | 'toggle-maximize' | 'close';

interface ControlSpec {
  readonly action: WindowAction;
  readonly glyph: string;
  readonly extraClass?: string;
}

const CONTROLS: readonly ControlSpec[] = [
  { action: 'minimize', glyph: '─' },
  { action: 'toggle-maximize', glyph: '□' },
  { action: 'close', glyph: '✕', extraClass: 'close' },
];

const DRAG_ATTR = 'data-tauri-drag-region';

export class Titlebar extends DisposableBase {
  private readonly root = document.createElement('div');
  private readonly menu = document.createElement('div');

  constructor(title: string) {
    super();
    this.build(title);
  }

  get element(): HTMLElement {
    return this.root;
  }

  get menuElement(): HTMLElement {
    return this.menu;
  }

  private build(title: string): void {
    this.root.className = 'ide-titlebar';

    const titleEl = document.createElement('div');
    titleEl.className = 'ide-titlebar-title';
    titleEl.textContent = title;
    titleEl.setAttribute(DRAG_ATTR, '');
    this.root.appendChild(titleEl);

    this.menu.className = 'ide-titlebar-menu';
    this.root.appendChild(this.menu);

    const spacer = document.createElement('div');
    spacer.className = 'ide-titlebar-spacer';
    spacer.setAttribute(DRAG_ATTR, '');
    this.root.appendChild(spacer);

    const controls = document.createElement('div');
    controls.className = 'ide-titlebar-controls';
    for (const spec of CONTROLS) {
      controls.appendChild(this.buildButton(spec));
    }
    this.root.appendChild(controls);
  }

  private buildButton(spec: ControlSpec): HTMLButtonElement {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = `ide-titlebar-btn${spec.extraClass ? ` ${spec.extraClass}` : ''}`;
    btn.textContent = spec.glyph;
    btn.addEventListener('click', () => void this.runAction(spec.action));
    return btn;
  }

  private async runAction(action: WindowAction): Promise<void> {
    try {
      const win = getCurrentWindow();
      if (action === 'minimize') await win.minimize();
      else if (action === 'toggle-maximize') await win.toggleMaximize();
      else await win.close();
    } catch (e) {
      console.warn(`[titlebar] ${action} failed (non-Tauri env?)`, e);
    }
  }

  dispose(): void {
    this.root.remove();
    super.dispose();
  }
}
