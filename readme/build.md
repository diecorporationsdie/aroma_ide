# Сборка и отладка

## Требования

| Инструмент | Версия | Зачем |
|------------|--------|-------|
| Node.js | ≥ 20 | frontend toolchain |
| Rust (rustup) | stable | Tauri-ядро |
| VS Build Tools | 2022, workload «Desktop development with C++» | MSVC линкер |
| WebView2 | — | на Win10/11 уже есть |

Проверка:

```bash
node --version
rustc --version
cargo --version
```

## Установка

```bash
npm install
```

## Команды

| Команда | Назначение |
|---------|-----------|
| `npm run tauri dev` | окно Tauri + hot reload фронта |
| `npm run dev` | фронт в браузере без Tauri (FS/PTY/процессы/порты недоступны) |
| `npm run tauri build` | релизный `.exe` + инсталлятор |
| `npm run build` | `tsc --noEmit` + Vite production build |
| `npm run lint` | ESLint (type-aware) |
| `npm run lint:fix` | ESLint + автофиксы |
| `npm run plugins:check` | проверка типов встроенных плагинов |

Первый `tauri build` — 5–15 минут (компиляция крейтов), дальше инкрементально.

## Артефакты

```
src-tauri/target/release/AROMA IDE.exe
src-tauri/target/release/bundle/msi/*.msi
src-tauri/target/release/bundle/nsis/*-setup.exe
```

## Тесты бэкенда

```bash
cd src-tauri
cargo test                  # 73 теста
cargo clippy --all-targets
```

Тесты включают интеграционные: реальный запуск PTY и процессов, коды возврата, запись в stdin, вытеснение истории сессий.

## Где что лежит

| Что | Путь |
|-----|------|
| логи | `%LOCALAPPDATA%\com.aroma.ide\logs\aroma.log` |
| настройки | `%APPDATA%\com.aroma.ide\storage\*.json` |
| плагины пользователя | `%APPDATA%\com.aroma.ide\plugins\<id>\` |
| встроенные плагины | `<папка .exe>\plugins\` или ресурсы приложения |

Логи ротируются (2 МБ, хранится 2 файла). Ошибки и предупреждения дублируются событием `app:log`.

## Отладка плагина

Плагин не появился в списке — причина не в тишине, её можно получить:

```js
await window.__TAURI__.core.invoke('plugins_diagnostics');
```

Возвращает `[{ id, path, error }]`. Частые причины:

- имя папки не совпадает с `id` в манифесте;
- битый `plugin.json`;
- файл из `entry` не существует;
- `trust: "sandboxed"` (не поддерживается).

Правка `index.js` подхватывается на лету — перезапуск не нужен. В `tauri dev` дополнительно ищется `./plugins/` рядом с `src-tauri/`.

## Типовые проблемы

**Терминал пустой.** Проверь лог: строка `pty '<id>': запущен`. Если процесс завершился мгновенно, вывод всё равно доступен через `pty_replay` — сессия хранит последние ~512 КБ.

**Порт не открывается.** COM-порт уже занят другим процессом (монитор порта Arduino, PuTTY). `serial_list_ports` покажет, что видит система.

**`npm run dev` в браузере.** FS, PTY, процессы и порты недоступны — это ожидаемо: вне Tauri нет моста к ОС.

**Сборка падает на линковке.** Нужен MSVC: VS Build Tools 2022 с workload «Desktop development with C++».

**WebView2 падает с ERROR_BUSY.**
при переносе проекта на другую машину нужно удалить сгенерированное (включая target, dist, node_modules), иначе WebView2 падает с ERROR_BUSY