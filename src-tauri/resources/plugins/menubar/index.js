/**
 * @typedef {Object} MenuItemSpec
 * @property {string} label
 * @property {string} command
 */
/**
 * @typedef {Object} MenuSpec
 * @property {string} label
 * @property {MenuItemSpec[]} items
 */
/** @typedef {import('../../../../src/core/plugin').PluginContext} PluginContext */

const PLUGIN_ID = 'menubar';
const ABOUT_COMMAND = 'menubar.about';
const OPEN_PROJECT_COMMAND = 'menubar.file.openProject';
const CLOSE_PROJECT_COMMAND = 'menubar.file.closeProject';
const SAVE_COMMAND = 'menubar.file.save';
const SAVE_ALL_COMMAND = 'menubar.file.saveAll';

/** @type {MenuSpec[]} */
const DEFAULT_MENUS = [
  {
    label: 'Файл',
    items: [
      { label: 'Открыть проект…', command: OPEN_PROJECT_COMMAND },
      { label: 'Закрыть проект', command: CLOSE_PROJECT_COMMAND },
      { label: 'Сохранить', command: SAVE_COMMAND },
      { label: 'Сохранить всё', command: SAVE_ALL_COMMAND },
    ],
  },
  {
    label: 'Правка',
    items: [
      { label: 'Отменить', command: 'menubar.edit.undo' },
      { label: 'Повторить', command: 'menubar.edit.redo' },
    ],
  },
  {
    label: 'Помощь',
    items: [{ label: 'Авторство', command: ABOUT_COMMAND }],
  },
];

/**
 * Срезает парные кавычки по краям — Windows «Copy as path» кладёт путь в "...".
 * @param {string} raw
 */
function cleanPath(raw) {
  const s = raw.trim();
  if (s.length < 2) return s;
  const first = s[0];
  const last = s[s.length - 1];
  if ((first === '"' || first === "'" || first === '`') && first === last) {
    return s.slice(1, -1).trim();
  }
  return s;
}

/**
 * Устойчиво определяет, что stat описывает директорию.
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
    stat.kind === 'dir'
  );
}

/**
 * Открывает нативный диалог выбора папки. Возвращает абсолютный путь или null.
 * @param {string} title
 * @returns {Promise<string | null>}
 */
async function pickNativeFolder(title) {
  const core = /** @type {any} */ (window).__TAURI__?.core;
  if (!core?.invoke) return null;
  try {
    const result = await core.invoke('plugin:dialog|open', {
      options: { directory: true, multiple: false, title },
    });
    if (typeof result === 'string') return result;
    if (Array.isArray(result) && typeof result[0] === 'string') return result[0];
    return null;
  } catch (e) {
    console.warn('[menubar] native dialog failed', e);
    return null;
  }
}

/** @type {MenuBar | null} */
let activeBar = null;

export const api = {
  /** @param {string} label */
  open: (label) => activeBar?.openByLabel(label) ?? false,
  closeAll: () => activeBar?.closeAll(),
  listMenus: () =>
    (activeBar?.getMenuLabels() ?? []).map(({ label, itemCount }) => ({ label, itemCount })),
};

// ---------------------------------------------------------------------------
// Modal
// ---------------------------------------------------------------------------

class Modal {
  /** @type {{ title: string; body?: string; input?: { label: string; value?: string } }} */
  #opts;

  /** @param {{ title: string; body?: string; input?: { label: string; value?: string } }} opts */
  constructor(opts) {
    this.#opts = opts;
  }

  /** @param {HTMLElement} container @returns {Promise<string | null>} */
  open(container) {
    return new Promise((resolve) => {
      const root = this.#build(resolve);
      container.appendChild(root);
      const input = /** @type {HTMLInputElement | null} */ (root.querySelector('input'));
      if (input) {
        input.focus();
        input.select();
      }
    });
  }

  /** @param {(value: string | null) => void} resolve */
  #build(resolve) {
    const opts = this.#opts;
    const root = document.createElement('div');
    root.className = 'menubar-modal';
    root.dataset.pluginId = PLUGIN_ID;

    const card = document.createElement('div');
    card.className = 'menubar-modal-card';

    const h = document.createElement('h2');
    h.textContent = opts.title;
    card.appendChild(h);

    if (opts.body) {
      const p = document.createElement('p');
      p.textContent = opts.body;
      card.appendChild(p);
    }

