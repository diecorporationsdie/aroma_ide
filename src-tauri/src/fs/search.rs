//! Поиск по проекту.
//!
//! Почему в Rust, а не в плагине: чтобы найти `useState` в 40 000 файлов,
//! JS-плагину пришлось бы прочитать каждый файл через IPC — десятки тысяч
//! переходов через мост и секунды ожидания. Здесь обход и чтение идут в одном
//! процессе, а наверх уходят только совпадения.
//!
//! Ограничения обязательны: без них поиск по `C:\` съест память и время.
//! Поэтому есть лимиты на число файлов, размер файла и число совпадений, а
//! результат помечается `truncated`, чтобы UI честно сказал «показаны не все».

use crate::error::{AppError, AppResult, ErrorCode};
use crate::fs;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Каталоги, которые почти никогда не нужны в поиске и всегда огромны.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    ".hg",
    ".svn",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".cache",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    "vendor",
    "obj",
];

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    pub query: String,
    pub roots: Vec<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    /// Глобы имён файлов: `*.ts`, `*.css`.
    #[serde(default)]
    pub include: Vec<String>,
    /// Глобы, которые нужно исключить (имя файла или каталога).
    #[serde(default)]
    pub exclude: Vec<String>,
    pub max_results: Option<usize>,
    pub max_file_size: Option<u64>,
    pub max_files: Option<usize>,
    /// Сколько строк контекста до и после совпадения.
    pub context_lines: Option<usize>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchMatch {
    pub path: String,
    /// 1-based.
    pub line: u32,
    /// 1-based, в символах.
    pub column: u32,
    /// Вся строка, где найдено совпадение.
    pub text: String,
    /// Смещение совпадения внутри `text` (в символах).
    pub match_start: u32,
    pub match_end: u32,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    pub files_scanned: usize,
    pub files_matched: usize,
    pub elapsed_ms: u64,
    /// Каталоги, которые не удалось прочитать (нет прав и т.п.).
    pub errors: Vec<String>,
}

pub fn search(options: SearchOptions) -> AppResult<SearchResult> {
    let started = std::time::Instant::now();

    if options.query.is_empty() {
        return Err(AppError::new(
            ErrorCode::InvalidArgument,
            "пустой поисковый запрос",
        ));
    }
    if options.roots.is_empty() {
        return Err(AppError::new(
            ErrorCode::InvalidArgument,
            "не указаны каталоги для поиска",
        ));
    }

    let pattern = build_pattern(&options)?;
    let max_results = options.max_results.unwrap_or(2_000).clamp(1, 50_000);
    let max_file_size = options.max_file_size.unwrap_or(8 * 1024 * 1024);
    let max_files = options.max_files.unwrap_or(200_000).clamp(1, 1_000_000);
    let context = options.context_lines.unwrap_or(0).min(5);

    let mut result = SearchResult {
        matches: Vec::new(),
        truncated: false,
        files_scanned: 0,
        files_matched: 0,
        elapsed_ms: 0,
        errors: Vec::new(),
    };

    let mut visited: HashSet<PathBuf> = HashSet::new();

    'roots: for root in &options.roots {
        let root_path = Path::new(root);
        if !root_path.is_dir() {
            result.errors.push(format!("{root}: не каталог"));
            continue;
        }

        let mut stack: Vec<PathBuf> = vec![root_path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let readdir = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) => {
                    result.errors.push(format!("{}: {e}", dir.display()));
                    continue;
                }
            };

            for entry in readdir {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        result.errors.push(format!("{}: {e}", dir.display()));
                        continue;
                    }
                };

                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Симлинки не разворачиваем: цикл в дереве — это зависание.
                let file_type = match entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if file_type.is_symlink() {
                    continue;
                }
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if meta.is_dir() {
                    if is_skipped_dir(&name, &options.exclude) {
                        continue;
                    }
                    stack.push(path);
                    continue;
                }

                if !meta.is_file() {
                    continue;
                }
                if !options.include.is_empty() && !matches_any(&name, &options.include) {
                    continue;
                }
                if matches_any(&name, &options.exclude) {
                    continue;
                }
                if meta.len() > max_file_size {
                    continue;
                }
                if !visited.insert(path.clone()) {
                    continue;
                }

                result.files_scanned += 1;
                if result.files_scanned > max_files {
                    result.truncated = true;
                    break 'roots;
                }

                let text = match fs::read_text(&path, max_file_size) {
                    Ok(f) => f.content,
                    // Бинарные и нечитаемые файлы пропускаем: это поиск по коду.
                    Err(_) => continue,
                };

                let mut file_had_match = false;
                let lines: Vec<&str> = text.lines().collect();

                for (idx, line) in lines.iter().enumerate() {
                    for m in pattern.find_iter(line) {
                        if result.matches.len() >= max_results {
                            result.truncated = true;
                            break 'roots;
                        }

                        let before = if context > 0 {
                            lines[idx.saturating_sub(context)..idx]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let after = if context > 0 {
                            let end = (idx + context + 1).min(lines.len());
                            lines[idx + 1..end].iter().map(|s| s.to_string()).collect()
                        } else {
                            Vec::new()
                        };

                        let start_chars = line[..m.start()].chars().count() as u32;
                        let len_chars = line[m.start()..m.end()].chars().count() as u32;

                        result.matches.push(SearchMatch {
                            path: path.to_string_lossy().to_string(),
                            line: (idx + 1) as u32,
                            column: start_chars + 1,
                            text: (*line).to_string(),
                            match_start: start_chars,
                            match_end: start_chars + len_chars,
                            before,
                            after,
                        });
                        file_had_match = true;
                    }
                }

                if file_had_match {
                    result.files_matched += 1;
                }
            }
        }
    }

    result.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(result)
}

