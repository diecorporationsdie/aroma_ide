//! Плагины: поиск, проверка манифеста, чтение файлов, горячая перезагрузка.
//!
//! Порядок источников (позже перекрывает раньше):
//!
//! 1. `bundled` — ресурсы Tauri (`resources/plugins/`), вшитые инсталлятором.
//! 2. `exe_sibling` — папка `plugins` рядом с `.exe` (portable-сборка).
//! 3. `user` — `%APPDATA%/com.aroma.ide/plugins`.
//! 4. `dev` — `./plugins` вверх по дереву, только в dev-сборке.
//!
//! Что здесь важно:
//!
//! - **Один источник на плагин.** Раньше `plugins_scan` брал манифест из
//!   последнего корня, а `plugins_read` искал файл по всем корням в обратном
//!   порядке — при наличии плагина в `dev` и `user` `index.js` и `style.css`
//!   могли прийти из разных папок. Теперь победивший корень определяется по
//!   наличию `plugin.json` и используется для всех файлов плагина.
//! - **Манифест проверяется до загрузки.** Битый JSON, чужой `id`, пустое
//!   `name`, отсутствующий `entry` — плагин не попадает в список, а причина
//!   видна в `plugins_diagnostics`. Раньше такие плагины падали в JS с
//!   невнятной ошибкой.
//! - **Побег из каталога закрыт.** `Path::join` с абсолютным путём заменяет
//!   базу, поэтому `file` и `entry` разбираются по компонентам и проверяются
//!   на вложенность после канонизации.
//! - **Горячая перезагрузка.** Правка `index.js` не требует перезапуска:
//!   событие `plugins:changed` приходит после 300 мс тишины.

use crate::error::{AppError, AppResult, ErrorCode};
use crate::fs;
use crate::paths;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use notify::Watcher;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

/// Манифест больше этого — точно не манифест.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
/// Файл плагина больше этого не читаем.
const MAX_PLUGIN_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntry {
    pub id: String,
    /// Сырой JSON: фронтенд сам решает, какие поля ему нужны, а ядро не
    /// навязывает схему, которую придётся расширять под каждую фичу.
    pub manifest_json: String,
    /// Корень, из которого взят плагин.
    pub root: String,
    /// Вид источника: `bundled` | `exe_sibling` | `user` | `dev`.
    pub source: String,
    pub valid: bool,
    pub error: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub entry: Option<String>,
    pub style: Option<String>,
    pub mtime: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginRoot {
    pub kind: String,
    pub path: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginDiagnostic {
    pub id: String,
    pub path: String,
    pub error: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginBinary {
    pub base64: String,
    pub size: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PluginFile {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Структура манифеста, которую проверяет ядро. Остальные поля (`provides`,
/// `requires`, `inputs`, `outputs`) остаются во фронтенде: это контракт между
/// плагинами, а не с ОС.
#[derive(Deserialize, Debug)]
struct RawManifest {
    id: String,
    name: Option<String>,
    version: Option<String>,
    entry: Option<String>,
    style: Option<String>,
    trust: Option<String>,
}

#[derive(Debug)]
struct ParsedManifest {
    name: String,
    version: String,
    entry: String,
    style: Option<String>,
}

#[derive(Default)]
pub struct PluginState {
    roots: Mutex<Option<Vec<PluginRoot>>>,
}

impl PluginState {
    pub fn invalidate(&self) {
        *self.roots.lock() = None;
    }
}

fn candidate_roots<R: Runtime>(app: &AppHandle<R>) -> Vec<PluginRoot> {
    let mut roots: Vec<PluginRoot> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut push = |kind: &str, path: PathBuf| {
        let canon = path.canonicalize().unwrap_or(path);
        if seen.insert(canon.clone()) {
            roots.push(PluginRoot {
                kind: kind.to_string(),
                path: canon.to_string_lossy().to_string(),
            });
        }
    };

    // 1. Ресурсы Tauri (проверяем оба варианта вложенности).
    if let Ok(dir) = app.path().resource_dir() {
        for sub in ["plugins", "resources/plugins"] {
            let p = dir.join(sub);
            if p.is_dir() {
                push("bundled", p);
            }
        }
    }

    // 2. Папка `plugins` рядом с .exe — portable-сборка.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let p = exe_dir.join("plugins");
            if p.is_dir() {
                push("exe_sibling", p);
            }
        }
    }

    // 3. Пользовательские плагины. Каталог создаётся здесь один раз, а не в
    //    каждой команде чтения (раньше mkdir был на горячем пути).
    if let Ok(dir) = app.path().app_config_dir() {
        let p = dir.join("plugins");
        let _ = std::fs::create_dir_all(&p);
        push("user", p);
    }

    // 4. Dev-режим: правка плагина из репозитория.
    if cfg!(debug_assertions) {
        for p in dev_plugin_dirs() {
            push("dev", p);
        }
    }

    roots
}

fn dev_plugin_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut start_dirs = vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))];
    if let Ok(cwd) = std::env::current_dir() {
        start_dirs.push(cwd);
    }

    for mut dir in start_dirs {
        for _ in 0..4 {
            for candidate in ["plugins", "resources/plugins"] {
                let p = dir.join(candidate);
                if p.is_dir() {
                    if let Ok(canon) = p.canonicalize() {
                        if seen.insert(canon.clone()) {
                            out.push(canon);
                        }
                    }
                }
            }
            if !dir.pop() {
                break;
            }
        }
    }
    out
}

