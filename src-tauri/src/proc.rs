//! Запуск внешних процессов без PTY: сборка, линтеры, прошивка, CLI тулчейны.
//!
//! Зачем отдельно от терминала: у сборки нет интерактивности, но есть два
//! потока вывода, код возврата и требование не открывать консольное окно.
//! Гонять `cargo build` через PTY — значит получить ANSI-мусор и ложные
//! «терминальные» последовательности в панели вывода.
//!
//! Вывод обоих потоков сводится в один `proc:data`: для сборки важен общий
//! хронологический порядок, а не разделение по каналам.

use crate::error::{AppError, AppResult, ErrorCode};
use crate::stream::{Scrollback, StreamHub, StreamKind};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::State;

const SCROLLBACK_BYTES: usize = 256 * 1024;
/// Сколько завершённых процессов держим ради истории вывода.
///
/// Процесс завершился, но его вывод всё ещё нужен: последние строки
/// `cargo build`, ошибка компилятора, финальный лог. Поэтому сессия не
/// удаляется сразу — она помечается `alive = false` и вытесняется позже.
const MAX_FINISHED: usize = 16;

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProcInfo {
    pub id: String,
    pub pid: u32,
    pub program: String,
    pub alive: bool,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProcSpawned {
    pub id: String,
    pub pid: u32,
}

pub struct ProcSession {
    pub id: String,
    pub pid: u32,
    pub program: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    alive: AtomicBool,
    finished_at: Mutex<Option<std::time::Instant>>,
    pub scrollback: Arc<Scrollback>,
}

impl ProcSession {
    fn kill_tree(&self) {
        if !self.alive.swap(false, Ordering::SeqCst) {
            return;
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let _ = Command::new("taskkill")
                .args(["/PID", &self.pid.to_string(), "/T", "/F"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
        }

        let mut guard = self.child.lock();
        if let Some(child) = guard.as_mut() {
            let _ = child.kill();
        }
        // Reap в отдельном потоке: kill не должен ждать процесс, который
        // игнорирует сигнал.
        if let Some(mut child) = guard.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

type Registry = Arc<Mutex<HashMap<String, Arc<ProcSession>>>>;

/// Вытесняет самые старые завершённые процессы, кроме `keep`.
fn evict_finished(registry: &Registry, keep: &str) {
    let mut guard = registry.lock();

    let mut finished: Vec<(String, std::time::Instant)> = guard
        .iter()
        .filter(|(id, session)| id.as_str() != keep && !session.alive.load(Ordering::SeqCst))
        .filter_map(|(id, session)| session.finished_at.lock().map(|at| (id.clone(), at)))
        .collect();

    if finished.len() <= MAX_FINISHED {
        return;
    }

    finished.sort_by_key(|(_, at)| *at);
    let excess = finished.len() - MAX_FINISHED;
    for (id, _) in finished.into_iter().take(excess) {
        guard.remove(&id);
        log::debug!("proc '{id}': история вытеснена");
    }
}

pub struct ProcManager {
    sessions: Registry,
    hub: StreamHub,
}

impl ProcManager {
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
        program: String,
        args: Option<Vec<String>>,
        cwd: Option<String>,
        env: Option<HashMap<String, String>>,
        shell: Option<bool>,
        stdin: Option<bool>,
    ) -> AppResult<ProcSpawned> {
        if id.trim().is_empty() {
            return Err(AppError::invalid_argument("id процесса не может быть пустым"));
        }
        if program.trim().is_empty() {
            return Err(AppError::invalid_argument("не указана программа"));
        }

        if let Some(old) = self.sessions.lock().remove(&id) {
            log::warn!("proc '{id}': повторный запуск, старый процесс убит");
            old.kill_tree();
        }

        let args = args.unwrap_or_default();
        let mut cmd = if shell.unwrap_or(false) {
            shell_command(&program, &args)
        } else {
            let mut c = Command::new(&program);
            c.args(&args);
            c
        };

        if let Some(dir) = cwd.filter(|s| !s.trim().is_empty()) {
            let p = Path::new(&dir);
            if !p.is_dir() {
                return Err(AppError::new(
                    ErrorCode::NotADirectory,
                    format!("{dir}: рабочий каталог не найден"),
                ));
            }
            cmd.current_dir(p);
        }

        if let Some(env) = env {
            for (k, v) in env {
                cmd.env(k, v);
            }
        }

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.stdin(if stdin.unwrap_or(false) {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| AppError::internal(format!("не удалось запустить '{program}': {e}")))?;

        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdin_pipe = child.stdin.take();
        let scrollback = Arc::new(Scrollback::new(SCROLLBACK_BYTES));

        let session = Arc::new(ProcSession {
            id: id.clone(),
            pid,
            program: program.clone(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(stdin_pipe),
            alive: AtomicBool::new(true),
            finished_at: Mutex::new(None),
            scrollback: Arc::clone(&scrollback),
        });

        self.sessions.lock().insert(id.clone(), Arc::clone(&session));

        if let Some(pipe) = stdout {
            spawn_pipe_reader(pipe, id.clone(), Arc::clone(&scrollback), self.hub.clone());
        }
        if let Some(pipe) = stderr {
            spawn_pipe_reader(pipe, id.clone(), Arc::clone(&scrollback), self.hub.clone());
        }

        // --- ожидание выхода ---
        let id_wait = id.clone();
        let hub_wait = self.hub.clone();
        let registry = Arc::clone(&self.sessions);
        let session_wait = Arc::clone(&session);
        std::thread::Builder::new()
            .name(format!("proc-wait-{id_wait}"))
            .spawn(move || {
                let child = session_wait.child.lock().take();
                let code = match child {
                    Some(mut child) => match child.wait() {
                        Ok(status) => Some(status.code().unwrap_or(-1)),
                        Err(_) => None,
                    },
                    None => None,
                };

                session_wait.alive.store(false, Ordering::SeqCst);
                *session_wait.finished_at.lock() = Some(std::time::Instant::now());
                hub_wait.exit(StreamKind::Proc, &id_wait, code);

                // Сессию НЕ удаляем: её вывод ещё нужен (`proc_replay`).
                // Вытесняем только самые старые завершённые.
                evict_finished(&registry, &id_wait);
            })
            .map_err(|e| AppError::internal(format!("поток ожидания процесса: {e}")))?;

        log::info!("proc '{id}': запущен '{program}', pid={pid}");
        Ok(ProcSpawned { id, pid })
    }

    pub fn write(&self, id: &str, data: String) -> AppResult<()> {
        let session = self.session(id)?;
        let mut guard = session.stdin.lock();
        let stdin = guard.as_mut().ok_or_else(|| {
            AppError::new(
                ErrorCode::Unsupported,
                format!("процесс '{id}' запущен без stdin"),
            )
        })?;
        stdin
            .write_all(data.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| AppError::internal(format!("stdin процесса '{id}': {e}")))
    }

    pub fn kill(&self, id: &str) -> AppResult<()> {
        if let Some(session) = self.sessions.lock().remove(id) {
            session.kill_tree();
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<ProcInfo> {
        self.sessions
            .lock()
            .values()
            .map(|s| ProcInfo {
                id: s.id.clone(),
                pid: s.pid,
                program: s.program.clone(),
                alive: s.alive.load(Ordering::Relaxed),
            })
            .collect()
    }

    pub fn replay(&self, id: &str) -> AppResult<String> {
        Ok(self.session(id)?.scrollback.snapshot())
    }

    pub fn kill_all(&self) {
        let sessions: Vec<Arc<ProcSession>> =
            self.sessions.lock().drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.kill_tree();
        }
    }

    fn session(&self, id: &str) -> AppResult<Arc<ProcSession>> {
        self.sessions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, format!("процесс '{id}' не найден")))
    }
}

fn spawn_pipe_reader(
    mut pipe: impl Read + Send + 'static,
    id: String,
    scrollback: Arc<Scrollback>,
    hub: StreamHub,
) {
    std::thread::Builder::new()
        .name(format!("proc-out-{id}"))
        .spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        scrollback.push(&buf[..n]);
                        hub.data(StreamKind::Proc, &id, buf[..n].to_vec());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .ok();
}

#[cfg(windows)]
fn shell_command(program: &str, args: &[String]) -> Command {
    // Одна строка — как в cmd.exe: проще для команд вида `npm run build`.
    let mut line = String::from(program);
    for a in args {
        line.push(' ');
        line.push_str(a);
    }
    let mut c = Command::new("cmd");
    c.args(["/C", &line]);
    c
}

#[cfg(not(windows))]
fn shell_command(program: &str, args: &[String]) -> Command {
    let mut line = String::from(program);
    for a in args {
        line.push(' ');
        line.push_str(a);
    }
    let mut c = Command::new("sh");
    c.args(["-c", &line]);
    c
}

/// Проверка, что программа вообще есть в PATH. Нужна, чтобы ошибка
/// «не найден компилятор» была внятной, а не `os error 2`.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.is_absolute() {
        return if path.exists() { Some(path.to_path_buf()) } else { None };
    }

    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    } else {
        vec![String::new()]
    };

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{program}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Команды Tauri
// ---------------------------------------------------------------------------

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn proc_spawn(
    manager: State<'_, ProcManager>,
    id: String,
    program: String,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    env: Option<HashMap<String, String>>,
    shell: Option<bool>,
    stdin: Option<bool>,
) -> AppResult<ProcSpawned> {
    manager.spawn(id, program, args, cwd, env, shell, stdin)
}

#[tauri::command]
pub fn proc_write(manager: State<'_, ProcManager>, id: String, data: String) -> AppResult<()> {
    manager.write(&id, data)
}

#[tauri::command]
pub fn proc_kill(manager: State<'_, ProcManager>, id: String) -> AppResult<()> {
    manager.kill(&id)
}

#[tauri::command]
pub fn proc_list(manager: State<'_, ProcManager>) -> Vec<ProcInfo> {
    manager.list()
}

#[tauri::command]
pub fn proc_replay(manager: State<'_, ProcManager>, id: String) -> AppResult<String> {
    manager.replay(&id)
}

#[tauri::command]
pub fn proc_which(program: String) -> Option<String> {
    which(&program).map(|p| p.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::RecordingSink;
    use std::time::{Duration, Instant};

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

    /// Команда, печатающая текст через оболочку: переносимо между ОС.
    #[cfg(windows)]
    fn echo(text: &str) -> String {
        format!("echo {text}")
    }
    #[cfg(not(windows))]
    fn echo(text: &str) -> String {
        format!("echo {text}")
    }

    #[test]
    fn captures_output_and_exit_code() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        let spawned = manager
            .spawn(
                "p1".into(),
                echo("proc-marker"),
                None,
                None,
                None,
                Some(true),
                None,
            )
            .expect("spawn");

        assert_eq!(spawned.id, "p1");
        assert!(spawned.pid > 0);

        let saw_exit = wait_until(&sink, Duration::from_secs(20), |s| {
            s.names().iter().any(|n| n == "proc:exit")
        });
        assert!(saw_exit, "не дождались proc:exit");

        // Вывод сборки должен быть прочитан целиком.
        let text = sink.text_of("p1");
        assert!(text.contains("proc-marker"), "вывод не получен: {text:?}");

        // Код 0 у успешной команды.
        let zero = sink.events().iter().any(|(name, payload)| {
            name == "proc:exit" && payload.get("code").and_then(|c| c.as_i64()) == Some(0)
        });
        assert!(zero, "код возврата не 0: {:?}", sink.events());

        // После завершения процесс остаётся в реестре с `alive = false`:
        // его вывод нужен для `proc_replay`.
        let finished = wait_until(&sink, Duration::from_secs(5), |_| {
            manager
                .list()
                .iter()
                .find(|p| p.id == "p1")
                .map(|p| !p.alive)
                .unwrap_or(true)
        });
        assert!(finished, "процесс не помечен завершённым");
    }

    #[test]
    fn reports_nonzero_exit_code() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        #[cfg(windows)]
        let program = "exit 7".to_string();
        #[cfg(not(windows))]
        let program = "exit 7".to_string();

        manager
            .spawn("p2".into(), program, None, None, None, Some(true), None)
            .unwrap();

        let seven = wait_until(&sink, Duration::from_secs(20), |s| {
            s.events().iter().any(|(name, payload)| {
                name == "proc:exit" && payload.get("code").and_then(|c| c.as_i64()) == Some(7)
            })
        });
        assert!(seven, "код возврата не пришёл: {:?}", sink.events());
    }

    #[test]
    fn captures_stderr() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        #[cfg(windows)]
        let program = "echo err-marker 1>&2".to_string();
        #[cfg(not(windows))]
        let program = "echo err-marker 1>&2".to_string();

        manager
            .spawn("p3".into(), program, None, None, None, Some(true), None)
            .unwrap();

        // stderr сводится в тот же поток: для сборки важен общий порядок.
        let ok = wait_until(&sink, Duration::from_secs(20), |s| {
            s.text_of("p3").contains("err-marker")
        });
        assert!(ok, "stderr не получен: {:?}", sink.text_of("p3"));
    }

    #[test]
    fn kill_terminates_and_is_idempotent() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        #[cfg(windows)]
        let program = "ping -n 30 127.0.0.1 > nul".to_string();
        #[cfg(not(windows))]
        let program = "sleep 30".to_string();

        manager
            .spawn("p4".into(), program, None, None, None, Some(true), None)
            .unwrap();

        assert!(manager.list().iter().any(|p| p.id == "p4"));
        manager.kill("p4").unwrap();
        assert!(!manager.list().iter().any(|p| p.id == "p4"));
        manager.kill("p4").unwrap();
    }
    #[test]
    fn rejects_bad_arguments() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        assert_eq!(
            manager
                .spawn(" ".into(), "echo".into(), None, None, None, None, None)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );

        assert_eq!(
            manager
                .spawn("p5".into(), "  ".into(), None, None, None, None, None)
                .unwrap_err()
                .code,
            ErrorCode::InvalidArgument
        );

        let err = manager
            .spawn(
                "p6".into(),
                "echo".into(),
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
    fn missing_program_reports_internal_error() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        let err = manager
            .spawn(
                "p7".into(),
                "definitely-not-a-real-program-xyz".into(),
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Internal);
        assert!(err.message.contains("definitely-not-a-real-program-xyz"));
    }

    #[test]
    fn which_finds_known_programs() {
        // В Windows есть `cmd.exe`, на POSIX — `sh`.
        #[cfg(windows)]
        let probe = "cmd";
        #[cfg(not(windows))]
        let probe = "sh";

        assert!(which(probe).is_some(), "{probe} должен быть в PATH");
        assert!(which("definitely-not-a-real-program-xyz").is_none());
    }

    #[test]
    fn replay_returns_scrollback() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));
        let manager = ProcManager::new(hub);

        manager
            .spawn(
                "p8".into(),
                echo("replay-marker"),
                None,
                None,
                None,
                Some(true),
                None,
            )
            .unwrap();

        let ok = wait_until(&sink, Duration::from_secs(20), |_| {
            manager
                .replay("p8")
                .map(|s| s.contains("replay-marker"))
                .unwrap_or(false)
        });
        assert!(ok, "scrollback пуст");
    }
}
