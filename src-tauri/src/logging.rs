//! Логирование.
//!
//! В release-сборке окно консоли не создаётся, и `println!` уходит в никуда.
//! Без файла лога жалоба «плагин не работает» превращается в угадывание,
//! поэтому логгер пишет в `%LOCALAPPDATA%/<id>/logs/aroma.log` с ротацией по
//! 2 МБ и дублирует предупреждения/ошибки событием `app:log` — панель «Вывод»
//! может показывать их без чтения файла.

use log::{Level, LevelFilter, Log, Metadata, Record};
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const KEEP: usize = 2;

struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<File>>,
    app: OnceLock<AppHandle>,
}

impl FileLogger {
    fn open(&self) -> Option<File> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok()
    }

    fn rotate(&self) {
        let too_big = self
            .path
            .metadata()
            .map(|m| m.len() > MAX_BYTES)
            .unwrap_or(false);
        if !too_big {
            return;
        }
        for i in (1..KEEP).rev() {
            let from = self.path.with_extension(format!("{i}.log"));
            let to = self.path.with_extension(format!("{}.log", i + 1));
            if from.exists() {
                let _ = std::fs::rename(from, to);
            }
        }
        let _ = std::fs::rename(&self.path, self.path.with_extension("1.log"));
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // В release не пишем DEBUG, чтобы лог не рос на ровном месте.
        metadata.level() <= Level::Info || cfg!(debug_assertions)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = match record.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };
        let line = format!(
            "{} {level} [{}] {}",
            timestamp(),
            record.target(),
            record.args()
        );

        if record.level() <= Level::Warn {
            eprintln!("{line}");
        }

        let mut guard = self.file.lock();
        if guard.is_none() {
            self.rotate();
            *guard = self.open();
        }
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "{line}");
        }

        if record.level() <= Level::Warn {
            if let Some(app) = self.app.get() {
                let _ = app.emit(
                    "app:log",
                    serde_json::json!({
                        "level": level.trim(),
                        "target": record.target(),
                        "message": record.args().to_string(),
                    }),
                );
            }
        }
    }

    fn flush(&self) {
        if let Some(file) = self.file.lock().as_mut() {
            let _ = file.flush();
        }
    }
}

fn timestamp() -> String {
    // Без внешних зависимостей: время в формате `2026-11-27T09:00:00`.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}")
}

/// Перевод UNIX-времени в календарную дату (алгоритм Ховарда Хиннанта).
fn civil_from_unix(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let h = (rem / 3600) as u32;
    let mi = ((rem % 3600) / 60) as u32;
    let s = (rem % 60) as u32;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };

    (year, m, d, h, mi, s)
}

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

pub fn init(log_dir: PathBuf, app: AppHandle) {
    let logger = LOGGER.get_or_init(|| FileLogger {
        path: log_dir.join("aroma.log"),
        file: Mutex::new(None),
        app: OnceLock::new(),
    });
    let _ = logger.app.set(app);

    if log::set_logger(logger).is_ok() {
        log::set_max_level(if cfg!(debug_assertions) {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });
    }
}

/// Публичный путь лога — для команды «открыть папку логов».
#[allow(dead_code)]
pub fn log_path(log_dir: &Path) -> PathBuf {
    log_dir.join("aroma.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_unix(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil_from_unix(1_700_000_000).0, 2023);
    }
}