fn build_pattern(options: &SearchOptions) -> AppResult<Regex> {
    let mut source = if options.regex {
        options.query.clone()
    } else {
        regex::escape(&options.query)
    };

    if options.whole_word {
        source = format!(r"\b(?:{source})\b");
    }

    RegexBuilder::new(&source)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|e| {
            AppError::new(
                ErrorCode::InvalidArgument,
                format!("некорректное регулярное выражение: {e}"),
            )
        })
}

fn is_skipped_dir(name: &str, exclude: &[String]) -> bool {
    if SKIP_DIRS.iter().any(|d| d.eq_ignore_ascii_case(name)) {
        return true;
    }
    matches_any(name, exclude)
}

fn matches_any(name: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|p| glob_match(p, name))
}

/// Простой глоб: `*` — любая последовательность, `?` — один символ.
/// Регистр не важен: на Windows `SRC` и `src` — одно и то же.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pat: Vec<char> = pattern.to_lowercase().chars().collect();
    let text: Vec<char> = name.to_lowercase().chars().collect();
    glob_inner(&pat, &text)
}

fn glob_inner(pat: &[char], text: &[char]) -> bool {
    if pat.is_empty() {
        return text.is_empty();
    }
    match pat[0] {
        '*' => (0..=text.len()).any(|i| glob_inner(&pat[1..], &text[i..])),
        '?' => !text.is_empty() && glob_inner(&pat[1..], &text[1..]),
        c => !text.is_empty() && text[0] == c && glob_inner(&pat[1..], &text[1..]),
    }
}

// ---------------------------------------------------------------------------
// Команда
// ---------------------------------------------------------------------------

/// Поиск по проекту. `async` + `spawn_blocking`: обход дерева — работа на
/// секунды, и выполнять её на потоке обработки команд нельзя, иначе интерфейс
/// замирает на весь поиск.
#[tauri::command]
pub async fn fs_search(options: SearchOptions) -> AppResult<SearchResult> {
    tauri::async_runtime::spawn_blocking(move || search(options))
        .await
        .map_err(|e| AppError::internal(format!("поиск прерван: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match("*.ts", "main.ts"));
        assert!(glob_match("*.ts", "MAIN.TS"));
        assert!(!glob_match("*.ts", "main.tsx"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("index.*", "index.js"));
    }

    #[test]
    fn skip_dirs() {
        assert!(is_skipped_dir("node_modules", &[]));
        assert!(is_skipped_dir("NODE_MODULES", &[]));
        assert!(is_skipped_dir("custom", &["custom".to_string()]));
        assert!(!is_skipped_dir("src", &[]));
    }

    #[test]
    fn literal_pattern_by_default() {
        let opts = SearchOptions {
            query: "a.b".into(),
            roots: vec![".".into()],
            regex: false,
            case_sensitive: false,
            whole_word: false,
            include: vec![],
            exclude: vec![],
            max_results: None,
            max_file_size: None,
            max_files: None,
            context_lines: None,
        };
        let re = build_pattern(&opts).unwrap();
        assert!(re.is_match("a.b"));
        // Точка должна быть литералом, а не «любым символом».
        assert!(!re.is_match("axb"));
    }

    #[test]
    fn search_finds_matches_and_columns() {
        let tmp = std::env::temp_dir().join(format!("aroma-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(
            tmp.join("src/a.ts"),
            "const a = 1;\nconst target = 2;\n",
        )
        .unwrap();
        std::fs::write(tmp.join("src/b.md"), "target in markdown\n").unwrap();

        let result = search(SearchOptions {
            query: "target".into(),
            roots: vec![tmp.to_string_lossy().to_string()],
            regex: false,
            case_sensitive: true,
            whole_word: false,
            include: vec!["*.ts".into()],
            exclude: vec![],
            max_results: None,
            max_file_size: None,
            max_files: None,
            context_lines: Some(1),
        })
        .unwrap();

        // include = *.ts отсекает markdown.
        assert_eq!(result.matches.len(), 1, "{:?}", result.matches);
        assert_eq!(result.matches[0].line, 2);
        assert_eq!(result.matches[0].column, 7);
        assert_eq!(result.matches[0].before.len(), 1);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn respects_max_results() {
        let tmp = std::env::temp_dir().join(format!("aroma-search-max-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.txt"), "x\nx\nx\nx\nx\n").unwrap();

        let result = search(SearchOptions {
            query: "x".into(),
            roots: vec![tmp.to_string_lossy().to_string()],
            regex: false,
            case_sensitive: false,
            whole_word: false,
            include: vec![],
            exclude: vec![],
            max_results: Some(2),
            max_file_size: None,
            max_files: None,
            context_lines: None,
        })
        .unwrap();

        assert_eq!(result.matches.len(), 2);
        assert!(result.truncated);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
