//! Файловые операции для плагинов.
//!
//! Принципы, которые здесь соблюдаются:
//!
//! - **Атомарная запись.** `fs_write_file` пишет во временный файл рядом с
//!   целью, делает `fsync` и переименовывает поверх. Падение процесса или
//!   выключение питания не оставят обрезанный файл — читатель видит либо
//!   старую версию целиком, либо новую.
//! - **Текст и бинарь — разные каналы.** `fs_read_file` отдаёт только UTF-8
//!   (с явной ошибкой и подсказкой), бинарь идёт через base64. Иначе картинки,
//!   PDF и прошивки молча превращаются в мусор.
//! - **Каждая ошибка знает путь.** Сообщение без пути в IDE бесполезно.
//! - **Никаких «тихих» отказов.** `fs_stat` отличает «нет файла» от «нет
//!   прав», `fs_read_dir` не глотает ошибки чтения каталога.
//! - **Защита от катастрофы.** Рекурсивное удаление корня диска или домашнего
//!   каталога отклоняется: плагин может быть сгенерирован моделью, и цена
//!   одной ошибки слишком высока.
//!
//! ## Формат данных
//!
//! Поля отдаются в camelCase — ровно то, что объявлено в TS-интерфейсах
//! (`src/core/services/fs.ts`). Раньше бэкенд отдавал snake_case, поэтому
//! `stat.isFile` в TS всегда был `undefined`, а встроенные плагины спасались
//! проверкой обоих вариантов. Переход на camelCase их не ломает.
//!
//! `mtime` — **миллисекунды** UNIX epoch (раньше были секунды).

pub mod search;
pub mod watch;

use crate::error::{AppError, AppResult, ErrorCode};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Manager, Runtime};

/// Максимум, который `fs_read_file` готов отдать в строку: больше — это уже не
/// файл для редактора, а способ уронить webview.
pub const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;
/// Максимум для бинарного канала (base64 раздувает данные на треть).
pub const MAX_BINARY_BYTES: u64 = 256 * 1024 * 1024;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
    /// Скрытый: атрибут Windows или имя с точки.
    pub is_hidden: bool,
    pub size: u64,
    /// mtime в миллисекундах UNIX epoch.
    pub mtime: Option<u64>,

    // --- совместимость ---
    // Раньше бэкенд отдавал snake_case, хотя TS-интерфейс обещал camelCase.
    // Плагины могли быть написаны под любой из вариантов, поэтому отдаём оба:
    // это стоит нескольких байт и снимает целый класс «необъяснимых» багов.
    #[serde(rename = "is_dir")]
    pub is_dir_snake: bool,
    #[serde(rename = "is_symlink")]
    pub is_symlink_snake: bool,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Stat {
    pub is_file: bool,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// mtime в миллисекундах UNIX epoch.
    pub mtime: Option<u64>,
    /// Время создания (Windows/macOS), миллисекунды.
    pub created: Option<u64>,
    pub readonly: bool,

    // --- совместимость: см. комментарий в `DirEntry` ---
    #[serde(rename = "is_file")]
    pub is_file_snake: bool,
    #[serde(rename = "is_dir")]
    pub is_dir_snake: bool,
    #[serde(rename = "is_symlink")]
    pub is_symlink_snake: bool,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub content: String,
    pub size: u64,
    pub mtime: Option<u64>,
    /// `utf-8` | `utf-8-bom` | `utf-8-lossy`.
    pub encoding: &'static str,
    /// `\n` | `\r\n` — доминирующий перевод строки.
    pub eol: &'static str,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BinaryFile {
    pub base64: String,
    pub size: u64,
    pub mtime: Option<u64>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AppDirs {
    pub home: Option<String>,
    pub config: Option<String>,
    pub data: Option<String>,
    pub local_data: Option<String>,
    pub cache: Option<String>,
    pub log: Option<String>,
    pub resource: Option<String>,
    pub exe: Option<String>,
    pub cwd: Option<String>,
    pub desktop: Option<String>,
    pub documents: Option<String>,
    pub temp: String,
}

fn millis(t: std::io::Result<std::time::SystemTime>) -> Option<u64> {
    t.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

/// Метаданные без разыменования ссылки — для перечисления файлов.
pub fn metadata_symlink(path: &Path) -> std::io::Result<std::fs::Metadata> {
    fs::symlink_metadata(path)
}

fn detect_eol(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf {
        "\r\n"
    } else {
        "\n"
    }
}

/// Читает текстовый файл и сообщает, пришлось ли его чинить.
pub fn read_text(path: &Path, max_bytes: u64) -> AppResult<TextFile> {
    let meta = fs::metadata(path).map_err(|e| AppError::io(path, e))?;
    if meta.is_dir() {
        return Err(AppError::new(
            ErrorCode::IsADirectory,
            format!("{} — это каталог", path.display()),
        ));
    }
    if meta.len() > max_bytes {
        return Err(AppError::too_large(path, meta.len(), max_bytes));
    }

    let mut file = File::open(path).map_err(|e| AppError::io(path, e))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| AppError::io(path, e))?;

    // UTF-16 — частая причина «битого файла» в Windows-проектах. Говорим прямо,
    // что делать, вместо того чтобы отдать мусор.
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return Err(AppError::new(
            ErrorCode::NotUtf8,
            format!(
                "{}: файл в UTF-16 — пересохраните его в UTF-8",
                path.display()
            ),
        ));
    }

    let had_bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if had_bom { &bytes[3..] } else { &bytes[..] };

    let (content, encoding) = match std::str::from_utf8(body) {
        Ok(text) => (
            text.to_string(),
            if had_bom { "utf-8-bom" } else { "utf-8" },
        ),
        // Битые байты не должны блокировать открытие файла: редактор покажет
        // ромбики, но пользователь увидит, что в файле, и сможет починить.
        Err(_) => (String::from_utf8_lossy(body).into_owned(), "utf-8-lossy"),
    };

    Ok(TextFile {
        eol: detect_eol(&content),
        content,
        size: meta.len(),
        mtime: millis(meta.modified()),
        encoding,
    })
}