fn roots<R: Runtime>(app: &AppHandle<R>, state: &PluginState) -> Vec<PluginRoot> {
    let mut guard = state.roots.lock();
    if let Some(cached) = guard.as_ref() {
        return cached.clone();
    }
    let computed = candidate_roots(app);
    *guard = Some(computed.clone());
    computed
}

/// Проверка манифеста. `Err` — человекочитаемая причина отказа.
fn parse_manifest(folder_id: &str, json: &str) -> Result<ParsedManifest, String> {
    let raw: RawManifest = serde_json::from_str(json)
        .map_err(|e| format!("plugin.json: некорректный JSON: {e}"))?;

    if let Err(e) = paths::validate_plugin_id(&raw.id) {
        return Err(format!("plugin.json: id: {e}"));
    }
    if raw.id != folder_id {
        return Err(format!(
            "id в манифесте ({}) не совпадает с именем папки ({folder_id})",
            raw.id
        ));
    }

    let name = raw
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "plugin.json: не указано name".to_string())?
        .to_string();

    let version = raw
        .version
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("0.0.0")
        .to_string();

    let entry = raw
        .entry
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("index.js")
        .to_string();
    paths::validate_relative(&entry).map_err(|e| format!("plugin.json: entry: {e}"))?;

    if let Some(style) = raw.style.as_deref() {
        paths::validate_relative(style).map_err(|e| format!("plugin.json: style: {e}"))?;
    }

    if let Some(trust) = raw.trust.as_deref() {
        if trust != "trusted" && trust != "sandboxed" {
            return Err(format!(
                "plugin.json: trust должен быть \"trusted\" или \"sandboxed\", получено \"{trust}\""
            ));
        }
    }

    Ok(ParsedManifest {
        name,
        version,
        entry,
        style: raw.style,
    })
}

