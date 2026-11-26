use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Serialize, Clone, Debug)]
pub struct PluginEntry {
    pub id: String,
    pub manifest_json: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct PluginRoot {
    pub kind: &'static str,
    pub path: PathBuf,
}

fn candidate_roots<R: Runtime>(app: &AppHandle<R>) -> Vec<PluginRoot> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();

    let mut push_root = |kind: &'static str, path: PathBuf| {
        let canon = path.canonicalize().unwrap_or(path);
        if seen.insert(canon.clone()) {
            roots.push(PluginRoot { kind, path: canon });
        }
    };

    // 1. Встроенные ресурсы Tauri (проверяем оба варианта вложенности)
    if let Ok(dir) = app.path().resource_dir() {
        for sub in ["plugins", "resources/plugins"] {
            let p = dir.join(sub);
            if p.is_dir() {
                push_root("bundled", p);
            }
        }
    }

    // 2. Папка `plugins` строго рядом с .exe (работает в portable/release билдах)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let p = exe_dir.join("plugins");
            if p.is_dir() {
                push_root("exe_sibling", p);
            }
        }
    }

    // 3. Пользовательские плагины в AppData / Config
    if let Ok(dir) = app.path().app_config_dir() {
        let p = dir.join("plugins");
        let _ = std::fs::create_dir_all(&p);
        push_root("user", p);
    }

    // 4. Dev-режим (ищем от CARGO_MANIFEST_DIR и cwd вверх по дереву)
    if cfg!(debug_assertions) {
        for p in dev_plugin_dirs() {
            push_root("dev", p);
        }
    }

    roots
}

fn dev_plugin_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    let mut start_dirs = Vec::new();
    // CARGO_MANIFEST_DIR на этапе компиляции всегда указывает прямо на src-tauri
    start_dirs.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
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

/// Порядок приоритета: bundled -> exe_sibling -> user -> dev.
/// Более поздние перезаписывают более ранние по id.
#[tauri::command]
pub fn plugins_scan<R: Runtime>(app: AppHandle<R>) -> Vec<PluginEntry> {
    let roots = candidate_roots(&app);
    println!("[plugins_scan] roots = {:#?}", roots);

    let mut entries: Vec<PluginEntry> = Vec::new();
    for root in roots {
        let Ok(readdir) = std::fs::read_dir(&root.path) else {
            continue;
        };
        for e in readdir.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let manifest_path = p.join("plugin.json");
            let Ok(manifest_json) = std::fs::read_to_string(&manifest_path) else {
                println!("[plugins_scan] skip dir without plugin.json: {:?}", p);
                continue;
            };
            let Some(id) = p.file_name().and_then(|s| s.to_str()).map(String::from) else {
                continue;
            };
            println!("[plugins_scan] found plugin '{}' in {:?}", id, root.kind);
            entries.retain(|x| x.id != id);
            entries.push(PluginEntry { id, manifest_json });
        }
    }
    entries
}

#[tauri::command]
pub fn plugins_read<R: Runtime>(
    app: AppHandle<R>,
    plugin_id: String,
    file: String,
) -> Result<String, String> {
    if plugin_id.contains("..")
        || plugin_id.contains('/')
        || plugin_id.contains('\\')
        || file.contains("..")
        || file.contains('/')
        || file.contains('\\')
    {
        return Err("invalid path".into());
    }

    // Идем в обратном порядке (.rev()), чтобы приоритет совпадал с plugins_scan
    for root in candidate_roots(&app).into_iter().rev() {
        let p = Path::new(&root.path).join(&plugin_id).join(&file);
        if p.is_file() {
            return std::fs::read_to_string(p).map_err(|e| e.to_string());
        }
    }
    Err(format!("file not found: {plugin_id}/{file}"))
}
