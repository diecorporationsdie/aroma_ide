mod fs;
mod plugins;
mod pty;
mod storage;

use std::collections::HashMap;
use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(pty::PtyState(Mutex::new(HashMap::new())))
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            plugins::plugins_scan,
            plugins::plugins_read,
            storage::storage_read,
            storage::storage_write,
            fs::fs_read_file,
            fs::fs_write_file,
            fs::fs_read_dir,
            fs::fs_stat,
            fs::fs_exists,
            fs::fs_mkdir,
            fs::fs_remove,
            fs::fs_rename,
            fs::fs_home_dir,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
