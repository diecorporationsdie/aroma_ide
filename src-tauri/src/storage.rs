use tauri::{AppHandle, Manager, Runtime};

use serde_json::Value;
use std::path::PathBuf;


fn storage_path<R: Runtime>(app: &AppHandle<R>, namespace: &str) -> Result<PathBuf, String> {
    let base = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let dir = base.join("storage");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let safe: String = namespace
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    Ok(dir.join(format!("{safe}.json")))
}

#[tauri::command]
pub fn storage_read<R: Runtime>(app: AppHandle<R>, namespace: String) -> Result<Value, String> {
    let path = storage_path(&app, &namespace)?;
    if !path.is_file() {
        return Ok(Value::Object(Default::default()));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn storage_write<R: Runtime>(
    app: AppHandle<R>,
    namespace: String,
    data: Value,
) -> Result<(), String> {
    let path = storage_path(&app, &namespace)?;
    let text = serde_json::to_string(&data).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| e.to_string())
}
