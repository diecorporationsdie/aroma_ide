# Плагины: быстрый старт

Пять минут — и работает. Полный справочник: [plugins.md](plugins.md).

---

## Минимальный плагин

```
plugins/hello/
  plugin.json
  index.js
```

`plugin.json`:

```json
{
  "id": "hello",
  "name": "Hello",
  "version": "1.0.0"
}
```

`index.js`:

```js
export function activate(ctx) {
  ctx.subscriptions.add(
    ctx.commands.register('hello.greet', () => 'привет')
  );
}
```

Всё. Плагин найдётся сам — перезапуск не нужен.

---

## Куда класть

| Тип | Путь |
|-----|------|
| пользовательский | `%APPDATA%\com.aroma.ide\plugins\<id>\` |
| встроенный | `src-tauri/resources/plugins/<id>/` |
| dev | `./plugins/<id>/` (только в `tauri dev`) |

Позже найденный перекрывает ранний: `bundled` → `exe_sibling` → `user` → `dev`.

**Имя папки = `id` в манифесте.** Иначе плагин отклоняется.

---

## Что можно делать

```js
export async function activate(ctx) {
  // 1. Команда
  ctx.subscriptions.add(
    ctx.commands.register('my.save', async () => { /* ... */ }, 'Сохранить')
  );

  // 2. События
  ctx.subscriptions.add(
    ctx.events.on('editor:saved', ({ path }) => console.log(path))
  );

  // 3. UI
  const slot = await ctx.ui.slot('sidebar');
  const btn = slot.create('button');
  btn.textContent = 'Нажми';
  slot.element.appendChild(btn);
  ctx.subscriptions.add(slot);

  // 4. Хранилище
  await ctx.storage.set('lastFile', '/a.ts');
  const last = await ctx.storage.get('lastFile');

  // 5. Файлы
  const fs = await ctx.services.get('core.fs');
  const text = await fs.readFile('C:\\project\\main.ts');

  // 6. Терминал
  const pty = await ctx.services.get('pty');
  const proc = await pty.spawn({ cwd: '/project' });
  proc.onData((chunk) => term.write(chunk));
  ctx.subscriptions.add(proc);
}
```

---

## Главное правило

**Всё, что возвращает `Disposable`, кладите в `ctx.subscriptions`.**

```js
ctx.subscriptions.add(ctx.events.on('x', handler));
ctx.subscriptions.add(slot);
ctx.subscriptions.add(proc);
```

Иначе не освободится при деактивации — и плагин будет течь.

---

## Два ограничения

1. **`index.js` — один файл.** Относительные импорты не работают: модуль грузится через blob-URL.
2. **`trust: "sandboxed"` не поддерживается.** Плагин работает с правами IDE.

---

## Отладка

Если плагин не появился:

```js
await window.__TAURI__.core.invoke('plugins_diagnostics');
```

Возвращает список с причинами: битый манифест, несовпадение `id`, отсутствующий `entry`.

Лог: `%LOCALAPPDATA%\com.aroma.ide\logs\aroma.log`.

Правка `index.js` подхватывается на лету — перезапускать IDE не нужно.

---

## Дальше

- [plugins.md](plugins.md) — манифест, IO, capabilities, зависимости, все методы сервисов.
- [architecture.md](architecture.md) — как устроено ядро.
