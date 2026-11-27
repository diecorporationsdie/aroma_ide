//! РќР°СЃС‚СЂРѕР№РєРё: JSON-С„Р°Р№Р» РЅР° namespace.
//!
//! Р§С‚Рѕ Р·РґРµСЃСЊ РІР°Р¶РЅРѕ:
//!
//! - **РђС‚РѕРјР°СЂРЅР°СЏ Р·Р°РїРёСЃСЊ.** Р Р°РЅСЊС€Рµ `fs::write` РјРѕРі Р±С‹С‚СЊ РїСЂРµСЂРІР°РЅ, РѕСЃС‚Р°РІРёРІ
//!   РѕР±СЂРµР·Р°РЅРЅС‹Р№ JSON, вЂ” Рё РїСЂРё СЃР»РµРґСѓСЋС‰РµРј Р·Р°РїСѓСЃРєРµ IDE С‚РµСЂСЏР»Р° РІСЃРµ РЅР°СЃС‚СЂРѕР№РєРё
//!   СЂР°СЃРєР»Р°РґРєРё. РўРµРїРµСЂСЊ РІСЂРµРјРµРЅРЅС‹Р№ С„Р°Р№Р» + `fsync` + `rename`.
//! - **РЎРµСЂРёР°Р»РёР·Р°С†РёСЏ Р·Р°РїРёСЃРµР№.** Р”РІР° РїР»Р°РіРёРЅР°, РїРёС€СѓС‰РёС… РѕРґРЅРѕРІСЂРµРјРµРЅРЅРѕ, СЂР°РЅСЊС€Рµ
//!   РјРѕРіР»Рё РїРµСЂРµС‚РµСЂРµС‚СЊ РґСЂСѓРі РґСЂСѓРіР°. Р—Р°РїРёСЃСЊ РёРґС‘С‚ РїРѕРґ РѕРґРЅРёРј РјСЊСЋС‚РµРєСЃРѕРј.
//! - **Р‘РёС‚С‹Р№ С„Р°Р№Р» РЅРµ СѓР±РёРІР°РµС‚ Р·Р°РїСѓСЃРє.** РџРѕРІСЂРµР¶РґС‘РЅРЅС‹Р№ JSON РѕС‚РєР»Р°РґС‹РІР°РµС‚СЃСЏ РІ
//!   `.corrupt-<РІСЂРµРјСЏ>.json`, Р° IDE СЃС‚Р°СЂС‚СѓРµС‚ СЃ РїСѓСЃС‚С‹РјРё РЅР°СЃС‚СЂРѕР№РєР°РјРё.
//! - **РџСЂРµРґРµР» СЂР°Р·РјРµСЂР°.** Namespace РЅР° СЃРѕС‚РЅРё РјРµРіР°Р±Р°Р№С‚ вЂ” СЌС‚Рѕ СѓР¶Рµ РЅРµ РЅР°СЃС‚СЂРѕР№РєРё.
//! - **РўРѕС‡РµС‡РЅС‹Рµ РѕРїРµСЂР°С†РёРё.** `storage_set`/`storage_delete` РјРµРЅСЏСЋС‚ РѕРґРёРЅ РєР»СЋС‡,
//!   РЅРµ РїРµСЂРµР·Р°РїРёСЃС‹РІР°СЏ РІРµСЃСЊ РѕР±СЉРµРєС‚: РїР»Р°РіРёРЅС‹ РЅРµ Р·Р°С‚РёСЂР°СЋС‚ РґР°РЅРЅС‹Рµ РґСЂСѓРі РґСЂСѓРіР°.

use crate::error::{AppError, AppResult};
use crate::fs;
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime, State};

/// Р‘РѕР»СЊС€Рµ СЌС‚РѕРіРѕ namespace РЅРµ С‡РёС‚Р°РµРј.
const MAX_NAMESPACE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub namespace: String,
    pub path: String,
    pub size: u64,
    pub exists: bool,
}

/// РћРґРёРЅ РјСЊСЋС‚РµРєСЃ РЅР° РІСЃРµ namespace: Р·Р°РїРёСЃРё СЂРµРґРєРёРµ (СЂР°СЃРєР»Р°РґРєР°, РЅР°СЃС‚СЂРѕР№РєРё
/// РїР»Р°РіРёРЅРѕРІ), Р° РіРѕРЅРєР° В«РїСЂРѕС‡РёС‚Р°Р» вЂ” РёР·РјРµРЅРёР» вЂ” Р·Р°РїРёСЃР°Р»В» РјРµР¶РґСѓ РґРІСѓРјСЏ РїР»Р°РіРёРЅР°РјРё
/// СЂРµР°Р»СЊРЅР°.
#[derive(Default)]
pub struct StorageState {
    write_lock: Mutex<()>,
}

impl StorageState {
    fn guard(&self) -> parking_lot::MutexGuard<'_, ()> {
        self.write_lock.lock()
    }
}

fn storage_dir<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    let base = app
        .path()
        .app_config_dir()
        .map_err(|e| AppError::internal(e.to_string()))?;
    let dir = base.join("storage");
    std::fs::create_dir_all(&dir).map_err(|e| AppError::io(&dir, e))?;
    Ok(dir)
}