fn manifest_mtime(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

/// Собирает плагины. Никогда не падает целиком: один битый плагин не должен
/// лишать пользователя остальных.
fn scan<R: Runtime>(
    app: &AppHandle<R>,
    state: &PluginState,
) -> (Vec<PluginEntry>, Vec<PluginDiagnostic>) {
    let mut entries: Vec<PluginEntry> = Vec::new();
    let mut diagnostics: Vec<PluginDiagnostic> = Vec::new();

    for root in roots(app, state) {
        let root_path = PathBuf::from(&root.path);
        let readdir = match std::fs::read_dir(&root_path) {
            Ok(rd) => rd,
            Err(e) => {
                log::warn!("{}: {e}", root.path);
                continue;
            }
        };

        for entry in readdir.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Some(id) = dir.file_name().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };

            let manifest_path = dir.join("plugin.json");
            let manifest_json = match fs::read_text(&manifest_path, MAX_MANIFEST_BYTES) {
                Ok(f) => f.content,
                Err(_) => {
                    // Папка без манифеста — это не плагин (например `README`),
                    // поэтому в диагностику не попадает.
                    continue;
                }
            };

            let parsed = parse_manifest(&id, &manifest_json);

            let entry_file = parsed.as_ref().ok().map(|m| dir.join(&m.entry));
            let entry_exists = entry_file.as_ref().map(|p| p.is_file()).unwrap_or(false);
            let style_file = parsed
                .as_ref()
                .ok()
                .and_then(|m| m.style.as_ref().map(|s| dir.join(s)))
                .filter(|p| p.is_file());

            let (valid, error) = match (&parsed, entry_exists) {
                (Ok(_), true) => (true, None),
                (Ok(m), false) => (
                    false,
                    Some(format!("файл входа не найден: {}", m.entry)),
                ),
                (Err(reason), _) => (false, Some(reason.clone())),
            };

            if let Some(reason) = &error {
                diagnostics.push(PluginDiagnostic {
                    id: id.clone(),
                    path: dir.to_string_lossy().to_string(),
                    error: reason.clone(),
                });
                log::warn!("плагин '{id}' пропущен: {reason}");
            }

            // Позже найденный корень перекрывает более ранний — так работает
            // приоритет `dev` над `user` и `bundled`.
            entries.retain(|x| x.id != id);
            entries.push(PluginEntry {
                id,
                manifest_json,
                root: root.path.clone(),
                source: root.kind.clone(),
                valid,
                error,
                name: parsed.as_ref().ok().map(|m| m.name.clone()),
                version: parsed.as_ref().ok().map(|m| m.version.clone()),
                entry: entry_exists.then(|| {
                    parsed
                        .as_ref()
                        .map(|m| m.entry.clone())
                        .unwrap_or_else(|_| "index.js".to_string())
                }),
                style: style_file.map(|p| {
                    p.strip_prefix(&dir)
                        .map(|r| r.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default()
                }),
                mtime: manifest_mtime(&manifest_path),
            });
        }
    }

    entries.sort_by(|a, b| a.id.cmp(&b.id));
    (entries, diagnostics)
}

/// Каталог плагина и корень, из которого он взят.
fn plugin_dir<R: Runtime>(
    app: &AppHandle<R>,
    state: &PluginState,
    plugin_id: &str,
    root: Option<&str>,
) -> AppResult<(PathBuf, PluginRoot)> {
    paths::validate_plugin_id(plugin_id)?;
    let all = roots(app, state);

    if let Some(requested) = root {
        // Сравнение путей: точное совпадение строки ненадёжно (регистр,
        // разделители, `\\?\`-префикс). Сначала пробуем как есть, затем
        // сравниваем канонизированные пути.
        let requested_path = PathBuf::from(requested);
        let requested_canon = requested_path.canonicalize().ok();

        let found = all
            .iter()
            .find(|r| {
                let candidate = Path::new(&r.path);
                candidate == requested_path
                    || requested_canon
                        .as_ref()
                        .map(|q| candidate.canonicalize().map(|c| &c == q).unwrap_or(false))
                        .unwrap_or(false)
            })
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::InvalidPath,
                    format!("{requested}: неизвестный корень плагинов"),
                )
            })?;

        let plugin = Path::new(&found.path).join(plugin_id);
        if !plugin.join("plugin.json").is_file() {
            return Err(AppError::new(
                ErrorCode::NotFound,
                format!("плагин '{plugin_id}' не найден в {}", found.kind),
            ));
        }
        return Ok((plugin, found.clone()));
    }

    // Победитель — последний корень, где есть plugin.json: приоритет выше.
    let winner = all
        .iter()
        .rev()
        .find(|r| Path::new(&r.path).join(plugin_id).join("plugin.json").is_file())
        .cloned()
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::NotFound,
                format!("плагин '{plugin_id}' не найден"),
            )
        })?;

    Ok((Path::new(&winner.path).join(plugin_id), winner))
}

