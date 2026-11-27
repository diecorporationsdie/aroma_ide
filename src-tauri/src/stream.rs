//! Общая шина потоковых событий: PTY, процессы, serial.
//!
//! Зачем отдельный поток-агрегатор:
//!
//! 1. **Скорость.** `cargo build` или поток с платы выдают тысячи мелких
//!    чанков. Отправлять каждый отдельным IPC-событием — тысячи сериализаций
//!    JSON и переходов через мост. Агрегатор копит данные 8 мс и отправляет
//!    одним пакетом.
//! 2. **Порядок.** Событие выхода обязано прийти после последних данных, иначе
//!    терминал «зависает» с недописанной строкой. Все сообщения идут через
//!    одну очередь, поэтому порядок гарантирован.
//! 3. **Кодировка.** Многобайтовый символ может разорваться между чанками.
//!    Декодер живёт в агрегаторе и помнит хвост для каждого потока.
//!
//! События: `<kind>:data`, `<kind>:exit`, `<kind>:error`.
//! У `serial` поле `data` — base64 (это байты, а не текст).
//!
//! Шина не зависит от Tauri напрямую: приёмник событий задаётся трейтом
//! [`Sink`]. Это позволяет проверять порядок и склейку в тестах без окна.

use crate::utf8::Utf8Stream;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Сколько копим перед отправкой. 8 мс — ниже порога восприятия, но достаточно,
/// чтобы склеить сотни мелких чанков в одно событие.
const FLUSH_INTERVAL: Duration = Duration::from_millis(8);
/// Верхняя граница одного события: не забиваем мост одним пакетом.
const MAX_EVENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamKind {
    Pty,
    Proc,
    Serial,
}

impl StreamKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            StreamKind::Pty => "pty",
            StreamKind::Proc => "proc",
            StreamKind::Serial => "serial",
        }
    }
}

/// Приёмник событий. Прод: Tauri-события; тесты: вектор записей.
pub trait Sink: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: serde_json::Value);
}

impl Sink for AppHandle {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        // Ошибка доставки не должна ронять поток агрегатора: окно могло
        // закрыться раньше, чем завершился дочерний процесс.
        let _ = Emitter::emit(self, event, payload);
    }
}

/// Приёмник, складывающий события в память. Используется в тестах.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<(String, serde_json::Value)> {
        self.events.lock().clone()
    }

    /// Склеенный текст всех `data` для конкретного потока.
    pub fn text_of(&self, id: &str) -> String {
        self.events
            .lock()
            .iter()
            .filter(|(name, payload)| {
                name.ends_with(":data")
                    && payload.get("id").and_then(|v| v.as_str()) == Some(id)
                    && payload.get("binary").is_none()
            })
            .filter_map(|(_, payload)| payload.get("data")?.as_str().map(String::from))
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.events.lock().iter().map(|(n, _)| n.clone()).collect()
    }
}

impl Sink for RecordingSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        self.events.lock().push((event.to_string(), payload));
    }
}

impl<T: Sink + ?Sized> Sink for Arc<T> {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        (**self).emit(event, payload);
    }
}

enum Msg {
    Data(StreamKind, String, Vec<u8>),
    Binary(StreamKind, String, Vec<u8>),
    Exit(StreamKind, String, Option<i32>),
    Error(StreamKind, String, String),
    Flush,
}

#[derive(Default)]
struct Pending {
    text: String,
    binary: Vec<u8>,
    exit: Option<Option<i32>>,
    error: Option<String>,
}

#[derive(Clone)]
pub struct StreamHub {
    tx: Sender<Msg>,
}

impl StreamHub {
    /// Запускает поток-агрегатор для Tauri.
    pub fn new(app: AppHandle) -> Self {
        Self::with_sink(app)
    }

    /// Запускает агрегатор с произвольным приёмником (тесты, альтернативный UI).
    pub fn with_sink(sink: impl Sink) -> Self {
        let (tx, rx) = channel::<Msg>();
        std::thread::Builder::new()
            .name("aroma-stream-hub".into())
            .spawn(move || run(sink, rx))
            .expect("не удалось запустить поток потоковых событий");
        Self { tx }
    }

    /// Текстовый поток: байты декодируются как UTF-8 с сохранением хвоста.
    pub fn data(&self, kind: StreamKind, id: &str, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.tx.send(Msg::Data(kind, id.to_string(), bytes));
    }

    /// Бинарный поток: отдаётся в base64.
    pub fn binary(&self, kind: StreamKind, id: &str, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let _ = self.tx.send(Msg::Binary(kind, id.to_string(), bytes));
    }

    pub fn exit(&self, kind: StreamKind, id: &str, code: Option<i32>) {
        let _ = self.tx.send(Msg::Exit(kind, id.to_string(), code));
    }

