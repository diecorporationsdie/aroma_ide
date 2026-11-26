# AROMA IDE — Руководство по написанию плагинов

Версия документа: 0.1.0  
Статус API: ранний, возможны ломающие изменения.

---

## 1. Что такое плагин

Плагин — это папка с манифестом `plugin.json` и входным JS-модулем (`index.js` по умолчанию). Плагины обнаруживаются хостом, активируются по порядку зависимостей и получают изолированный `PluginContext` для доступа к ядру IDE: командам, событиям, UI, хранилищу, IO и «возможностям» (capabilities).

Ядро построено на `Disposable`: всё, что плагин регистрирует (подписки, команды, слоты, входы/выходы, capability), можно и нужно освобождать. При деактивации плагина всё, что он положил в `ctx.subscriptions`, будет освобождено автоматически.

---

## 2. Где лежат плагины

Хост сканирует три корня и **перекрывает по `id`** в порядке: `bundled` → `user` → `dev` (позже выигрывает).

| Корень     | Путь                                    | Назначение                         |
|------------|-----------------------------------------|------------------------------------|
| `bundled`  | `<resource_dir>/plugins/<id>/`          | Поставляются вместе с приложением  |
| `user`     | `<app_config_dir>/plugins/<id>/`        | Устанавливает пользователь         |
| `dev`      | `<cwd>/plugins/<id>/`                   | Только в debug-сборке (разработка) |

> ⚠️ Имя папки **должно совпадать** с `id` в манифесте. Иначе плагин будет отклонён.

---

## 3. Структура плагина

```
plugins/
└── my-plugin/
    ├── plugin.json      # манифест (обязателен)
    ├── index.js         # entry-модуль (имя задаётся полем entry)
    └── style.css        # стили (имя задаётся полем style, опционально)
```

Хост читает только **эти файлы**. Относительные импорты из `index.js` не поддерживаются (см. §11).

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

| Поле          | Обязательно | Описание |
|---------------|-------------|----------|
| `id`          | ✅          | Уникальный. Совпадает с именем папки. Строка без `/`, `\`, `..`. |
| `name`        | ✅          | Отображаемое имя. |
| `version`     | ✅          | SemVer (`1.2.3`), используется при проверке зависимостей. |
| `description` |             | Произвольный текст. |
| `entry`       |             | Имя entry-файла. По умолчанию `index.js`. |
| `style`       |             | Имя CSS-файла. По умолчанию `style.css`. Файл может отсутствовать. |
| `trust`       |             | `trusted` (по умолчанию) или `sandboxed`. **Sandboxed пока не поддерживаются** — плагин будет отклонён при активации. |
| `inputs`      |             | Входные порты IO (см. §7). |
| `outputs`     |             | Выходные порты IO (см. §7). |
| `provides`    |             | Capabilities, которые плагин публикует (см. §8). |
| `requires`    |             | Capabilities, которые плагину нужны для работы. |

---

## 5. Entry-модуль

`index.js` — ES-модуль. Экспортирует один или оба символа:

```js
export async function activate(ctx) {
  // инициализация: регистрация команд, слотов, IO, capabilities
}

export function deactivate() {
  // опционально; всё из ctx.subscriptions уже будет освобождено
}
```

- `activate` может быть асинхронной, может бросить исключение — тогда плагин целиком откатится.
- `deactivate` вызывается **до** авто-dispose `ctx.subscriptions`. Если нужна специфичная очистка — здесь.

Модуль грузится как blob-URL: **один самодостаточный файл**, без относительных импортов (§11).

---

## 6. `PluginContext`

`ctx` в `activate(ctx)` — это замороженный снимок API. Все поля — read-only.

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
  const d = ctx.events.on('my-event', (p) => console.log(p));
  ctx.subscriptions.add(d);
}
```

### 6.1. `ctx.commands` — реестр команд

```js
ctx.commands.register('my.doSomething', async (args) => {
  // args — то, что пришло в execute(id, args)
  return { done: true };
}, 'Что делает команда');

const result = await ctx.commands.execute('other.command', { x: 1 });
if (ctx.commands.has('other.command')) { /* ... */ }
```

Ошибки в обработчике **глушатся** (логируются) и превращаются в `undefined` — регистрируйте важные сбои через `ctx.events.emit`.

### 6.2. `ctx.events` — шина событий

```js
ctx.subscriptions.add(ctx.events.on('editor:open', ({ path }) => {
  // ...
}));

ctx.events.emit('my-plugin:ready', { version: '1.0.0' });
```

### 6.3. `ctx.ui` — слоты в регионах

```js
const slot = await ctx.ui.slot('sidebar', { tier: 'normal', scroll: true });
const btn = slot.create('button');
btn.textContent = 'Click';
btn.addEventListener('click', () => ctx.events.emit('my:clicked'));
slot.element.appendChild(btn);
ctx.subscriptions.add(slot);
```

**Регионы:** `sidebar`, `main`, `bottom`, `statusbar`, `overlay`.

**Tier (z-index):** `background` < `normal` < `raised` < `overlay` < `modal` < `system`.  
По умолчанию: для `overlay`-региона — `overlay`, для остальных — `normal`.

**Опции слота:**

| Опция      | Значения                  | По умолчанию |
|------------|---------------------------|--------------|
| `position` | `relative` \| `fixed`     | `relative`   |
| `tier`     | см. выше                  | `normal` / `overlay` |
| `scroll`   | `boolean`                 | `true`       |

`fixed` центрирует слот поверх окна (модалки/диалоги).

Вложенные слоты — через `slot.addSlot(options)`, вернёт `SlotHandle` внутри текущего.