pub fn read_bytes(path: &Path, limit: u64) -> AppResult<Vec<u8>> {
    let meta = fs::metadata(path).map_err(|e| AppError::io(path, e))?;
    if meta.is_dir() {
        return Err(AppError::new(
            ErrorCode::IsADirectory,
            format!("{} — это каталог", path.display()),
        ));
    }
    if meta.len() > limit {
        return Err(AppError::too_large(path, meta.len(), limit));
    }
    fs::read(path).map_err(|e| AppError::io(path, e))
}

/// Атомарная запись: временный файл в той же директории → fsync → rename.
///
/// `rename` внутри одной директории атомарен и на NTFS, и на POSIX. Именно
/// поэтому временный файл создаётся рядом с целью, а не в системном temp:
/// перенос между томами атомарным не будет.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(d) => {
            if !d.exists() {
                fs::create_dir_all(d).map_err(|e| AppError::io(d, e))?;
            }
            d.to_path_buf()
        }
        None => PathBuf::from("."),
    };

    if dir.exists() && !dir.is_dir() {
        return Err(AppError::new(
            ErrorCode::NotADirectory,
            format!("{} — родитель не каталог", dir.display()),
        ));
    }

    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let temp = dir.join(format!(".{stem}.{}-{n}.aroma-tmp", std::process::id()));

    let result = (|| -> AppResult<()> {
        let mut file = File::create(&temp).map_err(|e| AppError::io(&temp, e))?;
        file.write_all(bytes).map_err(|e| AppError::io(&temp, e))?;
        file.flush().map_err(|e| AppError::io(&temp, e))?;
        // Данные должны быть на диске до rename: иначе после сбоя получим
        // существующий, но пустой файл.
        file.sync_all().map_err(|e| AppError::io(&temp, e))?;
        drop(file);

        match fs::rename(&temp, path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                // Windows: файл может быть занят антивирусом или другим
                // процессом ровно в момент rename. Одна повторная попытка
                // снимает большинство таких случаев.
                std::thread::sleep(std::time::Duration::from_millis(40));
                fs::rename(&temp, path).map_err(|e| AppError::io(path, e))
            }
            Err(e) => Err(AppError::io(path, e)),
        }
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Защита от катастрофического `fs_remove`.
///
/// Корень диска, домашний каталог и каталог настроек IDE удалять нельзя.
fn guard_destructive(path: &Path, app: Option<&AppHandle<impl Runtime>>) -> AppResult<()> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    if canon.parent().is_none() {
        return Err(AppError::new(
            ErrorCode::InvalidPath,
            format!(
                "{} — корень файловой системы, удаление запрещено",
                canon.display()
            ),
        ));
    }

    if let Some(home) = home_dir() {
        if home.canonicalize().map(|h| h == canon).unwrap_or(false) {
            return Err(AppError::new(
                ErrorCode::InvalidPath,
                format!(
                    "{} — домашний каталог, удаление запрещено",
                    canon.display()
                ),
            ));
        }
    }

    if let Some(app) = app {
        if let Ok(cfg) = app.path().app_config_dir() {
            if cfg.canonicalize().map(|c| c == canon).unwrap_or(false) {
                return Err(AppError::new(
                    ErrorCode::InvalidPath,
                    format!(
                        "{} — каталог настроек IDE, удаление запрещено",
                        canon.display()
                    ),
                ));
            }
        }
    }

    Ok(())
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

pub fn copy_path(from: &Path, to: &Path, overwrite: bool) -> AppResult<()> {
    let meta = fs::symlink_metadata(from).map_err(|e| AppError::io(from, e))?;
    if to.exists() && !overwrite {
        return Err(AppError::new(
            ErrorCode::AlreadyExists,
            format!("{} уже существует", to.display()),
        ));
    }
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
    }

    if meta.file_type().is_symlink() {
        copy_symlink(from, to)
    } else if meta.is_dir() {
        copy_dir(from, to)
    } else {
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| AppError::io(to, e))
    }
}