// ---------------------------------------------------------------------------
// Команды
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn plugins_scan<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
) -> Vec<PluginEntry> {
    let (entries, diagnostics) = scan(&app, &state);
    log::debug!(
        "plugins_scan: {} плагинов, {} проблем",
        entries.len(),
        diagnostics.len()
    );
    entries
}

/// Сброс кэша корней: вызывается после установки/удаления плагина.
#[tauri::command]
pub fn plugins_rescan<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
) -> Vec<PluginEntry> {
    state.invalidate();
    plugins_scan(app, state)
}

/// Причины, по которым плагины не попали в список. Без этого «плагин не
/// появился» невозможно отладить.
#[tauri::command]
pub fn plugins_diagnostics<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
) -> Vec<PluginDiagnostic> {
    scan(&app, &state).1
}

#[tauri::command]
pub fn plugins_roots<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
) -> Vec<PluginRoot> {
    roots(&app, &state)
}

/// Читает текстовый файл плагина. `root` необязателен: без него берётся
/// корень-победитель — тот же, что дал манифест.
#[tauri::command]
pub fn plugins_read<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
    plugin_id: String,
    file: String,
    root: Option<String>,
) -> AppResult<String> {
    let (dir, _) = plugin_dir(&app, &state, &plugin_id, root.as_deref())?;
    let path = paths::resolve_within(&dir, &file)?;
    Ok(fs::read_text(&path, MAX_PLUGIN_FILE_BYTES)?.content)
}

/// Бинарный ресурс плагина: иконка, картинка, шрифт, шаблон платы.
#[tauri::command]
pub fn plugins_read_binary<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
    plugin_id: String,
    file: String,
    root: Option<String>,
) -> AppResult<PluginBinary> {
    let (dir, _) = plugin_dir(&app, &state, &plugin_id, root.as_deref())?;
    let path = paths::resolve_within(&dir, &file)?;
    let bytes = fs::read_bytes(&path, MAX_PLUGIN_FILE_BYTES)?;
    Ok(PluginBinary {
        size: bytes.len() as u64,
        base64: BASE64.encode(&bytes),
    })
}