### 6.4. `ctx.services` — сервисы ядра

```js
const fs = await ctx.services.get('core.fs');   // FsService
const pty = await ctx.services.get('pty');       // PtyService
```

Доступные сейчас имена: `'core.fs'`, `'pty'`.

### 6.5. `ctx.storage` — персистентное хранилище

Namespace изолирован: `plugin.<id>`. Формат — `key → JSON-значение`.

```js
await ctx.storage.set('lastFile', '/tmp/a.ts');
const p = await ctx.storage.get('lastFile', null);
const keys = await ctx.storage.keys();
await ctx.storage.delete('lastFile');
```

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
  return off; // Disposable будет освобождён при удалении capability
});

// Подписка на исчезновение capability
ctx.subscriptions.add(handle.onRemoved(() => {
  console.warn('editor.api ушёл');
}));
```

### 6.8. `ctx.subscriptions`

`ctx.subscriptions.add(d: Disposable)` — регистрирует disposable под жизненный цикл плагина. `ctx.subscriptions.size` — количество.

### 6.9. Результаты операций (`PluginStatusResult`)

Методы `io` и `capabilities` возвращают:

```ts
{ code: number; message?: string; value?: unknown; deliveredTo?: number }
```

| Код  | Константа             | Смысл |
|------|-----------------------|-------|
| 200  | `OK`                  | Успех |
| 202  | `ACCEPTED`            | Принято, но никто не слушал (IO) |
| 204  | `NO_CONTENT`          | Пусто |
| 400  | `BAD_REQUEST`         | Некорректный запрос / не объявлено в манифесте |
| 404  | `NOT_FOUND`           | Порт/провайдер отсутствует |
| 409  | `CONFLICT`            | Дубликат (capability уже предоставлена) |
| 424  | `DEPENDENCY_MISSING`  | Не хватает зависимости |
| 500  | `INTERNAL_ERROR`      | Обработчик упал |
| 503  | `NOT_AVAILABLE`       | Временно недоступно |

---

## 7. IO — контракт между плагинами

- **`outputs`** — то, что плагин публикует (`publish`). Только объявленные в манифесте.
- **`inputs`** — то, что плагин принимает (`onInput`). Только объявленные в манифесте.
- **`subscribe` / `inject`** — кросс-плагин, проверяются на стороне цели.

`publish` возвращает `deliveredTo` — количество доставок. Если `0`, `code === 202`.

---

## 8. Зависимости и порядок активации

Хост строит граф по `requires` → `provides` (с проверкой semver через `^`, `~`, `>=`, `<=`, `>`, `<`, `*`, или точного совпадения).

- Плагины активируются в **топологическом порядке** (провайдеры раньше потребителей).
- **Циклы** — ошибка активации.
- **Неразрешённые зависимости** — плагин пропускается с warn, остальные активируются.
- **`optional: true`** — не блокирует активацию.

При деактивации провайдера хост:
1. Эмитит `plugin:dependencyLost` c `{ provider, dependents }`.
2. Каскадно деактивирует зависимые плагины, потерявшие обязательную capability.

---

## 9. Версионирование в `requires`

```json
"requires": [{ "id": "editor.api", "version": "^2.0.0" }]
```

Правила:
- без `version` — любая доступная версия;
- `^1.2.3` — major совпадает, не меньше;
- `~1.2.3` — major и minor совпадают, не меньше;
- `>=1.0.0`, `>1.0.0`, `<=2.0.0`, `<2.0.0` — сравнения;
- `*`, `x`, пустая строка — любая.

---

## 10. Стили

Если рядом с `index.js` есть файл `style.css` (или тот, что в `style`), он будет вставлен в `<head>` как `<style data-plugin-id="my-plugin">` при активации и удалён при деактивации. Рекомендуется префиксовать селекторы `[data-plugin-id="my-plugin"]`.

---

## 11. Ограничения и поведение

- **Single-file ES-модуль.** Blob-URL не резолвит относительные импорты. Всё, что нужно, — в одном `index.js` (или используйте `import()` с абсолютным URL).
- **Sandboxed-плагины не поддерживаются** — `trust: "sandboxed"` приведёт к ошибке активации.
- **`manifest` заморожен** — не мутируйте; если нужны дефолты, создавайте копию.
- **Ошибки в командах и событиях не пробрасываются наружу** — ловятся и логируются.
- **При провале `activate`** — полный откат: стили, IO-выходы, capability, `subscriptions`, регистрация в реестре.

---

## 12. Полный пример

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

## 13. Чек-лист автора плагина

- [ ] Имя папки = `id` в манифесте.
- [ ] Все порты IO (`inputs`/`outputs`) объявлены в манифесте.
- [ ] Все предоставляемые capabilities (`provides`) объявлены.
- [ ] Все обязательные зависимости (`requires`) объявлены.
- [ ] `activate` не бросает необработанных исключений (иначе плагин не активируется).
- [ ] Каждый `Disposable` отправлен в `ctx.subscriptions.add(...)`.
- [ ] Стили префиксованы `[data-plugin-id="<id>"]`.
- [ ] Entry — самодостаточный файл без относительных импортов.
- [ ] `trust: "trusted"` (sandboxed не поддерживается).
- [ ] Плагин корректно работает, если зависимость деактивирована (слушайте `handle.onRemoved` и событие `plugin:dependencyLost`).

---

## 14. Будущее (что стоит иметь в виду)

- Появится sandboxing (`trust: "sandboxed"`).
- Возможны дополнительные регионы/сервисы.
- API стабилизируется; следите за `version` в манифесте и `requires` соседей.