fn copy_dir(from: &Path, to: &Path) -> AppResult<()> {
    fs::create_dir_all(to).map_err(|e| AppError::io(to, e))?;
    let readdir = fs::read_dir(from).map_err(|e| AppError::io(from, e))?;
    for entry in readdir {
        let entry = entry.map_err(|e| AppError::io(from, e))?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let meta = match fs::symlink_metadata(&src) {
            Ok(m) => m,
            Err(_) => continue, // файл исчез между readdir и stat
        };
        if meta.file_type().is_symlink() {
            // Битая ссылка не должна ронять копирование всего дерева.
            let _ = copy_symlink(&src, &dst);
        } else if meta.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst).map_err(|e| AppError::io(&dst, e))?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn copy_symlink(from: &Path, to: &Path) -> AppResult<()> {
    let target = fs::read_link(from).map_err(|e| AppError::io(from, e))?;
    let is_dir = fs::metadata(from).map(|m| m.is_dir()).unwrap_or(false);
    let res = if is_dir {
        std::os::windows::fs::symlink_dir(&target, to)
    } else {
        std::os::windows::fs::symlink_file(&target, to)
    };
    res.map_err(|e| AppError::io(to, e))
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> AppResult<()> {
    let target = fs::read_link(from).map_err(|e| AppError::io(from, e))?;
    std::os::unix::fs::symlink(&target, to).map_err(|e| AppError::io(to, e))
}

#[cfg(not(any(windows, unix)))]
fn copy_symlink(_from: &Path, _to: &Path) -> AppResult<()> {
    Err(AppError::unsupported("symlink не поддерживается"))
}

/// Перемещение с фолбэком «копировать + удалить» между разными томами.
pub fn move_path(from: &Path, to: &Path, overwrite: bool) -> AppResult<()> {
    if !from.exists() {
        return Err(AppError::new(
            ErrorCode::NotFound,
            format!("{} не найден", from.display()),
        ));
    }
    if to.exists() && !overwrite {
        return Err(AppError::new(
            ErrorCode::AlreadyExists,
            format!("{} уже существует", to.display()),
        ));
    }
    if let Some(parent) = to.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
    }

    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) => {
            let cross_device = AppError::from(std::io::Error::new(e.kind(), e.to_string())).code
                == ErrorCode::CrossDevice;
            if cross_device {
                copy_path(from, to, overwrite)?;
                let meta = fs::symlink_metadata(from).map_err(|e| AppError::io(from, e))?;
                if meta.is_dir() {
                    fs::remove_dir_all(from).map_err(|e| AppError::io(from, e))
                } else {
                    fs::remove_file(from).map_err(|e| AppError::io(from, e))
                }
            } else {
                Err(AppError::io(to, e))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Команды
// ---------------------------------------------------------------------------

/// Читает текст. Возвращает строку — так плагины вызывали её и раньше.
#[tauri::command]
pub fn fs_read_file(path: String, max_bytes: Option<u64>) -> AppResult<String> {
    Ok(read_text(Path::new(&path), max_bytes.unwrap_or(MAX_TEXT_BYTES))?.content)
}

/// Читает текст вместе с метаданными: кодировка, EOL, mtime.
#[tauri::command]
pub fn fs_read_text(path: String, max_bytes: Option<u64>) -> AppResult<TextFile> {
    read_text(Path::new(&path), max_bytes.unwrap_or(MAX_TEXT_BYTES))
}

#[tauri::command]
pub fn fs_read_binary(path: String, max_bytes: Option<u64>) -> AppResult<BinaryFile> {
    let p = Path::new(&path);
    let bytes = read_bytes(p, max_bytes.unwrap_or(MAX_BINARY_BYTES))?;
    let meta = fs::metadata(p).map_err(|e| AppError::io(p, e))?;
    Ok(BinaryFile {
        base64: BASE64.encode(&bytes),
        size: meta.len(),
        mtime: millis(meta.modified()),
    })
}

/// Запись текста. `atomic` по умолчанию включён: это IDE, а не скретчпад.
#[tauri::command]
pub fn fs_write_file(path: String, contents: String, atomic: Option<bool>) -> AppResult<Stat> {
    let p = Path::new(&path);
    if atomic.unwrap_or(true) {
        write_atomic(p, contents.as_bytes())?;
    } else {
        fs::write(p, contents).map_err(|e| AppError::io(p, e))?;
    }
    stat_required(p)
}

#[tauri::command]
pub fn fs_write_binary(path: String, base64: String) -> AppResult<Stat> {
    let p = Path::new(&path);
    let bytes = BASE64
        .decode(base64.as_bytes())
        .map_err(|e| AppError::new(ErrorCode::InvalidArgument, format!("base64: {e}")))?;
    write_atomic(p, &bytes)?;
    stat_required(p)
}

#[tauri::command]
pub fn fs_append_file(path: String, contents: String) -> AppResult<()> {
    let p = Path::new(&path);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .map_err(|e| AppError::io(p, e))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| AppError::io(p, e))
}

#[tauri::command]
pub fn fs_read_dir(path: String) -> AppResult<Vec<DirEntry>> {
    let dir = Path::new(&path);
    let readdir = fs::read_dir(dir).map_err(|e| AppError::io(dir, e))?;
    let mut out = Vec::new();

    for entry in readdir {
        // Ошибку отдельной записи не глотаем: молча пропущенный файл в IDE —
        // худший вариант, чем ошибка.
        let entry = entry.map_err(|e| AppError::io(dir, e))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let full = entry.path();

        let own = fs::symlink_metadata(&full).ok();
        let is_symlink = own
            .as_ref()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        let (is_dir, is_file, size) = match own.as_ref() {
            Some(_) if is_symlink => match fs::metadata(&full) {
                // Тип берём у цели: проводник должен рисовать папку папкой.
                Ok(target) => (target.is_dir(), target.is_file(), target.len()),
                Err(_) => (false, false, 0), // битая ссылка
            },
            Some(m) => (m.is_dir(), m.is_file(), m.len()),
            None => (false, false, 0),
        };

        out.push(DirEntry {
            is_hidden: name.starts_with('.') || is_hidden_attr(&full),
            name,
            path: full.to_string_lossy().to_string(),
            is_dir,
            is_file,
            is_symlink,
            size,
            mtime: own.as_ref().and_then(|m| millis(m.modified())),
            is_dir_snake: is_dir,
            is_symlink_snake: is_symlink,
        });    }

    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    Ok(out)
}

#[cfg(windows)]
fn is_hidden_attr(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    fs::symlink_metadata(path)
        .map(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_hidden_attr(_path: &Path) -> bool {
    false
}

/// `None` — только если файла действительно нет. Остальные ошибки (нет прав,
/// слишком длинный путь) пробрасываются: иначе UI врёт «файл отсутствует».
#[tauri::command]
pub fn fs_stat(path: String) -> AppResult<Option<Stat>> {
    let p = Path::new(&path);
    let own = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(AppError::io(p, e)),
    };

    let is_symlink = own.file_type().is_symlink();
    let meta = if is_symlink {
        fs::metadata(p).unwrap_or_else(|_| own.clone())
    } else {
        own.clone()
    };

    Ok(Some(Stat {
        is_file: meta.is_file(),
        is_dir: meta.is_dir(),
        is_symlink,
        size: meta.len(),
        mtime: millis(meta.modified()),
        created: millis(meta.created()),
        readonly: meta.permissions().readonly(),
        is_file_snake: meta.is_file(),
        is_dir_snake: meta.is_dir(),
        is_symlink_snake: is_symlink,
    }))
}

fn stat_required(path: &Path) -> AppResult<Stat> {
    match fs_stat(path.to_string_lossy().to_string())? {
        Some(stat) => Ok(stat),
        None => Err(AppError::new(
            ErrorCode::NotFound,
            format!("{}: файл исчез сразу после записи", path.display()),
        )),
    }
}

#[tauri::command]
pub fn fs_exists(path: String) -> AppResult<bool> {
    let p = Path::new(&path);
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(AppError::io(p, e)),
    }
}

#[tauri::command]
pub fn fs_mkdir(path: String, recursive: Option<bool>) -> AppResult<()> {
    let p = Path::new(&path);
    if recursive.unwrap_or(false) {
        fs::create_dir_all(p).map_err(|e| AppError::io(p, e))
    } else {
        fs::create_dir(p).map_err(|e| AppError::io(p, e))
    }
}

/// Удаление. Решает по `symlink_metadata`: ссылку удаляем как ссылку, а не как
/// каталог, на который она указывает.
#[tauri::command]
pub fn fs_remove<R: Runtime>(
    app: AppHandle<R>,
    path: String,
    recursive: Option<bool>,
) -> AppResult<()> {
    let p = Path::new(&path);
    let meta = match fs::symlink_metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppError::io(p, e)),
    };

    guard_destructive(p, Some(&app))?;

    if meta.file_type().is_symlink() || meta.is_file() {
        fs::remove_file(p).map_err(|e| AppError::io(p, e))
    } else if meta.is_dir() {
        if recursive.unwrap_or(true) {
            fs::remove_dir_all(p).map_err(|e| AppError::io(p, e))
        } else {
            fs::remove_dir(p).map_err(|e| AppError::io(p, e))
        }
    } else {
        Err(AppError::unsupported(format!(
            "{}: неизвестный тип файла",
            p.display()
        )))
    }
}