/// Namespace РїСЂРµРІСЂР°С‰Р°РµС‚СЃСЏ РІ РёРјСЏ С„Р°Р№Р»Р°. Р Р°Р·СЂРµС€РµРЅС‹ Р±СѓРєРІС‹, С†РёС„СЂС‹, `.`, `-`, `_`;
/// РѕСЃС‚Р°Р»СЊРЅРѕРµ Р·Р°РјРµРЅСЏРµС‚СЃСЏ РЅР° `_`, С‡С‚РѕР±С‹ `../../etc/passwd` РЅРµ СЃС‚Р°Р» РїСѓС‚С‘Рј.
///
/// Windows РјРѕР»С‡Р° РѕС‚Р±СЂР°СЃС‹РІР°РµС‚ С…РІРѕСЃС‚РѕРІС‹Рµ С‚РѕС‡РєРё Рё РїСЂРѕР±РµР»С‹, РїРѕСЌС‚РѕРјСѓ РёРјСЏ
/// РЅРѕСЂРјР°Р»РёР·СѓРµС‚СЃСЏ: РёРЅР°С‡Рµ РґРІР° СЂР°Р·РЅС‹С… namespace РјРѕРіР»Рё Р±С‹ РїРѕРїР°СЃС‚СЊ РІ РѕРґРёРЅ С„Р°Р№Р».
fn safe_name(namespace: &str) -> String {
    let mut out = String::with_capacity(namespace.len());
    for c in namespace.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }

    if out.is_empty() {
        return "default".to_string();
    }

    // РРјСЏ РёР· РѕРґРЅРёС… С‚РѕС‡РµРє (`..`, `...`) РћРЎ С‚СЂР°РєС‚СѓРµС‚ РєР°Рє СЃСЃС‹Р»РєСѓ РЅР° РєР°С‚Р°Р»РѕРі,
    // РїРѕСЌС‚РѕРјСѓ Р·Р°РјРµРЅСЏРµРј РµРіРѕ РЅР° РїСЂРµРґСЃРєР°Р·СѓРµРјРѕРµ.
    if out.chars().all(|c| c == '.') {
        return format!("dots-{}", out.len());
    }

    let trimmed = out.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        format!("_{out}")
    } else if trimmed.len() != out.len() {
        trimmed.to_string()
    } else {
        out
    }
}

fn storage_path<R: Runtime>(app: &AppHandle<R>, namespace: &str) -> AppResult<PathBuf> {
    Ok(storage_dir(app)?.join(format!("{}.json", safe_name(namespace))))
}

fn read_namespace(path: &Path) -> AppResult<Map<String, Value>> {
    if !path.is_file() {
        return Ok(Map::new());
    }

    let text = fs::read_text(path, MAX_NAMESPACE_BYTES)?.content;

    if text.trim().is_empty() {
        return Ok(Map::new());
    }

    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => {
            // РќР°СЃС‚СЂРѕР№РєРё РѕР±СЏР·Р°РЅС‹ Р±С‹С‚СЊ РѕР±СЉРµРєС‚РѕРј: РјР°СЃСЃРёРІ РёР»Рё С‡РёСЃР»Рѕ вЂ” СЌС‚Рѕ Р»РёР±Рѕ
            // РїРѕРІСЂРµР¶РґРµРЅРёРµ, Р»РёР±Рѕ С‡СѓР¶Р°СЏ Р·Р°РїРёСЃСЊ.
            quarantine(path);
            Ok(Map::new())
        }
        Err(e) => {
            log::error!("{}: Р±РёС‚С‹Р№ JSON ({e}), С„Р°Р№Р» РѕС‚Р»РѕР¶РµРЅ", path.display());
            quarantine(path);
            Ok(Map::new())
        }
    }
}

/// РћС‚РєР»Р°РґС‹РІР°РµС‚ РїРѕРІСЂРµР¶РґС‘РЅРЅС‹Р№ С„Р°Р№Р», С‡С‚РѕР±С‹ РЅРµ РїРѕС‚РµСЂСЏС‚СЊ РґР°РЅРЅС‹Рµ РјРѕР»С‡Р°.
fn quarantine(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_extension(format!("corrupt-{stamp}.json"));
    match std::fs::rename(path, &backup) {
        Ok(()) => log::warn!("РїРѕРІСЂРµР¶РґС‘РЅРЅС‹Рµ РЅР°СЃС‚СЂРѕР№РєРё СЃРѕС…СЂР°РЅРµРЅС‹ РєР°Рє {}", backup.display()),
        Err(e) => log::error!("{}: РЅРµ СѓРґР°Р»РѕСЃСЊ РѕС‚Р»РѕР¶РёС‚СЊ Р±РёС‚С‹Р№ С„Р°Р№Р»: {e}", path.display()),
    }
}

fn write_namespace(path: &Path, map: &Map<String, Value>) -> AppResult<()> {
    let text = serde_json::to_string(&Value::Object(map.clone()))?;
    fs::write_atomic(path, text.as_bytes())
}