    /** @type {HTMLInputElement | null} */
    let input = null;
    if (opts.input) {
      const lbl = document.createElement('p');
      lbl.textContent = opts.input.label;
      card.appendChild(lbl);
      input = document.createElement('input');
      input.type = 'text';
      input.className = 'menubar-input';
      input.value = opts.input.value ?? '';
      card.appendChild(input);
    }

    const actions = document.createElement('div');
    actions.className = 'menubar-modal-actions';

    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.className = 'menubar-modal-btn';
    cancel.textContent = 'Отмена';

    const okBtn = document.createElement('button');
    okBtn.type = 'button';
    okBtn.className = 'menubar-modal-btn primary';
    okBtn.textContent = opts.input ? 'Открыть' : 'OK';

    /** @param {string | null} value */
    const finish = (value) => {
      root.remove();
      resolve(value);
    };

    cancel.addEventListener('click', () => finish(null));
    okBtn.addEventListener('click', () => finish(input ? input.value.trim() || null : ''));
    root.addEventListener('mousedown', (e) => {
      if (e.target === root) finish(null);
    });
    if (input) {
      input.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') finish(input.value.trim() || null);
        else if (e.key === 'Escape') finish(null);
      });
    }

    actions.append(cancel, okBtn);
    card.appendChild(actions);
    root.appendChild(card);
    return root;
  }
}

// ---------------------------------------------------------------------------
// Dropdown
// ---------------------------------------------------------------------------

class Dropdown {
  /** @type {HTMLElement} */
  #element;

  constructor() {
    this.#element = document.createElement('div');
    this.#element.className = 'menubar-dropdown';
    this.#element.dataset.pluginId = PLUGIN_ID;
    this.#element.hidden = true;
  }

  /** @param {MenuItem} item */
  add(item) {
    this.#element.appendChild(item.render());
  }

  /** @param {HTMLElement} anchor */
  open(anchor) {
    const r = anchor.getBoundingClientRect();
    this.#element.style.left = `${r.left}px`;
    this.#element.style.top = `${r.bottom}px`;
    this.#element.hidden = false;
    document.body.appendChild(this.#element);
  }

  close() {
    this.#element.hidden = true;
    this.#element.remove();
  }
}

// ---------------------------------------------------------------------------
// MenuItem / Menu / MenuBar
// ---------------------------------------------------------------------------

class MenuItem {
  /** @type {MenuItemSpec} */
  #spec;
  /** @type {(command: string) => void} */
  #onClick;

  /** @param {MenuItemSpec} spec @param {(command: string) => void} onClick */
  constructor(spec, onClick) {
    this.#spec = spec;
    this.#onClick = onClick;
  }

  render() {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'menubar-item';
    btn.textContent = this.#spec.label;
    btn.addEventListener('mousedown', (e) => e.preventDefault());
    btn.addEventListener('click', () => this.#onClick(this.#spec.command));
    return btn;
  }
}

class Menu {
  /** @type {MenuBar} */
  #bar;
  /** @type {HTMLElement} */
  #trigger;
  /** @type {Dropdown} */
  #dropdown;
  /** @type {string} */
  #label;
  /** @type {number} */
  #itemCount;

  /** @param {MenuBar} bar @param {MenuSpec} spec @param {(command: string) => void} execute */
  constructor(bar, spec, execute) {
    this.#bar = bar;
    this.#label = spec.label;
    this.#itemCount = spec.items.length;
    this.#trigger = this.#buildTrigger(spec.label);
    this.#dropdown = new Dropdown();
    for (const item of spec.items) this.#dropdown.add(new MenuItem(item, execute));
  }

