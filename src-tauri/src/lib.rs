//! AROMA IDE — бэкенд.
//!
//! Ядро намеренно не знает про конкретные фичи: редактор, терминал, проводник,
//! редактор схем и всё, что появится дальше, живут как плагины. Бэкенд даёт
//! им возможности ОС, которых нет у браузерного JavaScript:
//!
//! | Возможность | Модуль |
//! |-------------|--------|
//! | файлы, поиск, наблюдение | [`fs`] |
//! | терминал | [`pty`] |
//! | процессы (сборка, тулчейны) | [`proc`] |
//! | последовательные порты (платы, контроллеры) | [`serial`] |
//! | настройки | [`storage`] |
//! | плагины | [`plugins`] |
//!
//! Сквозные принципы:
//!
//! - **Плагин не должен вешать IDE.** Вывод потоков идёт через [`stream`], где
//!   данные схлопываются в пакеты, а запись в PTY не держит глобальный лок.
//! - **Ошибка объясняет себя.** [`error::AppError`] сериализуется в строку
//!   `КОД: сообщение`, где сообщение содержит путь.
//! - **Путь из плагина — недоверенный.** Всё, что из JS участвует в склейке
//!   путей, проходит через [`paths`].
//! - **Данные не теряются.** Запись файлов и настроек атомарна.
//! - **Процессы не остаются сиротами.** При выходе гасятся деревья процессов
//!   шеллов, сборок и открытые порты.

mod error;
mod fs;
mod logging;
mod paths;
mod plugins;
mod proc;
mod pty;
mod serial;
mod storage;
mod stream;
mod utf8;

use tauri::{AppHandle, Manager, RunEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // Логи инициализируем первыми: если что-то ниже упадёт, причина
            // должна попасть в файл, а не исчезнуть вместе с окном.
            let log_dir = handle
                .path()
                .app_log_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            logging::init(log_dir.clone(), handle.clone());
            log::info!(
                "AROMA IDE {} запускается; логи: {}",
                env!("CARGO_PKG_VERSION"),
                log_dir.display()
            );

            // Одна шина на все потоки: PTY, процессы, порты.
            let hub = stream::StreamHub::new(handle.clone());

            app.manage(hub.clone());
            app.manage(pty::PtyManager::new(hub.clone()));
            app.manage(proc::ProcManager::new(hub.clone()));
            app.manage(serial::SerialManager::new(hub));
            app.manage(fs::watch::WatcherManager::new(handle.clone()));
            app.manage(plugins::PluginState::default());
            app.manage(storage::StorageState::default());

            // Горячая перезагрузка плагинов: правка index.js не должна
            // требовать перезапуска IDE.
            plugins::start_hot_reload(handle);

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // --- файлы ---
            fs::fs_read_file,
            fs::fs_read_text,
            fs::fs_read_binary,
            fs::fs_write_file,
            fs::fs_write_binary,
            fs::fs_append_file,
            fs::fs_read_dir,
            fs::fs_stat,
            fs::fs_exists,
            fs::fs_mkdir,
            fs::fs_remove,
            fs::fs_rename,
            fs::fs_copy,
            fs::fs_canonicalize,
            fs::fs_home_dir,
            fs::fs_app_dirs,
            fs::fs_reveal,
            fs::fs_open,
            // --- поиск и наблюдение ---
            fs::search::fs_search,
            fs::watch::fs_watch,
            fs::watch::fs_unwatch,
            fs::watch::fs_watch_list,
            // --- плагины ---
            plugins::plugins_scan,
            plugins::plugins_rescan,
            plugins::plugins_diagnostics,
            plugins::plugins_roots,
            plugins::plugins_read,
            plugins::plugins_read_binary,
            plugins::plugins_list_files,
            // --- терминал ---
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            pty::pty_list,
            pty::pty_replay,
            // --- процессы ---
            proc::proc_spawn,
            proc::proc_write,
            proc::proc_kill,
            proc::proc_list,
            proc::proc_replay,
            proc::proc_which,
            // --- порты ---
            serial::serial_list_ports,
            serial::serial_open,
            serial::serial_write,
            serial::serial_set_signals,
            serial::serial_close,
            serial::serial_list,
            // --- настройки ---
            storage::storage_read,
            storage::storage_write,
            storage::storage_set,
            storage::storage_delete,
            storage::storage_info,
            storage::storage_list,
            // --- служебное ---
            app_info,
            app_open_log_dir,
        ])
        .build(tauri::generate_context!())
        .expect("не удалось собрать приложение AROMA IDE");

    app.run(|handle, event| {
        // Плагины не успевают вызвать kill при закрытии окна: дочерние
        // процессы (шеллы, сборки, порты) нужно погасить здесь, иначе после
        // выхода остаются сироты, держащие файлы и сетевые порты.
        if matches!(event, RunEvent::ExitRequested { .. } | RunEvent::Exit) {
            shutdown(handle);
        }
    });
}

fn shutdown(handle: &AppHandle) {
    log::info!("AROMA IDE завершается: останавливаем дочерние процессы");

    if let Some(pty) = handle.try_state::<pty::PtyManager>() {
        pty.kill_all();
    }
    if let Some(proc) = handle.try_state::<proc::ProcManager>() {
        proc.kill_all();
    }
    if let Some(serial) = handle.try_state::<serial::SerialManager>() {
        serial.close_all();
    }
    if let Some(watchers) = handle.try_state::<fs::watch::WatcherManager>() {
        watchers.unwatch_all();
    }
}

/// Версия и платформа: плагины показывают это в «О программе» и включают
/// платформенные ветки поведения.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub platform: &'static str,
    pub arch: &'static str,
    pub debug: bool,
    pub log_dir: Option<String>,
}

#[tauri::command]
fn app_info(handle: AppHandle) -> AppInfo {
    AppInfo {
        name: "AROMA IDE",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        debug: cfg!(debug_assertions),
        log_dir: handle
            .path()
            .app_log_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
    }
}

/// Открыть каталог логов — первое, что нужно при разборе проблемы.
#[tauri::command]
fn app_open_log_dir(handle: AppHandle) -> error::AppResult<()> {
    let dir = handle
        .path()
        .app_log_dir()
        .map_err(|e| error::AppError::internal(e.to_string()))?;
    std::fs::create_dir_all(&dir).map_err(|e| error::AppError::io(&dir, e))?;
    fs::fs_open(dir.to_string_lossy().to_string())
}
