/** @typedef {import('../../../../src/core/plugin').PluginContext} PluginContext */
/** @typedef {{ start: number; end: number; type: string }} Token */

const LANGUAGE_EXTENSIONS = {
  js: 'javascript', mjs: 'javascript', cjs: 'javascript',
  ts: 'typescript', mts: 'typescript', cts: 'typescript',
  jsx: 'javascript', tsx: 'typescript',
  json: 'json',
  css: 'css', scss: 'css', less: 'css',
  html: 'html', htm: 'html', xml: 'html', svg: 'html',
};

/** @param {string} path */
function detectLanguage(path) {
  const dot = path.lastIndexOf('.');
  if (dot < 0) return 'plain';
  return LANGUAGE_EXTENSIONS[path.slice(dot + 1).toLowerCase()] || 'plain';
}

// ---------------------------------------------------------------------------
// Tokenizers
// ---------------------------------------------------------------------------

class Tokenizer {
  /** @returns {Token[]} */
  tokenize(_text) { return []; }
}

const JS_KEYWORDS = new Set([
  'abstract', 'as', 'async', 'await', 'break', 'case', 'catch', 'class', 'const', 'continue',
  'debugger', 'default', 'delete', 'do', 'else', 'enum', 'export', 'extends', 'false', 'finally',
  'for', 'from', 'function', 'get', 'if', 'implements', 'import', 'in', 'instanceof', 'interface',
  'let', 'new', 'null', 'of', 'package', 'private', 'protected', 'public', 'readonly', 'return',
  'set', 'static', 'super', 'switch', 'this', 'throw', 'true', 'try', 'type', 'typeof', 'undefined',
  'var', 'void', 'while', 'with', 'yield', 'declare', 'keyof', 'infer', 'is', 'never', 'unknown',
  'any', 'boolean', 'number', 'string', 'symbol', 'object', 'bigint',
]);

class JsTokenizer extends Tokenizer {
  tokenize(text) {
    /** @type {Token[]} */
    const out = [];
    let i = 0;
    const n = text.length;
    while (i < n) {
      const c = text[i];
      if (c === '/' && text[i + 1] === '/') {
        const end = text.indexOf('\n', i);
        const stop = end < 0 ? n : end;
        out.push({ start: i, end: stop, type: 'comment' });
        i = stop;
        continue;
      }
      if (c === '/' && text[i + 1] === '*') {
        const end = text.indexOf('*/', i + 2);
        const stop = end < 0 ? n : end + 2;
        out.push({ start: i, end: stop, type: 'comment' });
        i = stop;
        continue;
      }
      if (c === '"' || c === "'" || c === '`') {
        const q = c;
        let j = i + 1;
        while (j < n) {
          if (text[j] === '\\') { j += 2; continue; }
          if (text[j] === q) { j++; break; }
          if (q !== '`' && text[j] === '\n') break;
          j++;
        }
        out.push({ start: i, end: j, type: 'string' });
        i = j;
        continue;
      }
      if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(text[i + 1] || ''))) {
        let j = i;
        while (j < n && /[0-9a-fA-FxXoObBeE._n]/.test(text[j])) j++;
        out.push({ start: i, end: j, type: 'number' });
        i = j;
        continue;
      }
      if (/[A-Za-z_$]/.test(c)) {
        let j = i;
        while (j < n && /[A-Za-z0-9_$]/.test(text[j])) j++;
        const word = text.slice(i, j);
        if (JS_KEYWORDS.has(word)) out.push({ start: i, end: j, type: 'keyword' });
        else if (text[j] === '(') out.push({ start: i, end: j, type: 'function' });
        else out.push({ start: i, end: j, type: 'ident' });
        i = j;
        continue;
      }
      if ('(){}[];,.:?!+-*/%=&|<>^~'.includes(c)) {
        out.push({ start: i, end: i + 1, type: 'punct' });
        i++;
        continue;
      }
      i++;
    }
    return out;
  }
}

