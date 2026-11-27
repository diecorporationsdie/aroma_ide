//! Ошибки IPC.
//!
//! Формат — одна строка `КОД: сообщение`. Почему строка, а не объект:
//! плагины ловят ошибки как `catch (e) { String(e) }` и показывают текст
//! пользователю. Объект превратился бы в `[object Object]`. При этом код
//! остаётся машиночитаемым:
//!
//! ```js
//! catch (e) {
//!   if (String(e).startsWith('NOT_FOUND:')) { /* ... */ }
//! }
//! ```
//!
//! Коды стабильны — на них можно завязываться в плагинах:
//!
//! | Код | Смысл |
//! |-----|-------|
//! | `NOT_FOUND` | файла/плагина нет |
//! | `PERMISSION_DENIED` | ОС не даёт доступ |
//! | `ALREADY_EXISTS` | цель уже существует |
//! | `NOT_A_DIRECTORY` / `IS_A_DIRECTORY` | не тот тип |
//! | `NOT_UTF8` | файл не текст (используйте бинарный канал) |
//! | `TOO_LARGE` | превышен лимит чтения |
//! | `INVALID_PATH` | путь отвергнут проверкой (побег, ADS, абсолютный) |
//! | `INVALID_ARGUMENT` | некорректные данные вызова |
//! | `CROSS_DEVICE` | операция между разными томами |
//! | `BUSY` | файл занят другим процессом |
//! | `UNSUPPORTED` | операция не поддерживается |
//! | `INTERNAL` | внутренняя ошибка IDE |
//! | `UNKNOWN` | всё остальное |

use serde::{Serialize, Serializer};
use std::fmt;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    IsADirectory,
    NotUtf8,
    TooLarge,
    InvalidPath,
    InvalidArgument,
    CrossDevice,
    Busy,
    Unsupported,
    Internal,
    Unknown,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::PermissionDenied => "PERMISSION_DENIED",
            ErrorCode::AlreadyExists => "ALREADY_EXISTS",
            ErrorCode::NotADirectory => "NOT_A_DIRECTORY",
            ErrorCode::IsADirectory => "IS_A_DIRECTORY",
            ErrorCode::NotUtf8 => "NOT_UTF8",
            ErrorCode::TooLarge => "TOO_LARGE",
            ErrorCode::InvalidPath => "INVALID_PATH",
            ErrorCode::InvalidArgument => "INVALID_ARGUMENT",
            ErrorCode::CrossDevice => "CROSS_DEVICE",
            ErrorCode::Busy => "BUSY",
            ErrorCode::Unsupported => "UNSUPPORTED",
            ErrorCode::Internal => "INTERNAL",
            ErrorCode::Unknown => "UNKNOWN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    /// Ошибка ОС с привязкой к пути.
    pub fn io(path: &Path, err: io::Error) -> Self {
        let mut e = AppError::from(err);
        e.message = format!("{}: {}", path.display(), e.message);
        e
    }

    pub fn too_large(path: &Path, size: u64, limit: u64) -> Self {
        Self::new(
            ErrorCode::TooLarge,
            format!(
                "{}: {} байт при лимите {} — используйте бинарный канал",
                path.display(),
                size,
                limit
            ),
        )
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AppError {}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(windows)]
fn os_code(err: &io::Error) -> ErrorCode {
    match err.raw_os_error() {
        // ERROR_NOT_SAME_DEVICE
        Some(17) => ErrorCode::CrossDevice,
        // ERROR_SHARING_VIOLATION / ERROR_LOCK_VIOLATION
        Some(32) | Some(33) => ErrorCode::Busy,
        // ERROR_DIRECTORY
        Some(267) => ErrorCode::NotADirectory,
        _ => ErrorCode::Unknown,
    }
}

#[cfg(unix)]
fn os_code(err: &io::Error) -> ErrorCode {
    match err.raw_os_error() {
        // EXDEV
        Some(18) => ErrorCode::CrossDevice,
        // EBUSY
        Some(16) => ErrorCode::Busy,
        // ENOTDIR
        Some(20) => ErrorCode::NotADirectory,
        // EISDIR
        Some(21) => ErrorCode::IsADirectory,
        _ => ErrorCode::Unknown,
    }
}

#[cfg(not(any(windows, unix)))]
fn os_code(_err: &io::Error) -> ErrorCode {
    ErrorCode::Unknown
}

impl From<io::Error> for AppError {
    fn from(err: io::Error) -> Self {
        let code = match err.kind() {
            io::ErrorKind::NotFound => ErrorCode::NotFound,
            io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            io::ErrorKind::AlreadyExists => ErrorCode::AlreadyExists,
            io::ErrorKind::InvalidInput => ErrorCode::InvalidArgument,
            _ => os_code(&err),
        };
        AppError {
            code,
            message: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::new(
            ErrorCode::InvalidArgument,
            format!("некорректный JSON: {err}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_code_and_message() {
        let e = AppError::new(ErrorCode::NotFound, "C:\\a.txt: нет файла");
        assert_eq!(e.to_string(), "NOT_FOUND: C:\\a.txt: нет файла");
    }

    #[test]
    fn serializes_to_string() {
        let e = AppError::new(ErrorCode::TooLarge, "big");
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, "\"TOO_LARGE: big\"");
    }

    #[test]
    fn maps_not_found() {
        let e: AppError = io::Error::new(io::ErrorKind::NotFound, "gone").into();
        assert_eq!(e.code, ErrorCode::NotFound);
    }
}