// ---------------------------------------------------------------------------
// РљРѕРјР°РЅРґС‹
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn storage_read<R: Runtime>(
    app: AppHandle<R>,
    namespace: String,
) -> AppResult<Map<String, Value>> {
    read_namespace(&storage_path(&app, &namespace)?)
}

/// РџРѕР»РЅР°СЏ Р·Р°РјРµРЅР° СЃРѕРґРµСЂР¶РёРјРѕРіРѕ. `merge` РїРѕРґРјРµС€РёРІР°РµС‚ Рє С‚РµРєСѓС‰РµРјСѓ СЃРѕСЃС‚РѕСЏРЅРёСЋ РІРјРµСЃС‚Рѕ
/// Р·Р°РјРµРЅС‹ вЂ” СЌС‚Рѕ Р·Р°С‰РёС‰Р°РµС‚ РѕС‚ РїРѕС‚РµСЂРё РєР»СЋС‡РµР№ РїСЂРё РїР°СЂР°Р»Р»РµР»СЊРЅРѕР№ СЂР°Р±РѕС‚Рµ РїР»Р°РіРёРЅРѕРІ.
#[tauri::command]
pub fn storage_write<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, StorageState>,
    namespace: String,
    data: Map<String, Value>,
    merge: Option<bool>,
) -> AppResult<()> {
    let path = storage_path(&app, &namespace)?;
    let _guard = state.guard();

    let payload = if merge.unwrap_or(false) {
        let mut current = read_namespace(&path)?;
        current.extend(data);
        current
    } else {
        data
    };

    write_namespace(&path, &payload)
}

/// РўРѕС‡РµС‡РЅР°СЏ Р·Р°РїРёСЃСЊ РѕРґРЅРѕРіРѕ РєР»СЋС‡Р°: РЅРµ РїРµСЂРµС‚РёСЂР°РµС‚ С‡СѓР¶РёРµ РёР·РјРµРЅРµРЅРёСЏ.
#[tauri::command]
pub fn storage_set<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, StorageState>,
    namespace: String,
    key: String,
    value: Value,
) -> AppResult<()> {
    let path = storage_path(&app, &namespace)?;
    let _guard = state.guard();

    let mut map = read_namespace(&path)?;
    map.insert(key, value);
    write_namespace(&path, &map)
}

/// РЈРґР°Р»РµРЅРёРµ РєР»СЋС‡Р° РёР»Рё РІСЃРµРіРѕ namespace (`key` РЅРµ РїРµСЂРµРґР°РЅ).
#[tauri::command]
pub fn storage_delete<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, StorageState>,
    namespace: String,
    key: Option<String>,
) -> AppResult<()> {
    let path = storage_path(&app, &namespace)?;
    let _guard = state.guard();

    match key {
        Some(key) => {
            let mut map = read_namespace(&path)?;
            if map.remove(&key).is_none() {
                return Ok(());
            }
            write_namespace(&path, &map)
        }
        None => match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::io(&path, e)),
        },
    }
}

#[tauri::command]
pub fn storage_info<R: Runtime>(app: AppHandle<R>, namespace: String) -> AppResult<StorageInfo> {
    let path = storage_path(&app, &namespace)?;
    let meta = std::fs::metadata(&path).ok();
    Ok(StorageInfo {
        namespace,
        path: path.to_string_lossy().to_string(),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        exists: meta.is_some(),
    })
}

/// РЎРїРёСЃРѕРє namespace СЃ СЂР°Р·РјРµСЂР°РјРё вЂ” РґР»СЏ РїР°РЅРµР»Рё В«Р”Р°РЅРЅС‹Рµ РїР»Р°РіРёРЅРѕРІВ».
#[tauri::command]
pub fn storage_list<R: Runtime>(app: AppHandle<R>) -> AppResult<Vec<StorageInfo>> {
    let dir = storage_dir(&app)?;
    let mut out = Vec::new();
    let readdir = std::fs::read_dir(&dir).map_err(|e| AppError::io(&dir, e))?;
    for entry in readdir.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        out.push(StorageInfo {
            namespace: stem.to_string(),
            path: path.to_string_lossy().to_string(),
            size: entry.metadata().map(|m| m.len()).unwrap_or(0),
            exists: true,
        });
    }
    out.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sanitized() {
        assert_eq!(safe_name("plugin.editor"), "plugin.editor");
        // Р Р°Р·РґРµР»РёС‚РµР»Рё РїСѓС‚РµР№ РёСЃС‡РµР·Р°СЋС‚: `..` РѕСЃС‚Р°С‘С‚СЃСЏ С‚РµРєСЃС‚РѕРј, Р° РЅРµ РєРѕРјРїРѕРЅРµРЅС‚РѕРј.
        assert_eq!(safe_name("../../etc/passwd"), ".._.._etc_passwd");
        assert!(!safe_name("../../etc/passwd").contains('/'));
        assert!(!safe_name("../../etc/passwd").contains('\\'));
        assert_eq!(safe_name("a/b\\c"), "a_b_c");
        assert_eq!(safe_name(".."), "dots-2");
        assert_eq!(safe_name(""), "default");
    }
}