/// Файлы плагина: плагину нужно знать, какие ассеты он привёз с собой.
#[tauri::command]
pub fn plugins_list_files<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginState>,
    plugin_id: String,
    root: Option<String>,
) -> AppResult<Vec<PluginFile>> {
    let (dir, _) = plugin_dir(&app, &state, &plugin_id, root.as_deref())?;
    let mut out = Vec::new();
    let mut stack = vec![dir.clone()];

    while let Some(current) = stack.pop() {
        let Ok(readdir) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in readdir.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::metadata_symlink(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(&dir)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if rel.is_empty() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            }
            out.push(PluginFile {
                path: rel,
                size: meta.len(),
                is_dir: meta.is_dir(),
            });
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Горячая перезагрузка
// ---------------------------------------------------------------------------

/// Следит за всеми корнями плагинов и шлёт `plugins:changed` со списком id,
/// которые изменились. Хост плагинов по этому событию перезагружает модули.
pub fn start_hot_reload<R: Runtime>(app: AppHandle<R>) {
    let all = {
        let state = app.state::<PluginState>();
        roots(&app, &state)
    };
    if all.is_empty() {
        return;
    }

    std::thread::Builder::new()
        .name("plugin-hot-reload".into())
        .spawn(move || {
            let (tx, rx) = channel::<notify::Result<notify::Event>>();
            let mut watcher = match notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            }) {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("горячая перезагрузка плагинов недоступна: {e}");
                    return;
                }
            };

            for root in &all {
                if let Err(e) = watcher.watch(Path::new(&root.path), notify::RecursiveMode::Recursive)
                {
                    log::warn!("не удалось следить за {}: {e}", root.path);
                }
            }

            let mut pending: HashSet<String> = HashSet::new();
            let mut first_event: Option<Instant> = None;

            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(Ok(event)) => {
                        for path in event.paths {
                            if let Some(id) = plugin_id_of(&all, &path) {
                                pending.insert(id);
                            }
                        }
                        if first_event.is_none() {
                            first_event = Some(Instant::now());
                        }
                        // Debounce: редактор сохраняет файл несколькими записями.
                        let waited = first_event.map(|t| t.elapsed()).unwrap_or_default();
                        if !pending.is_empty() && waited.as_millis() >= 300 {
                            flush_changes(&app, &mut pending);
                            first_event = None;
                        }
                    }
                    Ok(Err(_)) => {}
                    Err(RecvTimeoutError::Timeout) => {
                        if !pending.is_empty() {
                            flush_changes(&app, &mut pending);
                            first_event = None;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .ok();
}

fn flush_changes<R: Runtime>(app: &AppHandle<R>, pending: &mut HashSet<String>) {
    let ids: Vec<String> = pending.drain().collect();
    if ids.is_empty() {
        return;
    }
    log::info!("плагины изменились: {ids:?}");
    let _ = app.emit("plugins:changed", serde_json::json!({ "ids": ids }));
}

fn plugin_id_of(all: &[PluginRoot], path: &Path) -> Option<String> {
    for root in all {
        let Ok(rel) = path.strip_prefix(&root.path) else {
            continue;
        };
        let first = rel.components().next()?;
        let id = first.as_os_str().to_string_lossy().to_string();
        // Служебный файл в корне (`README.md`) — не плагин.
        if id.is_empty() || !Path::new(&root.path).join(&id).is_dir() {
            continue;
        }
        return Some(id);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(json: &str) -> Result<ParsedManifest, String> {
        parse_manifest("demo", json)
    }

    #[test]
    fn accepts_minimal_manifest() {
        let m = check(r#"{"id":"demo","name":"Demo","version":"1.0.0"}"#).unwrap();
        assert_eq!(m.entry, "index.js");
        assert_eq!(m.version, "1.0.0");
        assert!(m.style.is_none());
    }

    #[test]
    fn defaults_entry_and_version() {
        let m = check(r#"{"id":"demo","name":"Demo"}"#).unwrap();
        assert_eq!(m.entry, "index.js");
        assert_eq!(m.version, "0.0.0");
    }

    #[test]
    fn rejects_id_mismatch() {
        let err = check(r#"{"id":"other","name":"Demo"}"#).unwrap_err();
        assert!(err.contains("не совпадает"), "{err}");
    }

    #[test]
    fn rejects_missing_name() {
        assert!(check(r#"{"id":"demo"}"#).is_err());
        assert!(check(r#"{"id":"demo","name":"   "}"#).is_err());
    }

    #[test]
    fn rejects_escaping_entry() {
        assert!(check(r#"{"id":"demo","name":"D","entry":"../../evil.js"}"#).is_err());
        assert!(check(r#"{"id":"demo","name":"D","entry":"C:\\evil.js"}"#).is_err());
    }

    #[test]
    fn rejects_bad_trust() {
        assert!(check(r#"{"id":"demo","name":"D","trust":"whatever"}"#).is_err());
    }

    #[test]
    fn rejects_broken_json() {
        assert!(check("{").is_err());
        assert!(check("[]").is_err());
    }
}
