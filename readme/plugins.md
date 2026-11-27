# Справочник по API плагинов

Версия документа: 0.1.0
Статус API: ранний, возможны ломающие изменения.

Краткое введение — в [plugins-quick.md](plugins-quick.md). Здесь — полный контракт.

---

## Содержание

1. [Что такое плагин](#1-что-такое-плагин)
2. [Где лежат плагины](#2-где-лежат-плагины)
3. [Структура](#3-структура)
4. [Манифест](#4-манифест-pluginjson)
5. [Entry-модуль](#5-entry-модуль)
6. [PluginContext](#6-plugincontext)
7. [Сервисы ядра](#7-сервисы-ядра)
8. [IO между плагинами](#8-io-между-плагинами)
9. [Capabilities](#9-capabilities)
10. [Зависимости и порядок активации](#10-зависимости-и-порядок-активации)
11. [Версионирование](#11-версионирование-в-requires)
12. [Стили](#12-стили)
13. [Ограничения](#13-ограничения)
14. [Полный пример](#14-полный-пример)
15. [Чек-лист](#15-чек-лист-автора-плагина)

---

## 1. Что такое плагин

Плагин — папка с манифестом `plugin.json` и входным JS-модулем (`index.js` по умолчанию). Хост обнаруживает плагины, активирует их в порядке зависимостей и выдаёт каждому изолированный `PluginContext` — доступ к командам, событиям, UI, хранилищу, IO, capabilities и сервисам ОС.

Ядро построено на `Disposable`. Всё, что плагин регистрирует (подписки, команды, слоты, порты, capability), освобождается при деактивации — если плагин положил это в `ctx.subscriptions`.

---

## 2. Где лежат плагины

Хост сканирует четыре корня и перекрывает по `id` в порядке приоритета — **позже найденный выигрывает**:

| Корень | Путь | Назначение |
|--------|------|-----------|
| `bundled` | `<resource_dir>/plugins/<id>/` | Поставляются с приложением |
| `exe_sibling` | `<папка .exe>/plugins/<id>/` | Portable-сборка |
| `user` | `%APPDATA%\com.aroma.ide\plugins\<id>\` | Устанавливает пользователь |
| `dev` | `<cwd>/plugins/<id>/` | Только в debug-сборке |

> **Имя папки должно совпадать с `id`.** Иначе плагин отклоняется, а причина видна в `plugins_diagnostics`.

Важно: для плагина выбирается **один** корень-победитель, и все его файлы читаются оттуда. Не бывает так, что `plugin.json` из `dev`, а `index.js` из `user`.

Установка = скопировать папку. Удаление = удалить папку. Правка `index.js` подхватывается на лету.

---

## 3. Структура

```
plugins/
└── my-plugin/
    ├── plugin.json      # манифест (обязателен)
    ├── index.js         # entry-модуль (имя задаётся полем entry)
    └── style.css        # стили (имя задаётся полем style, опционально)
```

Хост читает только эти файлы. Относительные импорты не поддерживаются (см. [§13](#13-ограничения)).

---

## 4. Манифест `plugin.json`

```json
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "1.0.0",
  "description": "Короткое описание",
  "entry": "index.js",
  "style": "style.css",
  "trust": "trusted",
  "inputs":  [{ "id": "setValue", "description": "Задать значение" }],
  "outputs": [{ "id": "valueChanged", "description": "Значение изменилось" }],
  "provides": [{ "id": "my.api", "version": "1.0.0", "description": "API плагина" }],
  "requires": [{ "id": "editor.api", "version": "^2.0.0", "optional": false }]
}
```

### Поля

| Поле | Обязательно | Описание |
|------|-------------|----------|
| `id` | ✅ | Уникальный. Совпадает с именем папки. Без `/`, `\`, `:`, `..` |
| `name` | ✅ | Отображаемое имя |
| `version` | ✅ | SemVer (`1.2.3`), используется при проверке зависимостей |
| `description` | | Произвольный текст |
| `entry` | | Имя entry-файла. По умолчанию `index.js` |
| `style` | | Имя CSS-файла. По умолчанию `style.css`. Может отсутствовать |
| `trust` | | `trusted` (по умолчанию) или `sandboxed`. **Sandboxed пока не поддерживается** |
| `inputs` | | Входные порты IO ([§8](#8-io-между-плагинами)) |
| `outputs` | | Выходные порты IO ([§8](#8-io-между-плагинами)) |
| `provides` | | Capabilities, которые плагин публикует ([§9](#9-capabilities)) |
| `requires` | | Capabilities, которые плагину нужны |

### Что проверяет ядро

Манифест валидируется **до** загрузки кода. Отклоняется, если:

- некорректный JSON или не объект;
- `id` пустой, содержит разделители или не совпадает с именем папки;
- `name` пустой;
- `entry`/`style` — абсолютный путь или содержит `..`;
- `trust` не `trusted`/`sandboxed`;
- файл из `entry` не существует.

Такой плагин не попадает в список, а причина возвращается командой `plugins_diagnostics`.

---

## 5. Entry-модуль

`index.js` — ES-модуль. Экспортирует один или оба символа:

```js
export async function activate(ctx) {
  // инициализация: команды, слоты, IO, capabilities
}

export function deactivate() {
  // опционально; всё из ctx.subscriptions уже будет освобождено
}
```

- `activate` может быть асинхронной. Если бросит исключение — плагин откатится целиком.
- `deactivate` вызывается **до** авто-dispose `ctx.subscriptions`.

Модуль грузится как blob-URL: **один самодостаточный файл**, без относительных импортов.

---

## 6. `PluginContext`

`ctx` в `activate(ctx)` — замороженный снимок API. Все поля read-only.

```ts
interface PluginContext {
  readonly manifest: Readonly<PluginManifest>;
  readonly commands: PluginCommandsApi;
  readonly events: PluginEventsApi;
  readonly ui: PluginUiApi;
  readonly services: PluginServicesApi;
  readonly storage: PluginStorageApi;
  readonly io: PluginIoApi;
  readonly capabilities: PluginCapabilitiesApi;
  readonly subscriptions: PluginSubscriptions;
}
```

Каждый метод, возвращающий `Disposable`, регистрируйте в `ctx.subscriptions.add(...)` — иначе не освободится при деактивации.

```js
export function activate(ctx) {
  ctx.subscriptions.add(ctx.events.on('my-event', (p) => console.log(p)));
}
```

### 6.1. `ctx.commands` — реестр команд

```js
ctx.commands.register('my.doSomething', async (args) => {
  return { done: true };
}, 'Что делает команда');

const result = await ctx.commands.execute('other.command', { x: 1 });
if (ctx.commands.has('other.command')) { /* ... */ }
```

Ошибки в обработчике **глушатся** (логируются) и превращаются в `undefined`. Важные сбои сообщайте через `ctx.events.emit`.

### 6.2. `ctx.events` — шина событий

```js
ctx.subscriptions.add(ctx.events.on('editor:open', ({ path }) => {
  // ...
}));

ctx.events.emit('my-plugin:ready', { version: '1.0.0' });
```

Падение подписчика не роняет остальных и не роняет ядро.

### 6.3. `ctx.ui` — слоты в регионах

```js
const slot = await ctx.ui.slot('sidebar', { tier: 'normal', scroll: true });
const btn = slot.create('button');
btn.textContent = 'Click';
btn.addEventListener('click', () => ctx.events.emit('my:clicked'));
slot.element.appendChild(btn);
ctx.subscriptions.add(slot);
```

**Регионы:** `topbar`, `sidebar`, `main`, `bottom`, `statusbar`, `overlay`.

**Tier (z-index):** `background` < `normal` < `raised` < `overlay` < `modal` < `system`.
По умолчанию: для `overlay` — `overlay`, для остальных — `normal`.

**Опции слота:**

| Опция | Значения | По умолчанию |
|-------|----------|--------------|
| `position` | `relative` \| `fixed` | `relative` |
| `tier` | см. выше | `normal` / `overlay` |
| `scroll` | `boolean` | `true` |

`fixed` центрирует слот поверх окна (модалки, диалоги).

Вложенные слоты — `slot.addSlot(options)`, вернёт `SlotHandle` внутри текущего.

`SlotHandle`: `id`, `region`, `element`, `tier`, `zIndex`, `create(tag)`, `addSlot(opts)`, `dispose()`.

### 6.4. `ctx.services` — сервисы ядра

```js
const fs     = await ctx.services.get('core.fs');    // FsService
const tools  = await ctx.services.get('fs.tools');   // FsToolsService
const pty    = await ctx.services.get('pty');        // PtyService
const proc   = await ctx.services.get('proc');       // ProcService
const serial = await ctx.services.get('serial');     // SerialService
const vendor = await ctx.services.get('vendor.xterm'); // { Terminal, FitAddon }
```

Подробности — в [§7](#7-сервисы-ядра).

### 6.5. `ctx.storage` — персистентное хранилище

Namespace изолирован: `plugin.<id>`. Формат — `key → JSON-значение`.

```js
await ctx.storage.set('lastFile', '/tmp/a.ts');
const p = await ctx.storage.get('lastFile', null);
const keys = await ctx.storage.keys();
await ctx.storage.delete('lastFile');
```

Запись атомарна. Если файл повреждён, он откладывается в `.corrupt-*.json`, а IDE стартует с пустыми настройками вместо падения.

### 6.6. `ctx.io` — входы/выходы между плагинами

Порты **объявляются в манифесте**, хост проверяет декларации.

```js
// Публикация (output должен быть в manifest.outputs)
const r = await ctx.io.publish('valueChanged', { value: 42 });
// r.code === 200 если кому-то доставили, 202 если никто не подписан

// Приём на свой input (должен быть в manifest.inputs)
ctx.subscriptions.add(
  await ctx.io.onInput('setValue', async (value) => {
    return { code: 200 };
  })
);

// Подписка на чужой output
ctx.subscriptions.add(
  await ctx.io.subscribe('other-plugin', 'someOutput', (v) => { /* ... */ })
);

// Отправить значение в чужой input
const status = await ctx.io.inject('other-plugin', 'someInput', { x: 1 });
```

### 6.7. `ctx.capabilities` — публикация API

```js
// Публикация: id должен быть в manifest.provides
await ctx.capabilities.provide('editor.api', {
  openFile: async (p) => { /* ... */ },
  getCurrent: () => ({ path: '/a.ts' }),
});

// Получить конкретного провайдера
const handle = await ctx.capabilities.handle('editor', 'editor.api');
if (handle?.alive) handle.api.openFile('/b.ts');

// Первый живой провайдер по id capability
const any = await ctx.capabilities.find('editor.api');

// Все провайдеры
const all = await ctx.capabilities.all('editor.api');

// Список описателей
const descriptors = await ctx.capabilities.list();

// «Инъекция»: mutate api провайдера, вернув Disposable
await ctx.capabilities.inject('editor', 'editor.api', (api) => {
  const off = api.hook(() => { /* ... */ });
  return off; // освободится при удалении capability
});

// Подписка на исчезновение capability
ctx.subscriptions.add(handle.onRemoved(() => {
  console.warn('editor.api ушёл');
}));
```

### 6.8. `ctx.subscriptions`

`ctx.subscriptions.add(d)` — регистрирует disposable под жизненный цикл плагина. `ctx.subscriptions.size` — количество.

### 6.9. Результаты операций

Методы `io` и `capabilities` возвращают `PluginStatusResult`:

```ts
{ code: number; message?: string; value?: unknown; deliveredTo?: number }
```

| Код | Константа | Смысл |
|-----|-----------|-------|
| 200 | `OK` | Успех |
| 202 | `ACCEPTED` | Принято, но никто не слушал (IO) |
| 204 | `NO_CONTENT` | Пусто |
| 400 | `BAD_REQUEST` | Некорректный запрос / не объявлено в манифесте |
| 404 | `NOT_FOUND` | Порт/провайдер отсутствует |
| 409 | `CONFLICT` | Дубликат (capability уже предоставлена) |
| 424 | `DEPENDENCY_MISSING` | Не хватает зависимости |
| 500 | `INTERNAL_ERROR` | Обработчик упал |
| 503 | `NOT_AVAILABLE` | Временно недоступно |

---

## 7. Сервисы ядра

Сервисы — это доступ к ОС. В отличие от IO и capabilities, они не «между плагинами», а «между плагином и системой».

### 7.1. Ошибки сервисов

Все методы бросают строку вида `КОД: сообщение`, где сообщение содержит путь:

```js
try {
  await fs.readFile(path);
} catch (e) {
  if (String(e).startsWith('NOT_UTF8:')) {
    // это бинарь — читаем другим каналом
  }
}
```

| Код | Смысл |
|-----|-------|
| `NOT_FOUND` | файла/плагина нет |
| `PERMISSION_DENIED` | ОС не даёт доступ |
| `ALREADY_EXISTS` | цель уже существует |
| `NOT_A_DIRECTORY` / `IS_A_DIRECTORY` | не тот тип |
| `NOT_UTF8` | файл не текст — используйте бинарный канал |
| `TOO_LARGE` | превышен лимит чтения |
| `INVALID_PATH` | путь отвергнут проверкой (побег, ADS, абсолютный) |
| `INVALID_ARGUMENT` | некорректные данные вызова |
| `CROSS_DEVICE` | операция между разными томами |
| `BUSY` | файл занят другим процессом |
| `UNSUPPORTED` | операция не поддерживается |
| `INTERNAL` | внутренняя ошибка IDE |
| `UNKNOWN` | всё остальное |

### 7.2. `core.fs` — файлы

| Метод | Возвращает |
|-------|-----------|
| `readFile(path)` | `string` |
| `readText(path)` | `{ content, size, mtime, encoding, eol }` |
| `readBinary(path)` | `{ base64, size, mtime }` |
| `writeFile(path, contents)` | `Stat` |
| `writeBinary(path, base64)` | `Stat` |
| `appendFile(path, contents)` | `void` |
| `readDir(path)` | `DirEntry[]` (каталоги первыми) |
| `stat(path)` | `Stat \| null` |
| `exists(path)` | `boolean` |
| `mkdir(path, recursive?)` | `void` |
| `remove(path, recursive?)` | `void` |
| `rename(from, to, overwrite?)` | `void` |
| `copy(from, to, overwrite?)` | `void` |
| `canonicalize(path)` | `string` |
| `homeDir()` | `string` |
| `appDirs()` | `{ home, config, data, localData, cache, log, resource, exe, cwd, desktop, documents, temp }` |
| `reveal(path)` | показать в проводнике |
| `open(path)` | открыть программой по умолчанию |

```ts
interface DirEntry {
  name: string; path: string;
  isDir: boolean; isFile: boolean; isSymlink: boolean; isHidden: boolean;
  size: number; mtime: number | null;
}

interface Stat {
  isFile: boolean; isDir: boolean; isSymlink: boolean;
  size: number; mtime: number | null; created: number | null;
  readonly: boolean;
}
```

`mtime` и `created` — **миллисекунды** UNIX epoch.

`TextFile.encoding` — `utf-8`, `utf-8-bom` или `utf-8-lossy` (файл пришлось починить). `eol` — `\n` или `\r\n`.

**Гарантии:**

- запись атомарна (temp → `fsync` → `rename`), поэтому сбой питания не оставит обрезанный файл;
- `stat` отличает «нет файла» (`null`) от «нет прав» (ошибка);
- удаление корня диска, домашнего каталога и каталога настроек IDE отклоняется;
- ссылки удаляются как ссылки, а не как каталог, на который они указывают.

Для совместимости со старыми плагинами `DirEntry` и `Stat` дополнительно содержат snake_case-поля (`is_dir`, `is_file`, `is_symlink`).

### 7.3. `fs.tools` — поиск и наблюдение

```js
const result = await tools.search({
  query: 'useState',
  roots: [projectPath],
  include: ['*.ts', '*.tsx'],
  exclude: ['*.min.js'],
  caseSensitive: false,
  regex: false,
  wholeWord: false,
  contextLines: 2,
  maxResults: 2000,
});

// result.matches[i] = { path, line, column, text, matchStart, matchEnd, before, after }
// result.truncated — найдено больше, чем поместилось в лимит
// result.errors — каталоги, которые не удалось прочитать
```

```js
const sub = await tools.watch(projectPath, (changes) => {
  for (const { kind, path } of changes) {
    // kind: 'created' | 'modified' | 'removed' | 'accessed'
  }
}, { recursive: true, debounceMs: 120 });
ctx.subscriptions.add(sub);
```

События **пакетируются** по паузе тишины: сохранение одного файла даёт один вызов, а не четыре.

### 7.4. `pty` — терминал

```js
const proc = await pty.spawn({ cwd: projectPath, cols: 120, rows: 30 });

proc.onData((chunk) => term.write(chunk));
proc.onExit((code) => term.write(`\r\n[exit ${code}]\r\n`));

term.onData((data) => void proc.write(data));
void proc.resize(term.cols, term.rows);

ctx.subscriptions.add(proc);
const history = await proc.replay();   // последние ~512 КБ вывода
```

| Метод | |
|-------|---|
| `spawn(options?)` | `{ shell?, args?, cwd?, env?, cols?, rows? }` → `PtyProcess` |
| `list()` | `PtyInfo[]` |

`PtyProcess`: `id`, `pid`, `write(data)`, `resize(cols, rows)`, `kill()`, `replay()`, `onData`, `onExit`, `onError`, `dispose()`.

UTF-8 декодируется в ядре, поэтому русский текст и эмодзи не превращаются в «крякозябры», даже если символ разорван между чанками. `list()` возвращает и завершённые сессии (`alive: false`) — их вывод ещё доступен через `replay()`.

### 7.5. `proc` — процессы без PTY

Для сборки, линтеров и CLI-тулчейнов. Нет интерактивности, но есть код возврата.

```js
const build = await proc.spawn({
  program: 'cargo',
  args: ['build'],
  cwd: projectPath,
  shell: false,     // true — запустить через cmd /C или sh -c
  stdin: false,
});

build.onData((chunk) => output.append(chunk));
const code = await build.done;    // код возврата

if (!(await proc.which('cargo'))) {
  // компилятор не установлен — покажите это, а не os error 2
}
```

`stdout` и `stderr` сводятся в один поток: для сборки важен общий порядок.

`ProcHandle`: `id`, `pid`, `write(data)`, `kill()`, `replay()`, `done`, `onData`, `onExit`, `onError`, `dispose()`.

### 7.6. `serial` — платы и контроллеры

```js
const ports = await serial.listPorts();
// [{ name: 'COM3', kind: 'usb', manufacturer: 'Arduino', product, serialNumber, vid, pid }]

const port = await serial.open('COM3', {
  baudRate: 115200,
  dataBits: 8,
  parity: 'none',        // 'none' | 'odd' | 'even'
  stopBits: 1,           // 1 | 2
  flowControl: 'none',   // 'none' | 'hardware' | 'software'
  timeoutMs: 20,
});

port.onData((bytes) => { /* Uint8Array — протоколы МК бинарные */ });
await port.write(new Uint8Array([0x01, 0x02]));
await port.setSignals({ dtr: false, rts: true });   // сброс в загрузчик
ctx.subscriptions.add(port);
```

`SerialHandle`: `id`, `portName`, `write(Uint8Array)`, `setSignals({dtr?, rts?})`, `close()`, `onData`, `onExit`, `onError`, `dispose()`.

### 7.7. `vendor.xterm`

`{ Terminal, FitAddon }` из `@xterm/xterm` — чтобы плагин терминала не тащил свою копию.

---

## 8. IO между плагинами

- **`outputs`** — то, что плагин публикует (`publish`). Только объявленные в манифесте.
- **`inputs`** — то, что плагин принимает (`onInput`). Только объявленные в манифесте.
- **`subscribe` / `inject`** — кросс-плагин, проверяются на стороне цели.

`publish` возвращает `deliveredTo` — количество доставок. Если `0`, `code === 202`.

Падение одного слушателя не мешает остальным: ошибка логируется, доставка продолжается.

---

## 9. Capabilities

Capability — именованный контракт с версией. Провайдер публикует объект, потребитель получает handle.

```json
"provides": [{ "id": "editor.api", "version": "1.2.0", "description": "Управление редактором" }],
"requires": [{ "id": "editor.api", "version": "^1.0.0" }]
```

```js
await ctx.capabilities.provide('editor.api', { openFile, getCurrent });
const handle = await ctx.capabilities.find('editor.api');
handle.api.openFile('/a.ts');
```

`CapabilityHandle`: `pluginId`, `capabilityId`, `version`, `api`, `alive`, `onRemoved(fn)`, `dispose()`.

Обращение к `api` после исчезновения провайдера бросает исключение — проверяйте `alive` или подпишитесь на `onRemoved`.

---

## 10. Зависимости и порядок активации

Хост строит граф по `requires` → `provides` с проверкой semver.

- Плагины активируются в **топологическом порядке** (провайдеры раньше потребителей).
- **Циклы** — ошибка активации.
- **Неразрешённые зависимости** — плагин пропускается с предупреждением, остальные активируются.
- **`optional: true`** — не блокирует активацию.

При деактивации провайдера хост:

1. эмитит `plugin:dependencyLost` с `{ provider, dependents }`;
2. каскадно деактивирует зависимые плагины, потерявшие обязательную capability.

Ядро при этом не падает.

---

## 11. Версионирование в `requires`

```json
"requires": [{ "id": "editor.api", "version": "^2.0.0" }]
```

| Запись | Смысл |
|--------|-------|
| без `version` | любая доступная |
| `^1.2.3` | major совпадает, не меньше |
| `~1.2.3` | major и minor совпадают, не меньше |
| `>=1.0.0`, `>1.0.0`, `<=2.0.0`, `<2.0.0` | сравнения |
| `*`, `x`, `""` | любая |

---

## 12. Стили

Если рядом с `index.js` есть `style.css` (или файл из `style`), он вставляется в `<head>` как `<style data-plugin-id="my-plugin">` при активации и удаляется при деактивации.

Префиксуйте селекторы, чтобы не задеть чужие плагины:

```css
[data-plugin-id="my-plugin"] .my-button {
  color: var(--ide-fg);
}
```

---

## 13. Ограничения

- **Single-file ES-модуль.** Blob-URL не резолвит относительные импорты. Всё нужное — в одном `index.js` (либо `import()` с абсолютным URL).
- **Sandboxed не поддерживается.** `trust: "sandboxed"` → ошибка активации.
- **`manifest` заморожен.** Не мутируйте; нужны дефолты — делайте копию.
- **Ошибки в командах и событиях не пробрасываются наружу** — ловятся и логируются.
- **При провале `activate`** — полный откат: стили, IO-выходы, capability, подписки, запись в реестре.
- **Доступ к файлам не ограничен каталогом проекта.** Плагин работает с тем, что даст ОС. Ставьте только те плагины, которым доверяете.

---

## 14. Полный пример

`plugins/counter/plugin.json`:

```json
{
  "id": "counter",
  "name": "Counter",
  "version": "1.0.0",
  "entry": "index.js",
  "inputs":  [{ "id": "increment" }],
  "outputs": [{ "id": "changed" }],
  "provides": [{ "id": "counter.api", "version": "1.0.0" }]
}
```

`plugins/counter/index.js`:

```js
export async function activate(ctx) {
  let value = (await ctx.storage.get('value', 0)) ?? 0;

  const publish = () => {
    void ctx.io.publish('changed', { value });
    ctx.events.emit('counter:changed', value);
  };

  // UI в сайдбар
  const slot = await ctx.ui.slot('sidebar');
  const btn = slot.create('button');
  btn.textContent = `Counter: ${value}`;
  btn.addEventListener('click', () => bump(1));
  slot.element.appendChild(btn);
  ctx.subscriptions.add(slot);

  // Команда
  ctx.subscriptions.add(
    ctx.commands.register('counter.get', () => value, 'Текущее значение')
  );

  // Входящий IO
  ctx.subscriptions.add(
    await ctx.io.onInput('increment', async (delta) => {
      bump(Number(delta) || 1);
      return { code: 200 };
    })
  );

  // Публичный API
  await ctx.capabilities.provide('counter.api', {
    get: () => value,
    onChanged: (fn) => ctx.events.on('counter:changed', fn),
  });

  async function bump(delta) {
    value += delta;
    btn.textContent = `Counter: ${value}`;
    await ctx.storage.set('value', value);
    publish();
  }
}
```

---

## 15. Чек-лист автора плагина

- [ ] Имя папки = `id` в манифесте.
- [ ] Все порты IO (`inputs`/`outputs`) объявлены в манифесте.
- [ ] Все предоставляемые capabilities (`provides`) объявлены.
- [ ] Все обязательные зависимости (`requires`) объявлены.
- [ ] `activate` не бросает необработанных исключений.
- [ ] Каждый `Disposable` отправлен в `ctx.subscriptions.add(...)`.
- [ ] Стили префиксованы `[data-plugin-id="<id>"]`.
- [ ] Entry — самодостаточный файл без относительных импортов.
- [ ] `trust: "trusted"` (sandboxed не поддерживается).
- [ ] Плагин корректно работает, если зависимость деактивирована (слушайте `handle.onRemoved` и `plugin:dependencyLost`).
- [ ] Ошибки сервисов обрабатываются по коду (`NOT_UTF8`, `NOT_FOUND`, …), а не «на всякий случай».

---

## Будущее

- Sandboxing (`trust: "sandboxed"`).
- Дополнительные регионы и сервисы.
- API стабилизируется; следите за `version` в манифесте и `requires` соседей.
