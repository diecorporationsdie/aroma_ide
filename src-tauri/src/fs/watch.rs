//! Наблюдение за файлами.
//!
//! Зачем это ядру, а не плагину: `fs.watch` в браузере нет, а опрос mtime'ов
//! из JS — это либо задержка, либо постоянная нагрузка. Плагин подписывается
//! один раз и получает пакет изменений.
//!
//! Пакетирование обязательно: сохранение файла в редакторе порождает 2–4
//! события ОС (запись, закрытие, смена размера), а `npm install` — тысячи.
//! Отдавать их по одному через IPC нельзя, поэтому события копятся и уходят
//! одним пакетом после паузы.

use crate::error::{AppError, AppResult, ErrorCode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};

/// Пауза тишины, после которой накопленные события уходят наверх.
const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(120);
/// Больше этого ждать нельзя: при непрерывной записи (сборка) события должны
/// приходить, даже если паузы так и не наступило.
const MAX_COALESCE: Duration = Duration::from_millis(800);

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct WatchOptions {
    pub recursive: Option<bool>,
    pub debounce_ms: Option<u64>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FsChange {
    pub kind: String,
    pub path: String,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FsChangedPayload {
    pub id: String,
    pub changes: Vec<FsChange>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WatchInfo {
    pub id: String,
    pub path: String,
    pub recursive: bool,
}

struct ActiveWatch {
    /// Держим watcher живым: при `drop` наблюдение прекращается.
    _watcher: RecommendedWatcher,
    stop: Arc<AtomicBool>,
    info: WatchInfo,
}

#[derive(Default)]
pub struct WatcherManager {
    watches: Mutex<HashMap<String, ActiveWatch>>,
    app: Option<AppHandle>,
}

impl WatcherManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            watches: Mutex::new(HashMap::new()),
            app: Some(app),
        }
    }

    pub fn watch(&self, id: String, path: String, options: WatchOptions) -> AppResult<WatchInfo> {
        if id.trim().is_empty() {
            return Err(AppError::invalid_argument(
                "id наблюдения не может быть пустым",
            ));
        }

        let target = PathBuf::from(&path);
        let meta = std::fs::metadata(&target).map_err(|e| AppError::io(&target, e))?;
        if !meta.is_dir() {
            return Err(AppError::new(
                ErrorCode::NotADirectory,
                format!("{path}: наблюдать можно только за каталогом"),
            ));
        }

        // Повторная подписка с тем же id заменяет старую: плагин может
        // перезагрузиться и не помнить, что уже подписан.
        self.unwatch(&id);

        let recursive = options.recursive.unwrap_or(true);
        let debounce = options
            .debounce_ms
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_DEBOUNCE)
            .clamp(Duration::from_millis(20), Duration::from_millis(5_000));

        let (tx, rx) = channel::<notify::Result<Event>>();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|e| AppError::internal(format!("наблюдение за файлами: {e}")))?;

        watcher
            .watch(
                &target,
                if recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )
            .map_err(|e| AppError::internal(format!("{path}: {e}")))?;

        let stop = Arc::new(AtomicBool::new(false));
        let app = self.app.clone();
        let thread_id = id.clone();
        let stop_thread = Arc::clone(&stop);

        std::thread::Builder::new()
            .name(format!("fs-watch-{id}"))
            .spawn(move || {
                let mut pending: HashMap<PathBuf, String> = HashMap::new();
                let mut first_event: Option<Instant> = None;

                loop {
                    if stop_thread.load(Ordering::Relaxed) {
                        break;
                    }

                    match rx.recv_timeout(debounce) {
                        Ok(Ok(event)) => {
                            let kind = kind_name(&event.kind);
                            if first_event.is_none() {
                                first_event = Some(Instant::now());
                            }
                            for path in event.paths {
                                // Более сильное событие не должно затираться
                                // более слабым из того же пакета.
                                pending
                                    .entry(path)
                                    .and_modify(|existing| {
                                        if kind_rank(&kind) > kind_rank(existing) {
                                            *existing = kind.clone();
                                        }
                                    })
                                    .or_insert_with(|| kind.clone());
                            }

                            if first_event
                                .map(|t| t.elapsed() >= MAX_COALESCE)
                                .unwrap_or(false)
                            {
                                emit(&app, &thread_id, &mut pending);
                                first_event = None;
                            }
                        }
                        Ok(Err(e)) => {
                            log::warn!("fs_watch '{thread_id}': {e}");
                            if let Some(app) = &app {
                                let _ = app.emit(
                                    "fs:error",
                                    serde_json::json!({ "id": thread_id, "message": e.to_string() }),
                                );
                            }
                        }
                        Err(RecvTimeoutError::Timeout) => {
                            // Пауза тишины: отдаём накопленное.
                            if !pending.is_empty() {
                                emit(&app, &thread_id, &mut pending);
                                first_event = None;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(|e| AppError::internal(format!("поток наблюдения: {e}")))?;

        let info = WatchInfo {
            id: id.clone(),
            path,
            recursive,
        };

        self.watches.lock().insert(
            id,
            ActiveWatch {
                _watcher: watcher,
                stop,
                info: info.clone(),
            },
        );

        Ok(info)
    }

    pub fn unwatch(&self, id: &str) {
        if let Some(watch) = self.watches.lock().remove(id) {
            watch.stop.store(true, Ordering::Relaxed);
        }
    }

    pub fn list(&self) -> Vec<WatchInfo> {
        self.watches.lock().values().map(|w| w.info.clone()).collect()
    }

    pub fn unwatch_all(&self) {
        let mut guard = self.watches.lock();
        for (_, watch) in guard.drain() {
            watch.stop.store(true, Ordering::Relaxed);
        }
    }
}

fn emit(app: &Option<AppHandle>, id: &str, pending: &mut HashMap<PathBuf, String>) {
    if pending.is_empty() {
        return;
    }
    let changes: Vec<FsChange> = pending
        .drain()
        .map(|(path, kind)| FsChange {
            kind,
            path: path.to_string_lossy().to_string(),
        })
        .collect();

    if let Some(app) = app {
        let _ = app.emit(
            "fs:changed",
            FsChangedPayload {
                id: id.to_string(),
                changes,
            },
        );
    }
}

fn kind_name(kind: &EventKind) -> String {
    match kind {
        EventKind::Create(_) => "created",
        EventKind::Modify(_) => "modified",
        EventKind::Remove(_) => "removed",
        EventKind::Access(_) => "accessed",
        EventKind::Any => "any",
        EventKind::Other => "other",
    }
    .to_string()
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "removed" => 3,
        "created" => 2,
        "modified" => 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// Команды
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn fs_watch(
    manager: State<'_, WatcherManager>,
    id: String,
    path: String,
    options: Option<WatchOptions>,
) -> AppResult<WatchInfo> {
    manager.watch(id, path, options.unwrap_or_default())
}

#[tauri::command]
pub fn fs_unwatch(manager: State<'_, WatcherManager>, id: String) -> AppResult<()> {
    manager.unwatch(&id);
    Ok(())
}

#[tauri::command]
pub fn fs_watch_list(manager: State<'_, WatcherManager>) -> Vec<WatchInfo> {
    manager.list()
}
