//! Псевдотерминал: интерактивный шелл внутри IDE.
//!
//! Что здесь важно и почему:
//!
//! - **Запись не блокирует IDE.** Данные уходят в ограниченную очередь, а
//!   отдельный поток пишет в PTY. Иначе дочерний процесс, переставший читать
//!   stdin (например, `less` на паузе), блокирует `write_all`, а вместе с ним
//!   мьютекс реестра — и перестают работать resize и kill для *всех* сессий.
//! - **Выход определяется опросом процесса, а не EOF.** На Windows ConPTY
//!   читатель **не получает EOF** после завершения дочернего процесса: дескриптор
//!   остаётся открытым, и `read` висит вечно. Если ждать EOF, `pty:exit` не
//!   приходит никогда, и терминал навсегда остаётся «занятым» — именно это
//!   и происходило. Поэтому отдельный поток опрашивает `try_wait`.
//! - **Убивается дерево процессов.** `cmd.exe` → `node` → `esbuild` — это три
//!   процесса. Убийство только шелла оставляет сирот, которые держат файлы и
//!   порты; на Windows поэтому `taskkill /T /F`.
//! - **Никаких гонок при мгновенном выходе.** Сессия попадает в реестр *до*
//!   запуска потоков, поэтому выход всегда находит свой обработчик.

use crate::error::{AppError, AppResult, ErrorCode};
use crate::stream::{Scrollback, StreamHub, StreamKind};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use tauri::State;

/// Сколько последних байт терминала помним для переподключения.
const SCROLLBACK_BYTES: usize = 512 * 1024;
/// Очередь на запись. Ограничена, чтобы зависший процесс не съел память.
const WRITE_QUEUE: usize = 256;
/// Размер одного чтения из PTY.
const READ_CHUNK: usize = 16 * 1024;
/// Период опроса завершения процесса.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(30);

/// Сколько завершённых сессий держим в памяти ради истории вывода.
///
/// Процесс завершился, но пользователю всё ещё нужен его вывод: последние
/// строки `cargo build`, сообщение об ошибке, финальный лог. Поэтому сессия
/// не удаляется сразу — она помечается `alive = false` и вытесняется только
/// когда таких сессий станет слишком много.
const MAX_FINISHED_SESSIONS: usize = 16;

pub struct PtySession {
    pub id: String,
    pub pid: Option<u32>,
    /// `MasterPty` не `Sync`, поэтому под мьютексом: resize вызывается из
    /// команд, а держать мастер открытым нужно на всё время жизни сессии.
    /// `Option` — чтобы поток-наблюдатель мог забрать и закрыть его, разбудив
    /// читателя, застрявшего в `read` на ConPTY.
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    write_tx: SyncSender<Vec<u8>>,
    child: Mutex<Option<Box<dyn Child + Send + Sync>>>,
    alive: AtomicBool,
    cols: AtomicU16,
    rows: AtomicU16,
    /// Когда процесс завершился: по этому времени вытесняем старые сессии.
    finished_at: Mutex<Option<std::time::Instant>>,
    pub scrollback: Arc<Scrollback>,
}