class JsonTokenizer extends Tokenizer {
  tokenize(text) {
    /** @type {Token[]} */
    const out = [];
    let i = 0;
    const n = text.length;
    while (i < n) {
      const c = text[i];
      if (/\s/.test(c)) { i++; continue; }
      if (c === '"') {
        let j = i + 1;
        while (j < n) {
          if (text[j] === '\\') { j += 2; continue; }
          if (text[j] === '"') { j++; break; }
          j++;
        }
        let k = j;
        while (k < n && /\s/.test(text[k])) k++;
        out.push({ start: i, end: j, type: text[k] === ':' ? 'key' : 'string' });
        i = j;
        continue;
      }
      if (/[0-9-]/.test(c)) {
        let j = i;
        while (j < n && /[0-9.eE+\-]/.test(text[j])) j++;
        out.push({ start: i, end: j, type: 'number' });
        i = j;
        continue;
      }
      if (text.startsWith('true', i) || text.startsWith('false', i) || text.startsWith('null', i)) {
        const word = text.startsWith('true', i) ? 'true' : text.startsWith('false', i) ? 'false' : 'null';
        out.push({ start: i, end: i + word.length, type: 'literal' });
        i += word.length;
        continue;
      }
      if ('{}[],:'.includes(c)) {
        out.push({ start: i, end: i + 1, type: 'punct' });
        i++;
        continue;
      }
      i++;
    }
    return out;
  }
}

// ===== HtmlTokenizer.tokenize — заменить блок атрибутов =====
class HtmlTokenizer extends Tokenizer {
  tokenize(text) {
    /** @type {Token[]} */
    const out = [];
    let i = 0;
    const n = text.length;
    while (i < n) {
      if (text.startsWith('<!--', i)) {
        const end = text.indexOf('-->', i + 4);
        const stop = end < 0 ? n : end + 3;
        out.push({ start: i, end: stop, type: 'comment' });
        i = stop;
        continue;
      }
      if (text[i] === '<') {
        let j = i + 1;
        if (text[j] === '/') j++;
        const tagStart = j;
        while (j < n && /[A-Za-z0-9:_-]/.test(text[j])) j++;
        if (j > tagStart) {
          out.push({ start: i, end: j, type: 'tag' });
          // Атрибуты: гарантируем прогресс j на каждой итерации
          while (j < n && text[j] !== '>') {
            const c = text[j];
            if (/\s/.test(c)) { j++; continue; }
            if (c === '/' || c === '=') {
              out.push({ start: j, end: j + 1, type: 'punct' });
              j++;
              continue;
            }
            const attrStart = j;
            while (j < n && /[A-Za-z0-9:_-]/.test(text[j])) j++;
            if (j > attrStart) {
              out.push({ start: attrStart, end: j, type: 'attr' });
              let k = j;
              while (k < n && /\s/.test(text[k])) k++;
              if (k < n && text[k] === '=') {
                out.push({ start: k, end: k + 1, type: 'punct' });
                j = k + 1;
                while (j < n && /\s/.test(text[j])) j++;
                if (j < n && (text[j] === '"' || text[j] === "'")) {
                  const q = text[j];
                  let e = j + 1;
                  while (e < n && text[e] !== q) e++;
                  if (e < n) e++;
                  out.push({ start: j, end: e, type: 'string' });
                  j = e;
                } else {
                  const vs = j;
                  while (j < n && !/[\s>]/.test(text[j])) j++;
                  if (j > vs) out.push({ start: vs, end: j, type: 'string' });
                }
              }
            } else {
              j++; // неизвестный символ — принудительный прогресс
            }
          }
          if (text[j] === '>') j++;
          i = j;
          continue;
        }
      }
      i++;
    }
    return out;
  }
}