#[tauri::command]
pub fn fs_rename(from: String, to: String, overwrite: Option<bool>) -> AppResult<()> {
    move_path(Path::new(&from), Path::new(&to), overwrite.unwrap_or(true))
}

#[tauri::command]
pub fn fs_copy(from: String, to: String, overwrite: Option<bool>) -> AppResult<()> {
    copy_path(Path::new(&from), Path::new(&to), overwrite.unwrap_or(true))
}

#[tauri::command]
pub fn fs_canonicalize(path: String) -> AppResult<String> {
    let p = Path::new(&path);
    p.canonicalize()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| AppError::io(p, e))
}

#[tauri::command]
pub fn fs_home_dir<R: Runtime>(app: AppHandle<R>) -> AppResult<String> {
    app.path()
        .home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| AppError::internal(e.to_string()))
}

/// Все пути, которые нужны плагину: настройки, кэш, логи, ресурсы.
#[tauri::command]
pub fn fs_app_dirs<R: Runtime>(app: AppHandle<R>) -> AppResult<AppDirs> {
    let path = app.path();
    let s = |r: Result<PathBuf, tauri::Error>| r.ok().map(|p| p.to_string_lossy().to_string());
    Ok(AppDirs {
        home: s(path.home_dir()),
        config: s(path.app_config_dir()),
        data: s(path.app_data_dir()),
        local_data: s(path.app_local_data_dir()),
        cache: s(path.app_cache_dir()),
        log: s(path.app_log_dir()),
        resource: s(path.resource_dir()),
        exe: std::env::current_exe()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        cwd: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        desktop: s(path.desktop_dir()),
        documents: s(path.document_dir()),
        temp: std::env::temp_dir().to_string_lossy().to_string(),
    })
}

