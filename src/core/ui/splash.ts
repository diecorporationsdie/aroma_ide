export type SplashStatus = 'pending' | 'loading' | 'ok' | 'error' | 'skipped';

const ICONS: Record<SplashStatus, string> = {
  pending: '·',
  loading: '⟳',
  ok: '✓',
  error: '✗',
  skipped: '−',
};

interface PluginInfo {
  readonly id: string;
  readonly name: string;
}

interface ChipEntry {
  readonly el: HTMLSpanElement;
  status: SplashStatus;
}

export class SplashScreen {
  readonly element: HTMLDivElement;
  private readonly progressFill: HTMLDivElement;
  private readonly statusEl: HTMLDivElement;
  private readonly chipsEl: HTMLDivElement;
  private readonly chips = new Map<string, ChipEntry>();
  private total = 0;
  private completed = 0;
  private fadedOut = false;

  constructor(appTitle = 'AROMA IDE') {
    this.element = document.createElement('div');
    this.element.className = 'ide-splash';

    const progress = document.createElement('div');
    progress.className = 'ide-splash-progress';
    this.progressFill = document.createElement('div');
    this.progressFill.className = 'ide-splash-progress-fill';
    progress.appendChild(this.progressFill);
    this.element.appendChild(progress);

    const body = document.createElement('div');
    body.className = 'ide-splash-body';

    const title = document.createElement('div');
    title.className = 'ide-splash-title';
    title.textContent = appTitle;

    this.statusEl = document.createElement('div');
    this.statusEl.className = 'ide-splash-status';
    this.statusEl.textContent = 'Инициализация…';

    this.chipsEl = document.createElement('div');
    this.chipsEl.className = 'ide-splash-chips';

    body.append(title, this.statusEl, this.chipsEl);
    this.element.appendChild(body);
  }

  setStatus(text: string): void {
    this.statusEl.textContent = text;
  }

  setPlugins(plugins: ReadonlyArray<PluginInfo>): void {
    this.chipsEl.replaceChildren();
    this.chips.clear();
    this.total = plugins.length;
    this.completed = 0;

    for (const p of plugins) {
      const chip = document.createElement('span');
      chip.className = 'ide-splash-chip';
      chip.dataset.status = 'pending';

      const icon = document.createElement('span');
      icon.className = 'ide-splash-icon';
      icon.textContent = ICONS.pending;

      const name = document.createElement('span');
      name.textContent = p.name || p.id;

      chip.append(icon, name);
      this.chipsEl.appendChild(chip);
      this.chips.set(p.id, { el: chip, status: 'pending' });
    }
    this.updateProgress();
  }

  setChip(id: string, status: SplashStatus): void {
    const chip = this.chips.get(id);
    if (!chip) return;
    const prev = chip.status;
    chip.status = status;
    chip.el.dataset.status = status;
    const icon = chip.el.querySelector('.ide-splash-icon');
    if (icon) icon.textContent = ICONS[status];

    const isDone = status === 'ok' || status === 'error' || status === 'skipped';
    const wasDone = prev === 'ok' || prev === 'error' || prev === 'skipped';
    if (isDone && !wasDone) this.completed++;
    this.updateProgress();
  }

  async fadeOut(durationMs = 420): Promise<void> {
    if (this.fadedOut) return;
    this.fadedOut = true;
    this.element.style.transition = `opacity ${durationMs}ms ease`;
    void this.element.offsetHeight;
    this.element.style.opacity = '0';
    await new Promise<void>((resolve) => {
      let done = false;
      const finish = (): void => {
        if (done) return;
        done = true;
        resolve();
      };
      this.element.addEventListener('transitionend', finish, { once: true });
      setTimeout(finish, durationMs + 60);
    });
    this.element.remove();
  }

  dispose(): void {
    this.element.remove();
  }

  private updateProgress(): void {
    const pct = this.total === 0 ? 0 : (this.completed / this.total) * 100;
    this.progressFill.style.width = `${pct}%`;
  }
}