// ===== CssTokenizer — заменить блок чисел =====
class CssTokenizer extends Tokenizer {
  tokenize(text) {
    /** @type {Token[]} */
    const out = [];
    let i = 0;
    const n = text.length;
    while (i < n) {
      const c = text[i];
      if (c === '/' && text[i + 1] === '*') {
        const end = text.indexOf('*/', i + 2);
        const stop = end < 0 ? n : end + 2;
        out.push({ start: i, end: stop, type: 'comment' });
        i = stop;
        continue;
      }
      if (c === '"' || c === "'") {
        const q = c;
        let j = i + 1;
        while (j < n) {
          if (text[j] === '\\') { j += 2; continue; }
          if (text[j] === q) { j++; break; }
          j++;
        }
        out.push({ start: i, end: j, type: 'string' });
        i = j;
        continue;
      }
      if (c === '#' && /[0-9a-fA-F]/.test(text[i + 1] || '')) {
        // hex-цвет: '#' не в char-классе, начинаем с i+1 — иначе зависание
        let j = i + 1;
        while (j < n && /[0-9a-fA-F]/.test(text[j])) j++;
        out.push({ start: i, end: j, type: 'number' });
        i = j;
        continue;
      }
      if (/[0-9]/.test(c) || (c === '.' && /[0-9]/.test(text[i + 1] || ''))) {
        let j = i;
        while (j < n && /[0-9a-fA-FxX%.]/.test(text[j])) j++;
        out.push({ start: i, end: j, type: 'number' });
        i = j;
        continue;
      }
      if (/[A-Za-z_-]/.test(c)) {
        let j = i;
        while (j < n && /[A-Za-z0-9_-]/.test(text[j])) j++;
        let k = j;
        while (k < n && /\s/.test(text[k])) k++;
        out.push({ start: i, end: j, type: text[k] === ':' ? 'property' : 'selector' });
        i = j;
        continue;
      }
      i++;
    }
    return out;
  }
}

class PlainTokenizer extends Tokenizer {}

class TokenizerRegistry {
  static #map = new Map([
    ['javascript', new JsTokenizer()],
    ['typescript', new JsTokenizer()],
    ['json', new JsonTokenizer()],
    ['css', new CssTokenizer()],
    ['html', new HtmlTokenizer()],
    ['plain', new PlainTokenizer()],
  ]);

  /** @param {string} language */
  static get(language) {
    return this.#map.get(language) ?? this.#map.get('plain');
  }
}

// ---------------------------------------------------------------------------
// Highlighter
// ---------------------------------------------------------------------------

class Highlighter {
  /** @type {Tokenizer} */
  #tokenizer;

  /** @param {string} language */
  constructor(language) {
    this.#tokenizer = TokenizerRegistry.get(language);
  }

  /** @param {string} language */
  setLanguage(language) {
    this.#tokenizer = TokenizerRegistry.get(language);
  }

  /** @param {string} text */
  toHtml(text) {
    const tokens = this.#tokenizer.tokenize(text);
    if (tokens.length === 0) return this.#escape(text);
    /** @type {string[]} */
    const parts = [];
    let cursor = 0;
    for (const t of tokens) {
      if (t.start > cursor) parts.push(this.#escape(text.slice(cursor, t.start)));
      parts.push(`<span class="tok-${t.type}">${this.#escape(text.slice(t.start, t.end))}</span>`);
      cursor = t.end;
    }
    if (cursor < text.length) parts.push(this.#escape(text.slice(cursor)));
    return parts.join('');
  }

  /** @param {string} s */
  #escape(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }
}

// ---------------------------------------------------------------------------
// EditorView
// ---------------------------------------------------------------------------

class EditorView {
  #root;
  #gutter;
  #highlight;
  #input;
  #highlighter;
  #onChange;

  /**
   * @param {string} content
   * @param {string} language
   * @param {(value: string) => void} onChange
   */
  constructor(content, language, onChange) {
    this.#onChange = onChange;
    this.#highlighter = new Highlighter(language);
    this.#root = this.#build();
    this.#gutter = this.#root.querySelector('.aroma-gutter');
    this.#highlight = this.#root.querySelector('.aroma-highlight');
    this.#input = this.#root.querySelector('.aroma-input');
    this.#bind();
    this.setValue(content);
  }

  get element() { return this.#root; }
  getValue() { return this.#input.value; }

  /** @param {string} value */
  setValue(value) {
    this.#input.value = value;
    this.#render();
  }

  focus() { this.#input.focus(); }

  dispose() { this.#root.remove(); }

  #build() {
    const root = document.createElement('div');
    root.className = 'aroma-editor';

    const gutter = document.createElement('div');
    gutter.className = 'aroma-gutter';

    const scroller = document.createElement('div');
    scroller.className = 'aroma-scroller';

    const highlight = document.createElement('pre');
    highlight.className = 'aroma-highlight';
    highlight.setAttribute('aria-hidden', 'true');

    const input = document.createElement('textarea');
    input.className = 'aroma-input';
    input.spellcheck = false;
    input.autocomplete = 'off';
    input.setAttribute('autocorrect', 'off');
    input.setAttribute('autocapitalize', 'off');

    scroller.append(highlight, input);
    root.append(gutter, scroller);
    return root;
  }

  #bind() {
    this.#input.addEventListener('input', () => {
      this.#render();
      this.#onChange(this.#input.value);
    });
    this.#input.addEventListener('scroll', () => {
      this.#highlight.scrollTop = this.#input.scrollTop;
      this.#highlight.scrollLeft = this.#input.scrollLeft;
      this.#gutter.scrollTop = this.#input.scrollTop;
    });
    this.#input.addEventListener('keydown', (e) => {
      if (e.key !== 'Tab') return;
      e.preventDefault();
      const start = this.#input.selectionStart;
      const end = this.#input.selectionEnd;
      const value = this.#input.value;
      this.#input.value = value.slice(0, start) + '  ' + value.slice(end);
      this.#input.selectionStart = this.#input.selectionEnd = start + 2;
      this.#render();
      this.#onChange(this.#input.value);
    });
  }

  #render() {
    const value = this.#input.value;
    this.#highlight.innerHTML = this.#highlighter.toHtml(value) + '\n';
    this.#renderGutter(value);
  }

  /** @param {string} value */
  #renderGutter(value) {
    const lines = value.split('\n').length;
    let html = '';
    for (let i = 1; i <= lines; i++) html += `<div class="aroma-line-no">${i}</div>`;
    this.#gutter.innerHTML = html;
  }
}