fn require_exists(path: &Path) -> AppResult<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::NotFound,
            format!("{} не найден", path.display()),
        ))
    }
}

/// Открыть в системном файловом менеджере с выделением файла.
#[tauri::command]
pub fn fs_reveal(path: String) -> AppResult<()> {
    let p = Path::new(&path);
    require_exists(p)?;

    #[cfg(windows)]
    {
        // explorer.exe /select,<path> — путь должен быть одним аргументом.
        let arg = if p.is_dir() {
            p.to_string_lossy().to_string()
        } else {
            format!("/select,{}", p.display())
        };
        std::process::Command::new("explorer.exe")
            .arg(arg)
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::internal(format!("проводник: {e}")))
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-R"])
            .arg(p)
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::internal(format!("Finder: {e}")))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
        std::process::Command::new("xdg-open")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::internal(format!("файловый менеджер: {e}")))
    }
}

/// Открыть файл программой по умолчанию.
#[tauri::command]
pub fn fs_open(path: String) -> AppResult<()> {
    let p = Path::new(&path);
    require_exists(p)?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // `cmd /C start "" <path>`: пустой заголовок обязателен, иначе `start`
        // примет первый сегмент пути с пробелами за заголовок окна.
        std::process::Command::new("cmd")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/C", "start", ""])
            .arg(p)
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::internal(format!("start: {e}")))
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(p)
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::internal(format!("open: {e}")))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(p)
            .spawn()
            .map(|_| ())
            .map_err(|e| AppError::internal(format!("xdg-open: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Уникальный каталог на тест: тесты идут параллельно.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let unique = format!(
                "aroma-fs-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            );
            let dir = std::env::temp_dir().join(unique);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn writes_and_reads_utf8() {
        let tmp = TempDir::new("rw");
        let file = tmp.file("a.txt");

        write_atomic(&file, "привет\nмир".as_bytes()).unwrap();
        let read = read_text(&file, MAX_TEXT_BYTES).unwrap();

        assert_eq!(read.content, "привет\nмир");
        assert_eq!(read.encoding, "utf-8");
        assert_eq!(read.eol, "\n");
        assert!(read.size > 0);
    }

    #[test]
    fn strips_utf8_bom() {
        let tmp = TempDir::new("bom");
        let file = tmp.file("bom.txt");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"{\"a\":1}");
        fs::write(&file, bytes).unwrap();

        let read = read_text(&file, MAX_TEXT_BYTES).unwrap();
        // BOM не должен попасть в контент: иначе JSON.parse падает.
        assert_eq!(read.content, "{\"a\":1}");
        assert_eq!(read.encoding, "utf-8-bom");
    }

    #[test]
    fn detects_crlf() {
        let tmp = TempDir::new("crlf");
        let file = tmp.file("win.txt");
        write_atomic(&file, b"one\r\ntwo\r\nthree").unwrap();
        assert_eq!(read_text(&file, MAX_TEXT_BYTES).unwrap().eol, "\r\n");
    }

    #[test]
    fn rejects_utf16_with_clear_error() {
        let tmp = TempDir::new("utf16");
        let file = tmp.file("wide.txt");
        fs::write(&file, [0xFF, 0xFE, b'h', 0, b'i', 0]).unwrap();

        let err = read_text(&file, MAX_TEXT_BYTES).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotUtf8);
        assert!(err.message.contains("UTF-16"), "{}", err.message);
    }

    #[test]
    fn broken_utf8_degrades_instead_of_failing() {
        let tmp = TempDir::new("lossy");
        let file = tmp.file("broken.txt");
        fs::write(&file, [b'a', 0xFF, b'b']).unwrap();

        let read = read_text(&file, MAX_TEXT_BYTES).unwrap();
        assert_eq!(read.encoding, "utf-8-lossy");
        assert!(read.content.starts_with('a'));
        assert!(read.content.ends_with('b'));
    }

    #[test]
    fn enforces_size_limit() {
        let tmp = TempDir::new("limit");
        let file = tmp.file("big.txt");
        fs::write(&file, vec![b'x'; 128]).unwrap();

        let err = read_text(&file, 64).unwrap_err();
        assert_eq!(err.code, ErrorCode::TooLarge);
    }

    #[test]
    fn reading_a_directory_is_an_error() {
        let tmp = TempDir::new("isdir");
        let err = read_text(tmp.path(), MAX_TEXT_BYTES).unwrap_err();
        assert_eq!(err.code, ErrorCode::IsADirectory);
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let tmp = TempDir::new("replace");
        let file = tmp.file("config.json");
        fs::write(&file, b"{\"v\":1}").unwrap();

        write_atomic(&file, b"{\"v\":2}").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"v\":2}");
    }

    #[test]
    fn atomic_write_creates_missing_directories() {
        let tmp = TempDir::new("mkdirs");
        let file = tmp.file("deep/nested/file.txt");
        write_atomic(&file, b"x").unwrap();
        assert!(file.is_file());
    }

    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let tmp = TempDir::new("notemp");
        let file = tmp.file("a.txt");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();

        let leftovers: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("aroma-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "остались временные файлы: {leftovers:?}");
    }

    #[test]
    fn stat_reports_types_and_mtime() {
        let tmp = TempDir::new("stat");
        let file = tmp.file("a.txt");
        write_atomic(&file, b"abc").unwrap();

        let stat = fs_stat(file.to_string_lossy().to_string()).unwrap().unwrap();
        assert!(stat.is_file);
        assert!(!stat.is_dir);
        assert_eq!(stat.size, 3);
        // mtime в миллисекундах: значение заведомо больше 1.6e12.
        assert!(stat.mtime.unwrap() > 1_600_000_000_000);

        assert!(fs_stat(tmp.file("nope").to_string_lossy().to_string())
            .unwrap()
            .is_none());
    }

    #[test]
    fn read_dir_sorts_directories_first() {
        let tmp = TempDir::new("ls");
        fs::create_dir(tmp.file("zdir")).unwrap();
        fs::create_dir(tmp.file("adir")).unwrap();
        fs::write(tmp.file("b.txt"), b"b").unwrap();
        fs::write(tmp.file("a.txt"), b"a").unwrap();

        let entries = fs_read_dir(tmp.path().to_string_lossy().to_string()).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["adir", "zdir", "a.txt", "b.txt"]);
    }

    #[test]
    fn copy_and_move_work() {
        let tmp = TempDir::new("copymove");
        let src = tmp.file("src.txt");
        write_atomic(&src, b"payload").unwrap();

        let copied = tmp.file("copy.txt");
        copy_path(&src, &copied, false).unwrap();
        assert_eq!(fs::read_to_string(&copied).unwrap(), "payload");
        // Копирование не удаляет источник.
        assert!(src.is_file());

        // Без overwrite существующая цель — ошибка, а не тихая перезапись.
        let err = copy_path(&src, &copied, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::AlreadyExists);

        let moved = tmp.file("moved.txt");
        move_path(&src, &moved, true).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&moved).unwrap(), "payload");
    }

    #[test]
    fn copy_dir_is_recursive() {
        let tmp = TempDir::new("copytree");
        fs::create_dir_all(tmp.file("src/nested")).unwrap();
        fs::write(tmp.file("src/a.txt"), b"a").unwrap();
        fs::write(tmp.file("src/nested/b.txt"), b"b").unwrap();

        copy_path(&tmp.file("src"), &tmp.file("dst"), false).unwrap();
        assert_eq!(fs::read_to_string(tmp.file("dst/a.txt")).unwrap(), "a");
        assert_eq!(fs::read_to_string(tmp.file("dst/nested/b.txt")).unwrap(), "b");
    }

    #[cfg(unix)]
    #[test]
    fn removing_a_symlink_does_not_touch_the_target() {
        let tmp = TempDir::new("symlink");
        let target = tmp.file("real.txt");
        fs::write(&target, b"keep me").unwrap();
        let link = tmp.file("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        // fs_remove для ссылки должен удалить ссылку, а не цель.
        let meta = fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink());
        fs::remove_file(&link).unwrap();

        assert!(!link.exists());
        assert!(target.is_file());
    }

    #[test]
    fn base64_roundtrip_through_binary_channel() {
        let tmp = TempDir::new("b64");
        let file = tmp.file("blob.bin");
        let payload: Vec<u8> = (0..=255u8).collect();

        write_atomic(&file, &payload).unwrap();
        let read = read_bytes(&file, MAX_BINARY_BYTES).unwrap();
        assert_eq!(read, payload);
    }

    #[test]
    fn reading_a_missing_file_reports_not_found() {
        let tmp = TempDir::new("missing");
        let err = read_text(&tmp.file("nope.txt"), MAX_TEXT_BYTES).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        // Сообщение обязано содержать путь, иначе в IDE оно бесполезно.
        assert!(err.message.contains("nope.txt"), "{}", err.message);
    }

    #[test]
    fn stat_serializes_both_camel_and_snake_case() {
        // Старый бэкенд отдавал snake_case, TS-интерфейс обещает camelCase.
        // Плагины могли писаться под любой вариант, поэтому отдаём оба.
        let tmp = TempDir::new("serde");
        let file = tmp.file("a.txt");
        write_atomic(&file, b"x").unwrap();

        let stat = fs_stat(file.to_string_lossy().to_string()).unwrap().unwrap();
        let json = serde_json::to_value(&stat).unwrap();

        assert_eq!(json["isFile"], true);
        assert_eq!(json["is_file"], true);
        assert_eq!(json["isDir"], false);
        assert_eq!(json["is_dir"], false);
        assert!(json["mtime"].as_u64().unwrap() > 1_600_000_000_000);
    }

    #[test]
    fn dir_entry_serializes_both_cases() {
        let tmp = TempDir::new("serde-dir");
        fs::create_dir(tmp.file("sub")).unwrap();

        let entries = fs_read_dir(tmp.path().to_string_lossy().to_string()).unwrap();
        let json = serde_json::to_value(&entries[0]).unwrap();

        assert_eq!(json["isDir"], true);
        assert_eq!(json["is_dir"], true);
        assert_eq!(json["isSymlink"], false);
        assert_eq!(json["is_symlink"], false);
        assert!(json["isHidden"].is_boolean());
    }

    #[test]
    fn legacy_snake_case_is_still_served() {
        // Регрессия: раньше бэкенд отдавал только snake_case, и плагины,
        // написанные под него, проверяли `entry.is_dir`. Оба варианта должны
        // присутствовать, иначе такие плагины молча ломаются.
        let tmp = TempDir::new("legacy");
        fs::create_dir(tmp.file("folder")).unwrap();

        let entries = fs_read_dir(tmp.path().to_string_lossy().to_string()).unwrap();
        let json = serde_json::to_value(&entries[0]).unwrap();

        assert_eq!(json.get("is_dir").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(json.get("isDir").and_then(|v| v.as_bool()), Some(true));
    }
}