impl PtySession {
    fn kill_tree(&self) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }

        #[cfg(windows)]
        if let Some(pid) = self.pid {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }

        let child = self.child.lock().take();
        if let Some(mut child) = child {
            let _ = child.kill();
            // Reap: иначе остаётся зомби-запись в таблице процессов.
            let _ = child.wait();
        }

        // Закрываем мастер, чтобы разбудить читателя, если он висит в `read`.
        self.master.lock().take();
    }
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PtyInfo {
    pub id: String,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PtySpawned {
    pub id: String,
    pub pid: Option<u32>,
}

type Registry = Arc<Mutex<HashMap<String, Arc<PtySession>>>>;

/// Вытесняет самые старые завершённые сессии, кроме `keep`.
///
/// Живые сессии не трогаем никогда: вытеснение истории не должно убивать
/// работающий терминал.
fn evict_finished(registry: &Registry, keep: &str) {
    let mut guard = registry.lock();

    let mut finished: Vec<(String, std::time::Instant)> = guard
        .iter()
        .filter(|(id, session)| {
            id.as_str() != keep && !session.alive.load(Ordering::SeqCst)
        })
        .filter_map(|(id, session)| {
            session
                .finished_at
                .lock()
                .map(|at| (id.clone(), at))
        })
        .collect();

    if finished.len() <= MAX_FINISHED_SESSIONS {
        return;
    }

    // Сначала уходят самые старые.
    finished.sort_by_key(|(_, at)| *at);
    let excess = finished.len() - MAX_FINISHED_SESSIONS;
    for (id, _) in finished.into_iter().take(excess) {
        guard.remove(&id);
        log::debug!("pty '{id}': история вытеснена");
    }
}

pub struct PtyManager {
    sessions: Registry,
    hub: StreamHub,
}

impl PtyManager {
    pub fn new(hub: StreamHub) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            hub,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        &self,
        id: String,
        shell: Option<String>,
        args: Option<Vec<String>>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        cols: Option<u16>,
        rows: Option<u16>,
    ) -> AppResult<PtySpawned> {
        if id.trim().is_empty() {
            return Err(AppError::invalid_argument("id терминала не может быть пустым"));
        }

        // Повторный spawn с тем же id (перезагрузка плагина) не должен
        // оставлять старый процесс жить в фоне.
        if let Some(old) = self.sessions.lock().remove(&id) {
            log::warn!("pty '{id}': повторный spawn, старый процесс убит");
            old.kill_tree();
        }

        let cols = cols.unwrap_or(80).clamp(1, 1000);
        let rows = rows.unwrap_or(24).clamp(1, 1000);

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::internal(format!("не удалось открыть PTY: {e}")))?;

        let mut cmd = match shell {
            Some(shell) if !shell.trim().is_empty() => {
                let mut c = CommandBuilder::new(shell);
                for a in args.unwrap_or_default() {
                    c.arg(a);
                }
                c
            }
            _ => CommandBuilder::new_default_prog(),
        };

        if let Some(dir) = cwd.filter(|s| !s.trim().is_empty()) {
            let p = Path::new(&dir);
            if !p.is_dir() {
                return Err(AppError::new(
                    ErrorCode::NotADirectory,
                    format!("{dir}: рабочий каталог не найден"),
                ));
            }
            cmd.cwd(p);
        }

        // TERM обязателен: без него TUI-программы (vim, htop, cargo) ломают
        // вывод и не рисуют цвета.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "AROMA_IDE");

        if let Some(env) = env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| AppError::internal(format!("не удалось запустить процесс: {e}")))?;
        // Slave нужно закрыть, иначе EOF на master никогда не наступит.
        drop(pair.slave);

        let pid = child.process_id();
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| AppError::internal(format!("PTY reader: {e}")))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|e| AppError::internal(format!("PTY writer: {e}")))?;

        let (write_tx, write_rx) = sync_channel::<Vec<u8>>(WRITE_QUEUE);
        let scrollback = Arc::new(Scrollback::new(SCROLLBACK_BYTES));

        let session = Arc::new(PtySession {
            id: id.clone(),
            pid,
            master: Mutex::new(Some(pair.master)),
            write_tx,
            child: Mutex::new(Some(child)),
            alive: AtomicBool::new(true),
            cols: AtomicU16::new(cols),
            rows: AtomicU16::new(rows),
            finished_at: Mutex::new(None),
            scrollback: Arc::clone(&scrollback),
        });

        // ВАЖНО: сессия попадает в реестр до старта потоков. Иначе мгновенно
        // завершившийся процесс успевает выйти, поток не находит сессию в
        // реестре, и `pty:exit` теряется.
        self.sessions
            .lock()
            .insert(id.clone(), Arc::clone(&session));

        // Канал «читатель закончил»: наблюдатель ждёт его, чтобы гарантировать
        // порядок «все данные → выход».
        let (reader_done_tx, reader_done_rx) = sync_channel::<()>(1);

        // --- поток записи ---
        let id_writer = id.clone();
        let hub_writer = self.hub.clone();
        std::thread::Builder::new()
            .name(format!("pty-write-{id_writer}"))
            .spawn(move || {
                while let Ok(bytes) = write_rx.recv() {
                    if let Err(e) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                        hub_writer.error(
                            StreamKind::Pty,
                            &id_writer,
                            format!("запись в PTY не удалась: {e}"),
                        );
                        break;
                    }
                }
            })
            .map_err(|e| AppError::internal(format!("поток записи PTY: {e}")))?;

        // --- поток чтения ---
        // Завершается по EOF, ошибке или когда мастер закрыт наблюдателем.
        let id_reader = id.clone();
        let hub_reader = self.hub.clone();
        let scroll_reader = Arc::clone(&scrollback);
        std::thread::Builder::new()
            .name(format!("pty-read-{id_reader}"))
            .spawn(move || {
                let mut buf = vec![0u8; READ_CHUNK];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            scroll_reader.push(&buf[..n]);
                            hub_reader.data(StreamKind::Pty, &id_reader, buf[..n].to_vec());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                // Сигнал наблюдателю: данные вычитаны, можно объявлять выход.
                let _ = reader_done_tx.send(());
            })
            .map_err(|e| AppError::internal(format!("поток чтения PTY: {e}")))?;

        // --- поток-наблюдатель за завершением процесса ---
        //
        // Почему опрос, а не EOF: на Windows ConPTY читатель не получает EOF
        // после выхода дочернего процесса — дескриптор остаётся открытым, и
        // `read` висит. Если ждать EOF, `pty:exit` не приходит никогда.
        let id_wait = id.clone();
        let hub_wait = self.hub.clone();
        let registry = Arc::clone(&self.sessions);
        let session_wait = Arc::clone(&session);
        std::thread::Builder::new()
            .name(format!("pty-wait-{id_wait}"))
            .spawn(move || {
                let code = loop {
                    {
                        let mut guard = session_wait.child.lock();
                        match guard.as_mut() {
                            Some(child) => match child.try_wait() {
                                Ok(Some(status)) => break Some(status.exit_code() as i32),
                                // Процесс ещё жив.
                                Ok(None) => {}
                                // Опросить нельзя (уже собран) — выходим без кода.
                                Err(_) => break None,
                            },
                            // Потомка забрал `kill_tree` — он уже собран там.
                            None => break None,
                        }
                    }
                    std::thread::sleep(POLL_INTERVAL);
                };

                // Закрываем мастер: это будит читателя, застрявшего в `read`.
                session_wait.master.lock().take();
                session_wait.alive.store(false, Ordering::SeqCst);
                *session_wait.finished_at.lock() = Some(std::time::Instant::now());

                // Ждём читателя, чтобы последние байты попали в шину до выхода.
                // Таймаут — страховка от зависшего `read`.
                let _ = reader_done_rx.recv_timeout(std::time::Duration::from_secs(2));

                hub_wait.exit(StreamKind::Pty, &id_wait, code);

                // Сессию НЕ удаляем: её вывод ещё нужен пользователю
                // (`pty_replay`). Вытесняем только самые старые завершённые.
                evict_finished(&registry, &id_wait);
            })
            .map_err(|e| AppError::internal(format!("поток ожидания PTY: {e}")))?;

        log::info!("pty '{id}': запущен, pid={pid:?}, {cols}x{rows}");
        Ok(PtySpawned { id, pid })
    }

    pub fn write(&self, id: &str, data: String) -> AppResult<()> {
        let session = self.session(id)?;
        session
            .write_tx
            .try_send(data.into_bytes())
            .map_err(|e| match e {
                std::sync::mpsc::TrySendError::Full(_) => AppError::new(
                    ErrorCode::Busy,
                    "очередь ввода переполнена: процесс не читает stdin",
                ),
                std::sync::mpsc::TrySendError::Disconnected(_) => {
                    AppError::new(ErrorCode::NotFound, format!("терминал '{id}' уже закрыт"))
                }
            })
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let session = self.session(id)?;
        let cols = cols.clamp(1, 1000);
        let rows = rows.clamp(1, 1000);
        let master = session.master.lock();
        let master = master
            .as_ref()
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, format!("терминал '{id}' уже закрыт")))?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| AppError::internal(format!("resize: {e}")))?;
        session.cols.store(cols, Ordering::Relaxed);
        session.rows.store(rows, Ordering::Relaxed);
        Ok(())
    }

    pub fn kill(&self, id: &str) -> AppResult<()> {
        let session = self.sessions.lock().remove(id);
        if let Some(session) = session {
            session.kill_tree();
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<PtyInfo> {
        self.sessions
            .lock()
            .values()
            .map(|s| PtyInfo {
                id: s.id.clone(),
                pid: s.pid,
                cols: s.cols.load(Ordering::Relaxed),
                rows: s.rows.load(Ordering::Relaxed),
                alive: s.alive.load(Ordering::Relaxed),
            })
            .collect()
    }

    /// Последние байты вывода: при перезагрузке плагина терминала история
    /// не должна исчезать.
    pub fn replay(&self, id: &str) -> AppResult<String> {
        Ok(self.session(id)?.scrollback.snapshot())
    }

    pub fn kill_all(&self) {
        let sessions: Vec<Arc<PtySession>> = self.sessions.lock().drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.kill_tree();
        }
    }

    fn session(&self, id: &str) -> AppResult<Arc<PtySession>> {
        self.sessions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, format!("терминал '{id}' не найден")))
    }
}

