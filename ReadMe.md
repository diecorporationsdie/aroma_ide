# AROMA IDE

Плагинная IDE на Tauri + TypeScript. Ядро не знает про конкретные фичи — редактор, терминал, explorer живут как плагины.

## Стек

- Frontend: TypeScript, Vite, нативный ES2022 (без сборки плагинов)
- Backend: Rust (Tauri 2) — только мост к ОС (`core.fs`, `pty`, сканирование плагинов)
- UI: CSS-переменные, scoped `.plugin-<id>`, JSON-слоты раскладки

## Требования

| Инструмент | Версия | Зачем |
|------------|--------|-------|
| Node.js | >= 20 | frontend toolchain |
| Rust (rustup) | stable | Tauri-ядро |
| VS Build Tools | 2022, workload "Desktop development with C++" | MSVC линкер |
| WebView2 | — | на Win10/11 уже есть |

Проверка:

    node --version
    rustc --version
    cargo --version

## Установка

    npm install

## Команды

| Команда | Назначение |
|---------|-----------|
| npm run dev | фронт в браузере без Tauri (FS/PTY недоступны) |
| npm run tauri dev | окно Tauri + hot reload фронта |
| npm run tauri build | релизный .exe + инсталлятор |
| npm run lint | ESLint (type-aware) |
| npm run lint:fix | ESLint + автофиксы |
| npm run build | tsc --noEmit + Vite production build |

Первый `tauri build` — 5–15 мин (компиляция крейтов), дальше инкрементально.

## Артефакты сборки

    src-tauri/target/release/AROMA IDE.exe
    src-tauri/target/release/bundle/msi/*.msi
    src-tauri/target/release/bundle/nsis/*-setup.exe

## Архитектура

    src/
      core/
        disposable.ts       Disposable / DisposableStore / DisposableBase
        errors.ts           PluginError, isolate()
        event-bus.ts        pub/sub, изолированный от падений подписчиков
        command-registry.ts register/execute, каждый register -> Disposable
        storage.ts          Storage + StorageNamespace (per-plugin)
        services.ts         ServiceRegistry (core.fs, pty и т.п.)
        plugin/
          api.ts            PluginContext — контракт для плагинов
          source.ts         PluginSource (InMemory, Filesystem)
          module-loader.ts  BlobModuleLoader (ESM через blob URL)
          ui-host.ts        UiHost — точка монтажа слотов
          context.ts        PluginContextImpl
          registry.ts       активные плагины + их dispose
          host.ts           PluginHost — activate/deactivate/list
      main.ts               bootstrap: core -> host -> activateAll

Правила ядра:

- все register* возвращают Disposable
- падение плагина не роняет ядро (isolate*)
- API только async, версионируется через PluginManifest
- запрещено: document/window/fs из плагина, синхронный API, хардкод раскладки
# AROMA Plugins

Плагин = папка `plugins/<id>/` с `plugin.json` + `index.js` (ESM, без сборки) + опциональный `style.css`.

## Обязательный контракт

- `activate(ctx)` — точка входа (опциональна).
- `deactivate()` — очистка (опциональна).
- `export const api` — **публичное API плагина** (опционально, но обязательно, если плагин хочет быть доступным другим).
- JSDoc-типы для `api` — **обязательны**, если экспортируется `api`.

## Публичный API

```js
/**
 * @typedef {Object} ExplorerApi
 * @property {(path: string) => Promise<void>} openFile
 * @property {() => string | null} getCurrentPath
 */

/** @type {ExplorerApi} */
export const api = {
  openFile: async (path) => { /* ... */ },
  getCurrentPath: () => currentPath,
};
```

### Структура

    plugins/<id>/
      plugin.json         манифест (id = имя папки)
      index.js            ESM-модуль, БЕЗ сборки
      style.css           опционально

### Манифест

    {
      "id": "hello",
      "name": "Hello",
      "version": "0.0.1",
      "entry": "index.js",
      "style": "style.css"
    }

### Модуль

    export function activate(ctx) {
      ctx.commands.register('hello.greet', () => 'hi');
      const sub = ctx.events.on('editor:save', (p) => console.log(p));
      ctx.subscriptions.add(sub);
    }

    export function deactivate() {
      // всё dispose-ится автоматически
    }

### Context API

| Поле | Методы |
|------|--------|
| ctx.commands | register(id, fn), execute(id, args), has(id) |
| ctx.events | on(event, fn), emit(event, payload) |
| ctx.ui | slot(region) -> sidebar / main / bottom / statusbar / overlay |
| ctx.services | get(name) — core.fs, pty |
| ctx.storage | get/set/delete/keys (namespace plugin.<id>) |
| ctx.subscriptions | add(disposable) |

### Куда класть

| Тип | Путь |
|-----|------|
| user | %APPDATA%\com.aroma.ide\plugins\<id>\ |
| bundled | src-tauri/resources/plugins/<id>/ (вшивается инсталлятором) |
| dev | ./plugins/<id>/ (только в tauri dev) |

Приоритет: bundled -> user -> dev (dev переопределяет).

Установка = скопировать папку. Удаление = удалить папку. Перезапуск подхватит.

## MVP-плагины

| id | Зависимости | Роль |
|----|-------------|------|
| explorer | core.fs | дерево файлов, открытие в редакторе |
| editor | CodeMirror 6 | вкладки, редактирование, сохранение |
| terminal | pty + xterm.js | встроенный шелл |

Ставятся как обычные плагины — в resources/plugins/ при сборке или в user-папку.

## Раскладка (JSON-слоты)

    {
      "sidebar": { "left": 260 },
      "main": { "editor": "*" },
      "bottom": { "terminal": 200 },
      "statusbar": { "height": 22 }
    }

Хранится в Storage. Ядро не хардкодит ни одной фичи — только раскладывает регионы.

## Лицензия

TBD
