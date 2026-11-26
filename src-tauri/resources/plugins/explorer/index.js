/** @typedef {import('../../../../src/core/plugin').PluginContext} PluginContext */
/** @typedef {{ name: string; path: string; isDir: boolean }} FsEntry */

const STORAGE_PROJECT = 'projectPath';
const STORAGE_EXPANDED = 'expandedPaths';

/** @type {boolean} */
let schemaLogged = false;

/**
 * Срезает парные кавычки по краям.
 * @param {string} raw
 */
function cleanPath(raw) {
  const s = String(raw ?? '').trim();
  if (s.length < 2) return s;
  const first = s[0];
  const last = s[s.length - 1];
  if ((first === '"' || first === "'" || first === '`') && first === last) {
    return s.slice(1, -1).trim();
  }
  return s;
}

/**
 * @param {any} stat
 */
function isDirectoryStat(stat) {
  if (!stat || typeof stat !== 'object') return false;
  return (
    stat.isDir === true ||
    stat.is_dir === true ||
    stat.isDirectory === true ||
    stat.type === 'dir' ||
    stat.type === 'directory' ||
    stat.kind === 'dir' ||
    stat.kind === 'directory'
  );
}

/** @param {string} parent @param {string} name */
function joinPath(parent, name) {
  if (!parent) return name;
  const sep = parent.includes('/') ? '/' : '\\';
  return parent.endsWith('/') || parent.endsWith('\\')
    ? `${parent}${name}`
    : `${parent}${sep}${name}`;
}

/**
 * @param {any} raw
 * @param {string} parentPath
 * @returns {FsEntry | null}
 */
function normalizeEntry(raw, parentPath) {
  if (typeof raw === 'string') {
    const name = raw.split(/[\\/]/).filter(Boolean).pop() || raw;
    return { name, path: raw, isDir: false };
  }
  if (!raw || typeof raw !== 'object') return null;

  const name = typeof raw.name === 'string' ? raw.name : '';
  if (!name) return null;

  const isDir =
    raw.isDir === true ||
    raw.is_dir === true ||
    raw.isDirectory === true ||
    raw.type === 'dir' ||
    raw.type === 'directory' ||
    raw.kind === 'dir' ||
    raw.kind === 'directory';

  const path =
    (typeof raw.path === 'string' && raw.path) ||
    (typeof raw.fullPath === 'string' && raw.fullPath) ||
    (typeof raw.full_path === 'string' && raw.full_path) ||
    joinPath(parentPath, name);

  return { name, path, isDir };
}

/** @param {FsEntry} entry */
function isVisible(entry) {
  return !entry.name.startsWith('.') && entry.name !== 'node_modules';
}

/**
 * Единая точка чтения каталога: нормализует поля и фильтрует служебное.
 * @param {any} fs
 * @param {string} dirPath
 * @returns {Promise<FsEntry[]>}
 */
async function readDirectory(fs, dirPath) {
  const raw = await fs.readDir(dirPath);
  if (!Array.isArray(raw)) return [];
  if (!schemaLogged && raw.length) {
    schemaLogged = true;
    console.debug('[explorer] readDir sample:', raw[0]);
  }
  const out = [];
  for (const item of raw) {
    const entry = normalizeEntry(item, dirPath);
    if (entry && isVisible(entry)) out.push(entry);
  }
  return out;
}

class ExplorerNode {
  /** @type {PluginContext} */
  #ctx;
  /** @type {any} */
  #fs;
  /** @type {FsEntry} */
  #entry;
  /** @type {Set<string>} */
  #expanded;
  /** @type {() => void} */
  #onStateChange;
  /** @type {HTMLElement} */
  #element;
  /** @type {HTMLElement | null} */
  #childrenEl = null;
  #rendered = false;

