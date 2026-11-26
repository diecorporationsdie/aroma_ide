/** @typedef {import('../../../../src/core/plugin').PluginContext} PluginContext */
/** @typedef {{ add(d: { dispose(): void }): { dispose(): void } }} DisposableStore */

class TerminalSession {
  /** @type {any} */
  #term;
  /** @type {any} */
  #fit;
  /** @type {any | null} */
  #proc = null;
  /** @type {ResizeObserver | null} */
  #observer = null;

  /** @param {any} Terminal @param {any} FitAddon @param {HTMLElement} host */
  constructor(Terminal, FitAddon, host) {
    this.#term = new Terminal({
      fontFamily: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace',
      fontSize: 13,
      cursorBlink: true,
      theme: {
        background: '#1e1e1e',
        foreground: '#d4d4d4',
        cursor: '#0a84ff',
      },
    });
    this.#fit = new FitAddon();
    this.#term.loadAddon(this.#fit);
    this.#term.open(host);
  }

  /** @param {any} proc @param {DisposableStore} subs */
  attach(proc, subs) {
    this.#proc = proc;
    subs.add(proc);
    subs.add(proc.onData((chunk) => this.#term.write(chunk)));
    subs.add(proc.onExit(() => this.#term.write('\r\n[process exited]\r\n')));
    const inputSub = this.#term.onData((data) => { void proc.write(data); });
    subs.add({ dispose: () => inputSub.dispose() });
    return this;
  }

  /** @param {HTMLElement} container @param {DisposableStore} subs */
  attachResize(container, subs) {
    const doFit = () => {
      try {
        this.#fit.fit();
        if (this.#proc) void this.#proc.resize(this.#term.cols, this.#term.rows);
      } catch { /* layout not ready */ }
    };
    this.#observer = new ResizeObserver(doFit);
    this.#observer.observe(container);
    subs.add({
      dispose: () => {
        this.#observer?.disconnect();
        this.#observer = null;
      },
    });
    queueMicrotask(doFit);
    return this;
  }

  focus() { this.#term.focus(); }
}

/** @param {PluginContext} ctx */
export async function activate(ctx) {
  const pty = await ctx.services.get('pty');
  if (!pty) throw new Error('pty service unavailable');

  const vendor = await ctx.services.get('vendor.xterm');
  if (!vendor) throw new Error('vendor.xterm service unavailable');

  const { Terminal, FitAddon } = vendor;

  const slot = await ctx.ui.slot('bottom', { scroll: false });
  ctx.subscriptions.add(slot);
  slot.element.replaceChildren();

  const host = document.createElement('div');
  host.className = 'terminal-host';
  slot.element.appendChild(host);

  const session = new TerminalSession(Terminal, FitAddon, host);
  const proc = await pty.spawn({});

  session.attach(proc, ctx.subscriptions);
  session.attachResize(host, ctx.subscriptions);
  session.focus();
}