// ---------------------------------------------------------------------------
// EditorTab
// ---------------------------------------------------------------------------

class EditorTab {
  #path;
  #name;
  #content;
  #dirty = false;
  #element;
  #onSelect;
  #onClose;

  /**
   * @param {string} path
   * @param {string} name
   * @param {string} content
   * @param {(path: string) => void} onSelect
   * @param {(path: string) => void} onClose
   */
  constructor(path, name, content, onSelect, onClose) {
    this.#path = path;
    this.#name = name;
    this.#content = content;
    this.#onSelect = onSelect;
    this.#onClose = onClose;
    this.#element = this.#build();
  }

  get path() { return this.#path; }
  get content() { return this.#content; }
  get dirty() { return this.#dirty; }
  get element() { return this.#element; }

  setContent(content) { this.#content = content; }

  setActive(active) { this.#element.classList.toggle('active', active); }

  markDirty() {
    if (this.#dirty) return;
    this.#dirty = true;
    this.#element.classList.add('dirty');
  }

  markClean() {
    if (!this.#dirty) return;
    this.#dirty = false;
    this.#element.classList.remove('dirty');
  }

  #build() {
    const el = document.createElement('div');
    el.className = 'editor-tab';
    el.title = this.#path;

    const label = document.createElement('span');
    label.className = 'editor-tab-label';
    label.textContent = this.#name;

    const close = document.createElement('button');
    close.type = 'button';
    close.className = 'editor-tab-close';
    close.textContent = '×';
    close.title = 'Закрыть';
    close.addEventListener('click', (e) => {
      e.stopPropagation();
      this.#onClose(this.#path);
    });

    el.append(label, close);
    el.addEventListener('click', () => this.#onSelect(this.#path));
    return el;
  }
}

// ---------------------------------------------------------------------------
// EditorWorkspace
// ---------------------------------------------------------------------------

class EditorWorkspace {
  #ctx;
  #fs;
  #host;
  #tabsBar;
  #tabs = new Map();
  #activePath = null;
  #view = null;
  #saveTimer = null;

  /** @param {PluginContext} ctx @param {any} fs @param {HTMLElement} host @param {HTMLElement} tabsBar */
  constructor(ctx, fs, host, tabsBar) {
    this.#ctx = ctx;
    this.#fs = fs;
    this.#host = host;
    this.#tabsBar = tabsBar;
    this.#showEmpty();
  }

  /** @param {string} path @param {string} [name] */
  async open(path, name) {
    if (this.#tabs.has(path)) { this.#switchTo(path); return; }
    let content;
    try { content = await this.#fs.readFile(path); }
    catch (e) { console.error('[editor] read failed', path, e); return; }
    const tabName = name || path.split(/[\\/]/).pop() || path;
    const tab = new EditorTab(
      path, tabName, content,
      (p) => this.#switchTo(p),
      (p) => { void this.requestClose(p); },
    );
    this.#tabs.set(path, tab);
    this.#tabsBar.appendChild(tab.element);
    this.#switchTo(path);
  }

  /** @param {string} path */
  async requestClose(path) {
    const tab = this.#tabs.get(path);
    if (!tab) return;
    if (tab.dirty) await this.#writeTab(tab);
    this.close(path);
  }

  /** @param {string} path */
  close(path) {
    const tab = this.#tabs.get(path);
    if (!tab) return;
    tab.element.remove();
    this.#tabs.delete(path);
    if (this.#activePath !== path) return;
    const next = this.#tabs.keys().next().value;
    if (next) this.#switchTo(next);
    else {
      this.#activePath = null;
      this.#view?.dispose();
      this.#view = null;
      this.#host.replaceChildren();
      this.#showEmpty();
    }
  }

  async save() {
    const tab = this.#activePath ? this.#tabs.get(this.#activePath) : null;
    if (tab) await this.#writeTab(tab);
  }

  async saveAll() {
    for (const tab of this.#tabs.values()) {
      if (tab.dirty) await this.#writeTab(tab);
    }
  }

  dispose() {
    if (this.#saveTimer) clearTimeout(this.#saveTimer);
    this.#view?.dispose();
    this.#view = null;
    this.#tabs.clear();
  }

  /** @param {EditorTab} tab */
  async #writeTab(tab) {
    const content = this.#activePath === tab.path && this.#view ? this.#view.getValue() : tab.content;
    try { await this.#fs.writeFile(tab.path, content); }
    catch (e) { console.error('[editor] save failed', tab.path, e); return; }
    tab.setContent(content);
    tab.markClean();
    this.#ctx.events.emit('editor:saved', { path: tab.path });
  }

  #scheduleSave() {
    if (this.#saveTimer) clearTimeout(this.#saveTimer);
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = null;
      void this.saveAll();
    }, 800);
  }

  /** @param {string} path */
  #switchTo(path) {
    if (!this.#tabs.has(path)) return;
    if (this.#activePath && this.#view) {
      const cur = this.#tabs.get(this.#activePath);
      if (cur) cur.setContent(this.#view.getValue());
    }
    for (const t of this.#tabs.values()) t.setActive(t.path === path);
    this.#activePath = path;
    this.#mount(path);
  }

  /** @param {string} path */
  #mount(path) {
    const tab = this.#tabs.get(path);
    if (!tab) return;
    this.#view?.dispose();
    this.#host.replaceChildren();
    this.#view = new EditorView(tab.content, detectLanguage(path), () => {
      this.#tabs.get(path)?.markDirty();
      this.#scheduleSave();
    });
    this.#host.appendChild(this.#view.element);
    this.#view.focus();
  }

  #showEmpty() {
    const empty = document.createElement('div');
    empty.className = 'editor-empty';
    empty.textContent = 'No file open';
    this.#host.appendChild(empty);
  }
}

// ---------------------------------------------------------------------------
// Plugin entry
// ---------------------------------------------------------------------------

/** @param {PluginContext} ctx */
export async function activate(ctx) {
  const fs = await ctx.services.get('core.fs');
  if (!fs) throw new Error('core.fs service unavailable');

  const slot = await ctx.ui.slot('main', { scroll: false });
  ctx.subscriptions.add(slot);
  slot.element.replaceChildren();

  const tabsBar = document.createElement('div');
  tabsBar.className = 'editor-tabs';
  slot.element.appendChild(tabsBar);

  const host = document.createElement('div');
  host.className = 'editor-host';
  slot.element.appendChild(host);

  const workspace = new EditorWorkspace(ctx, fs, host, tabsBar);
  ctx.subscriptions.add({ dispose: () => workspace.dispose() });

  ctx.subscriptions.add(
    ctx.events.on('explorer:fileOpen', (payload) => {
      if (payload?.path) void workspace.open(payload.path, payload.name);
    }),
  );

  ctx.subscriptions.add(
    ctx.commands.register('editor.save', () => workspace.save(), 'Save active file'),
  );
  ctx.subscriptions.add(
    ctx.commands.register('editor.saveAll', () => workspace.saveAll(), 'Save all files'),
  );
}