  /**
   * @param {PluginContext} ctx
   * @param {any} fs
   * @param {FsEntry} entry
   * @param {Set<string>} expanded
   * @param {() => void} onStateChange
   */
  constructor(ctx, fs, entry, expanded, onStateChange) {
    this.#ctx = ctx;
    this.#fs = fs;
    this.#entry = entry;
    this.#expanded = expanded;
    this.#onStateChange = onStateChange;
    this.#element = this.#build();
  }

  get element() { return this.#element; }

  get isExpanded() { return this.#expanded.has(this.#entry.path); }

  async renderIfExpanded() {
    if (this.#entry.isDir && this.isExpanded) await this.#ensureChildren();
  }

  #build() {
    const wrap = document.createElement('div');
    wrap.className = 'explorer-node';

    const row = document.createElement('div');
    row.className = 'explorer-row';
    row.classList.toggle('is-dir', this.#entry.isDir);
    row.classList.toggle('is-file', !this.#entry.isDir);
    row.title = this.#entry.path;

    const chevron = document.createElement('span');
    chevron.className = 'explorer-chevron';
    if (this.#entry.isDir) chevron.textContent = this.isExpanded ? '▾' : '▸';
    row.appendChild(chevron);

    const icon = document.createElement('span');
    icon.className = 'explorer-icon';
    icon.textContent = this.#entry.isDir ? '📁' : '📄';
    row.appendChild(icon);

    const label = document.createElement('span');
    label.className = 'explorer-label';
    label.textContent = this.#entry.name;
    row.appendChild(label);

    wrap.appendChild(row);

    if (this.#entry.isDir) {
      this.#childrenEl = document.createElement('div');
      this.#childrenEl.className = 'explorer-children';
      this.#childrenEl.hidden = !this.isExpanded;
      wrap.appendChild(this.#childrenEl);

      row.classList.toggle('expanded', this.isExpanded);
      row.addEventListener('click', () => { void this.#toggle(); });
    } else {
      row.addEventListener('click', () => {
        this.#ctx.events.emit('explorer:fileOpen', {
          path: this.#entry.path,
          name: this.#entry.name,
        });
      });
    }

    return wrap;
  }

  async #toggle() {
    const expanded = !this.isExpanded;
    if (expanded) this.#expanded.add(this.#entry.path);
    else this.#expanded.delete(this.#entry.path);

    const row = /** @type {HTMLElement} */ (this.#element.querySelector(':scope > .explorer-row'));
    row.classList.toggle('expanded', expanded);
    if (this.#childrenEl) this.#childrenEl.hidden = !expanded;
    const chevron = row.querySelector('.explorer-chevron');
    if (chevron) chevron.textContent = expanded ? '▾' : '▸';

    if (expanded) await this.#ensureChildren();
    this.#onStateChange();
  }

  async #ensureChildren() {
    if (this.#rendered || !this.#childrenEl) return;
    this.#rendered = true;
    let entries;
    try { entries = await readDirectory(this.#fs, this.#entry.path); }
    catch (e) {
      console.error('[explorer] readDir failed', this.#entry.path, e);
      return;
    }
    this.#childrenEl.replaceChildren();
    for (const entry of entries) {
      const node = new ExplorerNode(this.#ctx, this.#fs, entry, this.#expanded, this.#onStateChange);
      this.#childrenEl.appendChild(node.element);
      await node.renderIfExpanded();
    }
  }
}

class ExplorerTree {
  /** @type {PluginContext} */
  #ctx;
  /** @type {any} */
  #fs;
  /** @type {HTMLElement} */
  #header;
  /** @type {HTMLElement} */
  #container;
  /** @type {Set<string>} */
  #expanded = new Set();
  /** @type {ReturnType<typeof setTimeout> | null} */
  #saveTimer = null;

  /**
   * @param {PluginContext} ctx
   * @param {any} fs
   * @param {HTMLElement} header
   * @param {HTMLElement} container
   */
  constructor(ctx, fs, header, container) {
    this.#ctx = ctx;
    this.#fs = fs;
    this.#header = header;
    this.#container = container;
  }

  async restore() {
    const path = await this.#ctx.storage.get(STORAGE_PROJECT);
    const expanded = await this.#ctx.storage.get(STORAGE_EXPANDED);
    if (Array.isArray(expanded)) for (const p of expanded) this.#expanded.add(p);
    if (typeof path === 'string' && path) await this.open(path, false);
    else this.#showEmpty('Проект не открыт. Файл → Открыть проект…');
  }

  /** @param {string} path @param {boolean} [persist] */
  async open(path, persist = true) {
    const target = cleanPath(path);
    if (!target) return;
    try {
      const stat = await this.#fs.stat(target);
      if (!isDirectoryStat(stat)) { this.#showEmpty(`Не папка: ${target}`); return; }
    } catch (e) {
      this.#showEmpty(`Не удалось открыть: ${String(e)}`);
      return;
    }

    this.#setHeader(target);
    await this.#render(target);
    if (persist) await this.#ctx.storage.set(STORAGE_PROJECT, target);
  }

  close() {
    this.#expanded.clear();
    this.#container.replaceChildren();
    this.#setHeader(null);
    this.#showEmpty('Проект не открыт. Файл → Открыть проект…');
    void this.#ctx.storage.delete(STORAGE_PROJECT);
    void this.#ctx.storage.set(STORAGE_EXPANDED, []);
  }

  /** @param {string} dirPath */
  async #render(dirPath) {
    this.#container.replaceChildren();
    let entries;
    try { entries = await readDirectory(this.#fs, dirPath); }
    catch (e) {
      this.#showEmpty(`Не удалось прочитать: ${String(e)}`);
      return;
    }
    for (const entry of entries) {
      const node = new ExplorerNode(
        this.#ctx, this.#fs, entry, this.#expanded,
        () => this.#schedulePersist(),
      );
      this.#container.appendChild(node.element);
      await node.renderIfExpanded();
    }
  }

  /** @param {string | null} path */
  #setHeader(path) {
    if (path) {
      const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
      this.#header.textContent = name;
      this.#header.title = path;
    } else {
      this.#header.textContent = 'Explorer';
      this.#header.title = 'No project open';
    }
  }

  /** @param {string} msg */
  #showEmpty(msg) {
    const el = document.createElement('div');
    el.className = 'explorer-empty';
    el.textContent = msg;
    this.#container.appendChild(el);
  }

  #schedulePersist() {
    if (this.#saveTimer) clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = null;
      void this.#ctx.storage.set(STORAGE_EXPANDED, [...this.#expanded]);
    }, 200);
  }
}

/** @param {PluginContext} ctx */
export async function activate(ctx) {
  const fs = await ctx.services.get('core.fs');
  if (!fs) throw new Error('core.fs service unavailable');

  const slot = await ctx.ui.slot('sidebar', { scroll: false });
  ctx.subscriptions.add(slot);
  slot.element.replaceChildren();

  const header = document.createElement('div');
  header.className = 'explorer-header';
  header.textContent = 'Explorer';
  slot.element.appendChild(header);

  const tree = document.createElement('div');
  tree.className = 'explorer-tree';
  slot.element.appendChild(tree);

  const explorer = new ExplorerTree(ctx, fs, header, tree);
  await explorer.restore();

  ctx.subscriptions.add(
    ctx.events.on('explorer:openProject', (payload) => {
      if (payload?.path) void explorer.open(payload.path);
    }),
  );
  ctx.subscriptions.add(
    ctx.events.on('explorer:closeProject', () => explorer.close()),
  );

  ctx.subscriptions.add(
    ctx.commands.register('explorer.openProject', (payload) => explorer.open(payload?.path), 'Open project'),
  );
  ctx.subscriptions.add(
    ctx.commands.register('explorer.closeProject', () => explorer.close(), 'Close project'),
  );
}