// ---------------------------------------------------------------------------
// Команды Tauri
// ---------------------------------------------------------------------------

/// Запустить терминал. `id` задаёт вызывающий: он же потом адресует ввод и
/// resize, поэтому повторный запуск с тем же id перезапускает сессию.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn pty_spawn(
    manager: State<'_, PtyManager>,
    id: String,
    shell: Option<String>,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    cols: Option<u16>,
    rows: Option<u16>,
) -> AppResult<PtySpawned> {
    manager.spawn(id, shell, args, cwd, env, cols, rows)
}

#[tauri::command]
pub fn pty_write(manager: State<'_, PtyManager>, id: String, data: String) -> AppResult<()> {
    manager.write(&id, data)
}

#[tauri::command]
pub fn pty_resize(manager: State<'_, PtyManager>, id: String, cols: u16, rows: u16) -> AppResult<()> {
    manager.resize(&id, cols, rows)
}

#[tauri::command]
pub fn pty_kill(manager: State<'_, PtyManager>, id: String) -> AppResult<()> {
    manager.kill(&id)
}

#[tauri::command]
pub fn pty_list(manager: State<'_, PtyManager>) -> Vec<PtyInfo> {
    manager.list()
}

#[tauri::command]
pub fn pty_replay(manager: State<'_, PtyManager>, id: String) -> AppResult<String> {
    manager.replay(&id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::RecordingSink;
    use std::time::{Duration, Instant};

    /// Проверка допущения: получает ли читатель ConPTY EOF после выхода
    /// дочернего процесса. От этого зависит вся стратегия определения выхода.
    #[test]
    fn probe_conpty_eof_behaviour() {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();

        #[cfg(windows)]
        let cmd = {
            let mut c = CommandBuilder::new("cmd.exe");
            c.args(["/C", "echo probe"]);
            c
        };
        #[cfg(not(windows))]
        let cmd = {
            let mut c = CommandBuilder::new("sh");
            c.args(["-c", "echo probe"]);
            c
        };

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 4096];
            let mut collected = String::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        let _ = tx.send(format!("EOF after {collected:?}"));
                        return;
                    }
                    Ok(n) => collected.push_str(&String::from_utf8_lossy(&buf[..n])),
                    Err(e) => {
                        let _ = tx.send(format!("ERR {e} after {collected:?}"));
                        return;
                    }
                }
            }
        });

        // Ждём выхода процесса и завершения чтения.
        let status = child.wait().unwrap();
        let reader_result = rx.recv_timeout(Duration::from_secs(3));

        println!(
            "exit_code={} reader={:?}",
            status.exit_code(),
            reader_result
        );

        // Тест не утверждает конкретное поведение ОС — он документирует его.
        // Если EOF не пришёл, значит опрос `try_wait` обязателен.
        match reader_result {
            Ok(msg) => assert!(!msg.is_empty()),
            Err(_) => {
                println!("ConPTY не отдал EOF: полагаемся на опрос try_wait");
            }
        }
    }


    /// Ждём, пока в приёмнике появится событие, удовлетворяющее условию.
    fn wait_until<F: Fn(&RecordingSink) -> bool>(
        sink: &RecordingSink,
        timeout: Duration,
        check: F,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if check(sink) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// На Windows команда, печатающая текст, без `cmd` не запустится,
    /// поэтому тесты используют оболочку через `shell`.
    #[cfg(windows)]
    fn echo_command(text: &str) -> (String, Vec<String>) {
        ("cmd.exe".to_string(), vec!["/C".into(), format!("echo {text}")])
    }

    #[cfg(not(windows))]
    fn echo_command(text: &str) -> (String, Vec<String>) {
        ("sh".to_string(), vec!["-c".into(), format!("echo {text}")])
    }

    #[test]
    fn spawns_reads_and_reports_exit() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        let (shell, args) = echo_command("aroma-pty-ok");
        let spawned = manager
            .spawn(
                "t1".into(),
                Some(shell),
                Some(args),
                None,
                None,
                Some(80),
                Some(24),
            )
            .expect("spawn");

        assert_eq!(spawned.id, "t1");
        // Процесс должен появиться в реестре сразу после spawn.
        assert_eq!(manager.list().len(), 1);

        let saw_exit = wait_until(&sink, Duration::from_secs(10), |s| {
            s.names().iter().any(|n| n == "pty:exit")
        });
        assert!(saw_exit, "не дождались pty:exit");

        // Вывод процесса должен быть прочитан, а не потерян.
        let text = sink.text_of("t1");
        assert!(
            text.contains("aroma-pty-ok"),
            "вывод не получен: {text:?}"
        );

        // После выхода сессия помечается завершённой, но её вывод остаётся
        // доступен для `pty_replay` — пользователю нужен последний лог.
        let finished = wait_until(&sink, Duration::from_secs(5), |_| {
            manager
                .list()
                .iter()
                .find(|p| p.id == "t1")
                .map(|p| !p.alive)
                .unwrap_or(true)
        });
        assert!(finished, "сессия не помечена завершённой");
    }

    #[test]
    fn exit_code_is_reported() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        #[cfg(windows)]
        let (shell, args) = (
            "cmd.exe".to_string(),
            vec!["/C".to_string(), "exit 3".to_string()],
        );
        #[cfg(not(windows))]
        let (shell, args) = (
            "sh".to_string(),
            vec!["-c".to_string(), "exit 3".to_string()],
        );

        manager
            .spawn("t2".into(), Some(shell), Some(args), None, None, None, None)
            .unwrap();

        let ok = wait_until(&sink, Duration::from_secs(10), |s| {
            s.events().iter().any(|(name, payload)| {
                name == "pty:exit" && payload.get("code").and_then(|c| c.as_i64()) == Some(3)
            })
        });
        assert!(ok, "код возврата не пришёл: {:?}", sink.events());
    }

    #[test]
    fn write_reaches_the_process() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        // `more` ждёт stdin и печатает введённое — проверка записи в PTY.
        #[cfg(windows)]
        let (shell, args) = (
            "cmd.exe".to_string(),
            vec![
                "/C".to_string(),
                "set /p line=& echo got:%line%".to_string(),
            ],
        );
        #[cfg(not(windows))]
        let (shell, args) = (
            "sh".to_string(),
            vec!["-c".to_string(), "read line; echo got:$line".to_string()],
        );

        manager
            .spawn("t3".into(), Some(shell), Some(args), None, None, None, None)
            .unwrap();

        // Даём процессу запуститься, затем пишем строку.
        std::thread::sleep(Duration::from_millis(400));
        manager.write("t3", "hello\n".into()).unwrap();

        let ok = wait_until(&sink, Duration::from_secs(10), |s| {
            s.text_of("t3").contains("hello")
        });
        assert!(ok, "запись не дошла: {:?}", sink.text_of("t3"));
    }

    #[test]
    fn resize_updates_reported_size() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        // Долгоживущий процесс: нужен, чтобы resize успел примениться.
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe".to_string(), vec!["/C".to_string(), "pause".to_string()]);
        #[cfg(not(windows))]
        let (shell, args) = ("sh".to_string(), vec!["-c".to_string(), "sleep 30".to_string()]);

        manager
            .spawn("t4".into(), Some(shell), Some(args), None, None, Some(80), Some(24))
            .unwrap();

        manager.resize("t4", 120, 40).unwrap();
        let info = manager.list();
        assert_eq!(info[0].cols, 120);
        assert_eq!(info[0].rows, 40);

        manager.kill("t4").unwrap();
        assert!(manager.list().is_empty());
    }

    #[test]
    fn kill_removes_session_and_unknown_id_errors() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        #[cfg(windows)]
        let (shell, args) = ("cmd.exe".to_string(), vec!["/C".to_string(), "pause".to_string()]);
        #[cfg(not(windows))]
        let (shell, args) = ("sh".to_string(), vec!["-c".to_string(), "sleep 30".to_string()]);

        manager
            .spawn("t5".into(), Some(shell), Some(args), None, None, None, None)
            .unwrap();

        assert!(manager.list().iter().any(|p| p.id == "t5"));
        manager.kill("t5").unwrap();
        assert!(!manager.list().iter().any(|p| p.id == "t5"));

        // Повторный kill идемпотентен, а запись в несуществующую сессию — ошибка.
        manager.kill("t5").unwrap();
        let err = manager.write("t5", "x".into()).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }
    #[test]
    fn respawn_with_same_id_replaces_session() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        #[cfg(windows)]
        let (shell, args) = ("cmd.exe".to_string(), vec!["/C".to_string(), "pause".to_string()]);
        #[cfg(not(windows))]
        let (shell, args) = ("sh".to_string(), vec!["-c".to_string(), "sleep 30".to_string()]);

        let first = manager
            .spawn("t6".into(), Some(shell.clone()), Some(args.clone()), None, None, None, None)
            .unwrap();
        let second = manager
            .spawn("t6".into(), Some(shell), Some(args), None, None, None, None)
            .unwrap();

        // Должен остаться ровно один процесс с этим id.
        assert_eq!(manager.list().iter().filter(|p| p.id == "t6").count(), 1);
        assert_ne!(first.pid, second.pid, "старый процесс не был перезапущен");

        manager.kill("t6").unwrap();
    }

    #[test]
    fn rejects_bad_arguments() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        assert_eq!(
            manager.spawn("  ".into(), None, None, None, None, None, None).unwrap_err().code,
            ErrorCode::InvalidArgument
        );

        let err = manager
            .spawn(
                "t7".into(),
                None,
                None,
                Some("C:\\definitely\\not\\here".into()),
                None,
                None,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotADirectory);
    }

    #[test]
    fn finished_sessions_do_not_accumulate() {
        // На Windows ConPTY читатель не получает EOF после выхода процесса.
        // Если бы поток чтения оставался висеть в `read`, каждый терминал
        // оставлял бы за собой мёртвый поток. Проверяем, что завершённые
        // сессии помечаются и вытесняются, а не копятся бесконечно.
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        let (shell, args) = echo_command("leak-check");

        // Больше циклов, чем разрешено держать завершённых сессий.
        let total = MAX_FINISHED_SESSIONS + 6;
        for i in 0..total {
            let id = format!("leak-{i}");
            manager
                .spawn(
                    id.clone(),
                    Some(shell.clone()),
                    Some(args.clone()),
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap();

            let done = wait_until(&sink, Duration::from_secs(15), |_| {
                manager
                    .list()
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| !p.alive)
                    .unwrap_or(false)
            });
            assert!(done, "сессия '{id}' не завершилась");
        }

        // История ограничена: старые завершённые сессии вытеснены.
        let count = manager.list().len();
        assert!(
            count <= MAX_FINISHED_SESSIONS + 2,
            "завершённые сессии копятся: {count}"
        );
    }

    #[test]
    fn replay_returns_scrollback() {        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = PtyManager::new(hub);

        let (shell, args) = echo_command("scrollback-marker");
        manager
            .spawn("t8".into(), Some(shell), Some(args), None, None, None, None)
            .unwrap();

        // Ждём, пока вывод попадёт в scrollback.
        let ok = wait_until(&sink, Duration::from_secs(10), |_| {
            manager
                .replay("t8")
                .map(|s| s.contains("scrollback-marker"))
                .unwrap_or(false)
        });
        assert!(ok, "scrollback пуст");
    }
}
