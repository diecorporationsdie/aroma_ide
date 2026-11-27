//! Проверка и нормализация путей.
//!
//! Задача модуля — не дать плагину выйти за пределы каталога, который ему
//! разрешён, и при этом корректно вести себя на Windows: буква диска,
//! UNC-пути, ADS (`file.txt:stream`), symlink-побеги.
//!
//! Важно: `Path::join` с абсолютным путём **заменяет** базу. Поэтому
//! `root.join(user_input)` — не безопасная операция, и здесь она запрещена:
//! сначала проверяются компоненты, потом собирается путь.

use crate::error::{AppError, AppResult, ErrorCode};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

/// Идентификатор плагина — ровно одно имя каталога.
pub fn validate_plugin_id(id: &str) -> AppResult<()> {
    if id.is_empty() {
        return Err(AppError::new(ErrorCode::InvalidPath, "пустой id плагина"));
    }
    if id.contains('\0') {
        return Err(AppError::new(
            ErrorCode::InvalidPath,
            "id плагина содержит NUL",
        ));
    }
    if id == "." || id == ".." || id.contains(['/', '\\', ':']) {
        return Err(AppError::new(
            ErrorCode::InvalidPath,
            format!("недопустимый id плагина: {id}"),
        ));
    }
    Ok(())
}

/// Относительный путь внутри разрешённого каталога.
///
/// Отклоняет абсолютные пути (включая `C:` и `\\server`), компоненты `..`,
/// пустые строки, NUL и ADS-потоки (`file.txt:stream`).
pub fn validate_relative(rel: &str) -> AppResult<PathBuf> {
    if rel.trim().is_empty() {
        return Err(AppError::new(ErrorCode::InvalidPath, "пустой путь"));
    }
    if rel.contains('\0') {
        return Err(AppError::new(ErrorCode::InvalidPath, "путь содержит NUL"));
    }

    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(AppError::new(
            ErrorCode::InvalidPath,
            format!("ожидался относительный путь: {rel}"),
        ));
    }

    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                // `:` в имени файла — это ADS-поток (`file.txt:hidden`), а не
                // отдельный файл. Плагину он не нужен, а обходить так проверки
                // слишком легко.
                if text.contains(':') {
                    return Err(AppError::new(
                        ErrorCode::InvalidPath,
                        format!("недопустимый компонент пути: {text}"),
                    ));
                }
                out.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(AppError::new(
                    ErrorCode::InvalidPath,
                    format!("выход из каталога запрещён: {rel}"),
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::new(
                    ErrorCode::InvalidPath,
                    format!("ожидался относительный путь: {rel}"),
                ));
            }
        }
    }

    if out.as_os_str().is_empty() {
        return Err(AppError::new(ErrorCode::InvalidPath, "пустой путь"));
    }
    Ok(out)
}

/// `candidate` лежит внутри `base` (или совпадает с ним).
///
/// Сравнение покомпонентное: `/a/bc` **не** считается вложенным в `/a/b`.
/// На Windows регистр не важен.
pub fn is_within(base: &Path, candidate: &Path) -> bool {
    let mut b = base.components();
    let mut c = candidate.components();
    loop {
        match (b.next(), c.next()) {
            (None, _) => return true,
            (Some(x), Some(y)) => {
                if !component_eq(x, y) {
                    return false;
                }
            }
            (Some(_), None) => return false,
        }
    }
}

fn component_eq(a: Component<'_>, b: Component<'_>) -> bool {
    #[cfg(windows)]
    {
        a.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

/// Собирает `base/rel` и проверяет, что физический путь остаётся в `base`.
///
/// Работает и для ещё не существующих файлов: канонизируется ближайший
/// существующий предок (это ловит symlink-побеги), остаток дописывается.
pub fn resolve_within(base: &Path, rel: &str) -> AppResult<PathBuf> {
    let rel_path = validate_relative(rel)?;
    let base_canon = base.canonicalize().map_err(|e| AppError::io(base, e))?;

    let joined = base_canon.join(&rel_path);

    let mut probe = joined.clone();
    let mut tail: Vec<OsString> = Vec::new();

    while !probe.exists() {
        if probe == base_canon {
            break;
        }
        match (probe.parent(), probe.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                probe = parent.to_path_buf();
            }
            _ => {
                return Err(AppError::new(
                    ErrorCode::InvalidPath,
                    format!("{}: не удалось разобрать путь", joined.display()),
                ))
            }
        }
    }

    let canon = probe.canonicalize().map_err(|e| AppError::io(&probe, e))?;
    if !is_within(&base_canon, &canon) {
        return Err(AppError::new(
            ErrorCode::InvalidPath,
            format!(
                "{}: путь ведёт за пределы каталога (symlink?)",
                joined.display()
            ),
        ));
    }

    let mut out = canon;
    for part in tail.iter().rev() {
        out.push(part);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_id_is_one_component() {
        assert!(validate_plugin_id("explorer").is_ok());
        assert!(validate_plugin_id("my.plugin-2").is_ok());
        assert!(validate_plugin_id("..").is_err());
        assert!(validate_plugin_id("a/b").is_err());
        assert!(validate_plugin_id("a\\b").is_err());
        assert!(validate_plugin_id("C:").is_err());
        assert!(validate_plugin_id("").is_err());
    }

    #[test]
    fn relative_rejects_escape() {
        assert!(validate_relative("index.js").is_ok());
        assert!(validate_relative("assets/icon.png").is_ok());
        assert!(validate_relative("../secret").is_err());
        assert!(validate_relative("a/../../b").is_err());
        assert!(validate_relative("").is_err());
        assert!(validate_relative("   ").is_err());
    }

    #[test]
    fn relative_rejects_absolute_and_ads() {
        assert!(validate_relative("C:\\Windows\\win.ini").is_err());
        assert!(validate_relative("\\\\server\\share").is_err());
        assert!(validate_relative("/etc/passwd").is_err());
        assert!(validate_relative("file.txt:stream").is_err());
    }

    #[test]
    fn within_is_component_based() {
        let base = Path::new("/a/b");
        assert!(is_within(base, Path::new("/a/b/c")));
        assert!(is_within(base, Path::new("/a/b")));
        assert!(!is_within(base, Path::new("/a/bc")));
        assert!(!is_within(base, Path::new("/a")));
    }

    #[test]
    fn resolve_keeps_inside() {
        let tmp = std::env::temp_dir().join(format!("aroma-paths-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("sub")).unwrap();

        let ok = resolve_within(&tmp, "sub/new.txt").unwrap();
        assert!(is_within(&tmp.canonicalize().unwrap(), &ok));

        assert!(resolve_within(&tmp, "../escape.txt").is_err());
        assert!(resolve_within(&tmp, "C:\\Windows\\win.ini").is_err());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