    pub fn error(&self, kind: StreamKind, id: &str, message: impl Into<String>) {
        let _ = self.tx.send(Msg::Error(kind, id.to_string(), message.into()));
    }

    /// Немедленно сбросить накопленное (нужно тестам и «показать сейчас»).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn flush_now(&self) {
        let _ = self.tx.send(Msg::Flush);
    }
}

fn run(sink: impl Sink, rx: Receiver<Msg>) {
    let mut pending: HashMap<(StreamKind, String), Pending> = HashMap::new();
    let mut decoders: HashMap<(StreamKind, String), Decoder> = HashMap::new();
    let mut last_flush = Instant::now();

    loop {
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(Msg::Data(kind, id, bytes)) => {
                let key = (kind, id);
                let text = decoders.entry(key.clone()).or_default().utf8.push(&bytes);
                pending.entry(key).or_default().text.push_str(&text);
            }
            Ok(Msg::Binary(kind, id, bytes)) => {
                pending
                    .entry((kind, id))
                    .or_default()
                    .binary
                    .extend_from_slice(&bytes);
            }
            Ok(Msg::Exit(kind, id, code)) => {
                // Сбрасываем хвост декодера, чтобы последний символ не потерялся.
                let key = (kind, id);
                if let Some(mut decoder) = decoders.remove(&key) {
                    let tail = decoder.utf8.finish();
                    if !tail.is_empty() {
                        pending.entry(key.clone()).or_default().text.push_str(&tail);
                    }
                }
                pending.entry(key).or_default().exit = Some(code);
            }
            Ok(Msg::Error(kind, id, message)) => {
                pending.entry((kind, id)).or_default().error = Some(message);
            }
            Ok(Msg::Flush) => {
                flush(&sink, &mut pending);
                last_flush = Instant::now();
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Событие выхода/ошибки сбрасываем сразу: задержка здесь выглядит как
        // «терминал завис после завершения команды».
        let urgent = pending
            .values()
            .any(|p| p.exit.is_some() || p.error.is_some());
        if urgent || last_flush.elapsed() >= FLUSH_INTERVAL {
            flush(&sink, &mut pending);
            last_flush = Instant::now();
        }
    }

    // Приёмник отключился — отдаём остаток, чтобы не потерять хвост вывода.
    flush(&sink, &mut pending);
}

#[derive(Default)]
struct Decoder {
    utf8: Utf8Stream,
}

fn flush(sink: &impl Sink, pending: &mut HashMap<(StreamKind, String), Pending>) {
    if pending.is_empty() {
        return;
    }
    let keys: Vec<(StreamKind, String)> = pending.keys().cloned().collect();
    for key in keys {
        let Some(entry) = pending.get_mut(&key) else {
            continue;
        };
        flush_entry(sink, key.0, &key.1, entry);
        let done = entry.exit.is_some()
            && entry.text.is_empty()
            && entry.binary.is_empty()
            && entry.error.is_none();
        if done {
            pending.remove(&key);
        }
    }
}

fn flush_entry(sink: &impl Sink, kind: StreamKind, id: &str, entry: &mut Pending) {
    if !entry.text.is_empty() {
        let text = std::mem::take(&mut entry.text);
        for chunk in split_at_char_boundary(&text, MAX_EVENT_BYTES) {
            sink.emit(
                &format!("{}:data", kind.as_str()),
                serde_json::json!({ "id": id, "data": chunk }),
            );
        }
    }

    if !entry.binary.is_empty() {
        let bytes = std::mem::take(&mut entry.binary);
        for chunk in bytes.chunks(MAX_EVENT_BYTES / 2) {
            sink.emit(
                &format!("{}:data", kind.as_str()),
                serde_json::json!({
                    "id": id,
                    "data": BASE64.encode(chunk),
                    "binary": true,
                }),
            );
        }
    }

    if let Some(message) = entry.error.take() {
        sink.emit(
            &format!("{}:error", kind.as_str()),
            serde_json::json!({ "id": id, "message": message }),
        );
    }

    if let Some(code) = entry.exit.take() {
        sink.emit(
            &format!("{}:exit", kind.as_str()),
            serde_json::json!({ "id": id, "code": code }),
        );
    }
}

/// Режет текст на куски, не разрывая UTF-8.
fn split_at_char_boundary(text: &str, max: usize) -> Vec<&str> {
    if text.len() <= max {
        return vec![text];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        out.push(&text[start..end]);
        start = end;
    }
    out
}

/// Кольцевой буфер последних N байт — для переподключения терминала
/// (перезагрузка плагина, переключение вкладки).
pub struct Scrollback {
    limit: usize,
    data: Mutex<Vec<u8>>,
}

impl Scrollback {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            data: Mutex::new(Vec::with_capacity(limit.min(64 * 1024))),
        }
    }

    pub fn push(&self, bytes: &[u8]) {
        let mut guard = self.data.lock();
        guard.extend_from_slice(bytes);
        if guard.len() > self.limit {
            let excess = guard.len() - self.limit;
            guard.drain(..excess);
        }
    }

    pub fn snapshot(&self) -> String {
        String::from_utf8_lossy(&self.data.lock()).into_owned()
    }

    /// Очистка истории — например, при перезапуске терминала в той же вкладке.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn clear(&self) {
        self.data.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn wait_for(sink: &Arc<RecordingSink>, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if sink.events().len() >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn coalesces_chunks_into_one_event() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));

        for part in ["he", "ll", "o"] {
            hub.data(StreamKind::Pty, "t1", part.as_bytes().to_vec());
        }
        hub.flush_now();

        assert!(wait_for(&sink, 1, Duration::from_secs(2)));
        // Три чанка должны прийти одним событием, а не тремя.
        assert_eq!(sink.text_of("t1"), "hello");
        assert_eq!(sink.events().len(), 1, "{:?}", sink.names());
    }

    #[test]
    fn exit_comes_after_data() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));

        hub.data(StreamKind::Pty, "t2", b"bye".to_vec());
        hub.exit(StreamKind::Pty, "t2", Some(0));

        assert!(wait_for(&sink, 2, Duration::from_secs(2)));
        let names = sink.names();
        let data_idx = names.iter().position(|n| n == "pty:data").unwrap();
        let exit_idx = names.iter().position(|n| n == "pty:exit").unwrap();
        assert!(data_idx < exit_idx, "порядок нарушен: {names:?}");
        assert_eq!(sink.text_of("t2"), "bye");
    }

    #[test]
    fn never_splits_multibyte_characters() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));

        // Каждый байт отдельно: худший случай для «крякозябр».
        for b in "привет".as_bytes() {
            hub.data(StreamKind::Pty, "t3", vec![*b]);
        }
        hub.flush_now();

        assert!(wait_for(&sink, 1, Duration::from_secs(2)));
        assert_eq!(sink.text_of("t3"), "привет");
        assert!(!sink.text_of("t3").contains('\u{FFFD}'));
    }

    #[test]
    fn flushes_dangling_partial_sequence_on_exit() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));

        // Первый байт «й» без второго: символ не завершён.
        hub.data(StreamKind::Pty, "t4", vec![0xD0]);
        hub.exit(StreamKind::Pty, "t4", Some(1));

        assert!(wait_for(&sink, 2, Duration::from_secs(2)));
        // Хвост должен быть отдан (как U+FFFD), а не молча потерян.
        assert!(!sink.text_of("t4").is_empty());
    }

    #[test]
    fn binary_stream_is_base64_and_marked() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));

        hub.binary(StreamKind::Serial, "s1", vec![0x00, 0xFF, 0x10]);
        hub.flush_now();

        assert!(wait_for(&sink, 1, Duration::from_secs(2)));
        let events = sink.events();
        let (name, payload) = &events[0];
        assert_eq!(name, "serial:data");
        assert_eq!(payload["binary"], true);
        let decoded = BASE64.decode(payload["data"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, vec![0x00, 0xFF, 0x10]);
    }

    #[test]
    fn streams_are_isolated_by_id() {
        let sink = Arc::new(RecordingSink::new());
        let hub = StreamHub::with_sink(Arc::clone(&sink));

        hub.data(StreamKind::Pty, "a", b"one".to_vec());
        hub.data(StreamKind::Pty, "b", b"two".to_vec());
        hub.flush_now();

        assert!(wait_for(&sink, 2, Duration::from_secs(2)));
        assert_eq!(sink.text_of("a"), "one");
        assert_eq!(sink.text_of("b"), "two");
    }

    #[test]
    fn large_text_is_split_on_char_boundaries() {
        let text = "я".repeat(MAX_EVENT_BYTES); // 2 байта на символ
        let parts = split_at_char_boundary(&text, MAX_EVENT_BYTES);
        assert!(parts.len() > 1);
        // Ни один кусок не должен разрывать символ.
        for part in &parts {
            assert!(std::str::from_utf8(part.as_bytes()).is_ok());
        }
        assert_eq!(parts.concat(), text);
    }

    #[test]
    fn scrollback_keeps_only_tail() {
        let sb = Scrollback::new(8);
        sb.push(b"0123456789");
        assert_eq!(sb.snapshot(), "23456789");
        sb.push(b"ab");
        assert_eq!(sb.snapshot(), "456789ab");
        sb.clear();
        assert_eq!(sb.snapshot(), "");
    }
}