  get trigger() { return this.#trigger; }
  get label() { return this.#label; }
  get itemCount() { return this.#itemCount; }

  show() { this.#dropdown.open(this.#trigger); }
  hide() { this.#dropdown.close(); }

  destroy() {
    this.#trigger.remove();
    this.#dropdown.close();
  }

  /** @param {string} label */
  #buildTrigger(label) {
    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'menubar-trigger';
    btn.textContent = label;
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      this.#bar.toggle(this);
    });
    return btn;
  }
}

class MenuBar {
  /** @type {HTMLElement} */
  #root;
  /** @type {Menu[]} */
  #menus = [];
  /** @type {Menu | null} */
  #open = null;
  /** @type {(command: string) => void} */
  #execute;

  /** @param {(command: string) => void} execute */
  constructor(execute) {
    this.#execute = execute;
    this.#root = document.createElement('div');
    this.#root.className = 'menubar-bar';
  }

  get element() { return this.#root; }

  /** @param {MenuSpec} spec */
  addMenu(spec) {
    const menu = new Menu(this, spec, this.#execute);
    this.#menus.push(menu);
    this.#root.appendChild(menu.trigger);
    return {
      dispose: () => {
        const i = this.#menus.indexOf(menu);
        if (i >= 0) this.#menus.splice(i, 1);
        if (this.#open === menu) this.closeAll();
        menu.destroy();
      },
    };
  }

  /** @param {Menu} menu */
  toggle(menu) {
    if (this.#open === menu) { this.closeAll(); return; }
    this.closeAll();
    menu.show();
    this.#open = menu;
  }

  closeAll() {
    this.#open?.hide();
    this.#open = null;
  }

  /** @param {string} label */
  openByLabel(label) {
    const menu = this.#menus.find((m) => m.label === label);
    if (!menu) return false;
    this.toggle(menu);
    return true;
  }

  getMenuLabels() {
    return this.#menus.map((m) => ({ label: m.label, itemCount: m.itemCount }));
  }
}

// ---------------------------------------------------------------------------
// Plugin entry
// ---------------------------------------------------------------------------

/** @param {PluginContext} ctx */
export async function activate(ctx) {
  const fs = await ctx.services.get('core.fs');

  const topbar = await ctx.ui.slot('topbar');
  ctx.subscriptions.add(topbar);

  const overlay = await ctx.ui.slot('overlay', { tier: 'modal' });
  ctx.subscriptions.add(overlay);

  const bar = new MenuBar((id) => {
    bar.closeAll();
    void ctx.commands.execute(id);
  });
  topbar.element.replaceChildren(bar.element);
  activeBar = bar;

  ctx.subscriptions.add(
    ctx.commands.register(OPEN_PROJECT_COMMAND, async () => {
      const picked = await pickNativeFolder('Открыть проект');
      let raw = picked;

      if (picked === null) {
        // Fallback: ручной ввод (например, `npm run dev` в браузере без Tauri).
        const defaultPath = fs ? await fs.homeDir().catch(() => '') : '';
        raw = await new Modal({
          title: 'Открыть проект',
          body: 'Укажите путь к папке проекта:',
          input: { label: 'Путь', value: defaultPath },
        }).open(overlay.element);
      }
      if (!raw) return;

      const path = cleanPath(raw);
      if (!path) return;

      if (fs) {
        let stat;
        try {
          stat = await fs.stat(path);
        } catch (e) {
          await new Modal({
            title: 'Ошибка',
            body: `Не удалось проверить путь:\n${path}\n\n${String(e)}`,
          }).open(overlay.element);
          return;
        }
        if (!isDirectoryStat(stat)) {
          await new Modal({
            title: 'Ошибка',
            body: `Это не папка:\n${path}\n\nОтвет ядра: ${JSON.stringify(stat)}`,
          }).open(overlay.element);
          return;
        }
      }

      ctx.events.emit('explorer:openProject', { path });
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register(CLOSE_PROJECT_COMMAND, () => {
      ctx.events.emit('explorer:closeProject');
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register(SAVE_COMMAND, async () => {
      if (ctx.commands.has('editor.save')) await ctx.commands.execute('editor.save');
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register(SAVE_ALL_COMMAND, async () => {
      if (ctx.commands.has('editor.saveAll')) await ctx.commands.execute('editor.saveAll');
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register('menubar.edit.undo', () => {
      console.warn('[menubar] edit.undo: not implemented');
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register('menubar.edit.redo', () => {
      console.warn('[menubar] edit.redo: not implemented');
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register(ABOUT_COMMAND, async () => {
      await new Modal({ title: 'Авторство', body: 'Агафонов Роман' }).open(overlay.element);
    }),
  );

  for (const menuDef of DEFAULT_MENUS) {
    ctx.subscriptions.add(bar.addMenu(menuDef));
  }

  const onDocClick = () => bar.closeAll();
  document.addEventListener('click', onDocClick);
  ctx.subscriptions.add({
    dispose: () => document.removeEventListener('click', onDocClick),
  });
}

export function deactivate() {
  activeBar?.closeAll();
  activeBar = null;
  for (const el of document.querySelectorAll('.menubar-modal')) el.remove();
}