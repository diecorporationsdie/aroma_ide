//! Последовательные порты: прошивка и обмен с контроллерами.
//!
//! Для «проектирования плат и контроллеров в реальном времени» мало рисовать
//! схему — нужно видеть, что плата отвечает. Поэтому порт открывается через
//! Rust, а данные идут бинарным потоком (base64), без потерь на кодировках:
//! протоколы МК — это байты, а не текст.
//!
//! Чтение — в отдельном потоке с коротким таймаутом: `serialport` не умеет
//! асинхронно, а блокирующее чтение не даёт закрыть порт.

use crate::error::{AppError, AppResult, ErrorCode};
use crate::stream::{StreamHub, StreamKind};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serialport::{DataBits, FlowControl, Parity, SerialPort, StopBits};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::State;

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub name: String,
    pub kind: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SerialOptions {
    pub baud_rate: Option<u32>,
    pub data_bits: Option<u8>,
    pub parity: Option<String>,
    pub stop_bits: Option<u8>,
    pub flow_control: Option<String>,
    /// Таймаут чтения в миллисекундах.
    pub timeout_ms: Option<u64>,
}

pub struct SerialSession {
    pub id: String,
    pub port_name: String,
    port: Mutex<Box<dyn SerialPort>>,
    alive: AtomicBool,
}

impl SerialSession {
    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        // Явный flush: иначе последние байты могут остаться в буфере ОС.
        if let Some(mut port) = self.port.try_lock() {
            let _ = port.flush();
        }
    }
}

type Registry = Arc<Mutex<HashMap<String, Arc<SerialSession>>>>;

pub struct SerialManager {
    sessions: Registry,
    hub: StreamHub,
}

impl SerialManager {
    pub fn new(hub: StreamHub) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            hub,
        }
    }

    pub fn list_ports(&self) -> AppResult<Vec<PortInfo>> {
        let ports = serialport::available_ports()
            .map_err(|e| AppError::internal(format!("не удалось перечислить порты: {e}")))?;

        Ok(ports
            .into_iter()
            .map(|p| match p.port_type {
                serialport::SerialPortType::UsbPort(usb) => PortInfo {
                    name: p.port_name,
                    kind: "usb".into(),
                    manufacturer: usb.manufacturer,
                    product: usb.product,
                    serial_number: usb.serial_number,
                    vid: Some(usb.vid),
                    pid: Some(usb.pid),
                },
                serialport::SerialPortType::PciPort => PortInfo {
                    name: p.port_name,
                    kind: "pci".into(),
                    manufacturer: None,
                    product: None,
                    serial_number: None,
                    vid: None,
                    pid: None,
                },
                serialport::SerialPortType::BluetoothPort => PortInfo {
                    name: p.port_name,
                    kind: "bluetooth".into(),
                    manufacturer: None,
                    product: None,
                    serial_number: None,
                    vid: None,
                    pid: None,
                },
                serialport::SerialPortType::Unknown => PortInfo {
                    name: p.port_name,
                    kind: "unknown".into(),
                    manufacturer: None,
                    product: None,
                    serial_number: None,
                    vid: None,
                    pid: None,
                },
            })
            .collect())
    }

    pub fn open(
        &self,
        id: String,
        port_name: String,
        options: Option<SerialOptions>,
    ) -> AppResult<()> {
        if id.trim().is_empty() {
            return Err(AppError::invalid_argument("id порта не может быть пустым"));
        }
        if port_name.trim().is_empty() {
            return Err(AppError::invalid_argument("не указано имя порта"));
        }

        if let Some(old) = self.sessions.lock().remove(&id) {
            old.close();
        }

        let opts = options.unwrap_or_default();
        let baud = opts.baud_rate.unwrap_or(115_200);

        let mut builder = serialport::new(&port_name, baud)
            .timeout(Duration::from_millis(opts.timeout_ms.unwrap_or(20).clamp(1, 5_000)));

        builder = builder.data_bits(match opts.data_bits.unwrap_or(8) {
            5 => DataBits::Five,
            6 => DataBits::Six,
            7 => DataBits::Seven,
            _ => DataBits::Eight,
        });

        builder = builder.parity(match opts.parity.as_deref() {
            Some("odd") => Parity::Odd,
            Some("even") => Parity::Even,
            _ => Parity::None,
        });

        builder = builder.stop_bits(match opts.stop_bits.unwrap_or(1) {
            2 => StopBits::Two,
            _ => StopBits::One,
        });

        builder = builder.flow_control(match opts.flow_control.as_deref() {
            Some("hardware") => FlowControl::Hardware,
            Some("software") => FlowControl::Software,
            _ => FlowControl::None,
        });

        let port = builder
            .open()
            .map_err(|e| AppError::internal(format!("{port_name}: {e}")))?;

        let mut reader = port
            .try_clone()
            .map_err(|e| AppError::internal(format!("{port_name}: {e}")))?;

        let session = Arc::new(SerialSession {
            id: id.clone(),
            port_name: port_name.clone(),
            port: Mutex::new(port),
            alive: AtomicBool::new(true),
        });

        self.sessions
            .lock()
            .insert(id.clone(), Arc::clone(&session));

        // --- поток чтения ---
        let id_reader = id.clone();
        let hub = self.hub.clone();
        let registry = Arc::clone(&self.sessions);
        let session_reader = Arc::clone(&session);
        std::thread::Builder::new()
            .name(format!("serial-read-{id_reader}"))
            .spawn(move || {
                let mut buf = vec![0u8; 4096];
                loop {
                    if !session_reader.alive.load(Ordering::Relaxed) {
                        break;
                    }
                    match reader.read(&mut buf) {
                        Ok(0) => std::thread::sleep(Duration::from_millis(10)),
                        Ok(n) => hub.binary(StreamKind::Serial, &id_reader, buf[..n].to_vec()),
                        Err(e)
                            if e.kind() == std::io::ErrorKind::TimedOut
                                || e.kind() == std::io::ErrorKind::Interrupted =>
                        {
                            continue
                        }
                        Err(e) => {
                            hub.error(StreamKind::Serial, &id_reader, format!("{e}"));
                            break;
                        }
                    }
                }

                session_reader.alive.store(false, Ordering::SeqCst);
                hub.exit(StreamKind::Serial, &id_reader, None);

                let mut guard = registry.lock();
                let same = guard
                    .get(&id_reader)
                    .map(|s| Arc::ptr_eq(s, &session_reader))
                    .unwrap_or(false);
                if same {
                    guard.remove(&id_reader);
                }
            })
            .map_err(|e| AppError::internal(format!("поток чтения порта: {e}")))?;

        log::info!("serial '{id}': открыт {port_name} @ {baud} бод");
        Ok(())
    }

    pub fn write(&self, id: &str, bytes: &[u8]) -> AppResult<()> {
        let session = self.session(id)?;
        let mut port = session.port.lock();
        port.write_all(bytes)
            .and_then(|_| port.flush())
            .map_err(|e| AppError::internal(format!("запись в порт '{id}': {e}")))
    }

    /// DTR/RTS: нужны для сброса платы в загрузчик (Arduino, ESP, STM32).
    pub fn set_signals(&self, id: &str, dtr: Option<bool>, rts: Option<bool>) -> AppResult<()> {
        let session = self.session(id)?;
        let mut port = session.port.lock();
        if let Some(dtr) = dtr {
            port.write_data_terminal_ready(dtr)
                .map_err(|e| AppError::internal(format!("DTR: {e}")))?;
        }
        if let Some(rts) = rts {
            port.write_request_to_send(rts)
                .map_err(|e| AppError::internal(format!("RTS: {e}")))?;
        }
        Ok(())
    }

    pub fn close(&self, id: &str) -> AppResult<()> {
        if let Some(session) = self.sessions.lock().remove(id) {
            session.close();
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<String> {
        self.sessions
            .lock()
            .values()
            .map(|s| format!("{}:{}", s.id, s.port_name))
            .collect()
    }

    pub fn close_all(&self) {
        let sessions: Vec<Arc<SerialSession>> =
            self.sessions.lock().drain().map(|(_, s)| s).collect();
        for session in sessions {
            session.close();
        }
    }

    fn session(&self, id: &str) -> AppResult<Arc<SerialSession>> {
        self.sessions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, format!("порт '{id}' не открыт")))
    }
}

// ---------------------------------------------------------------------------
// Команды Tauri
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn serial_list_ports(manager: State<'_, SerialManager>) -> AppResult<Vec<PortInfo>> {
    manager.list_ports()
}

#[tauri::command]
pub fn serial_open(
    manager: State<'_, SerialManager>,
    id: String,
    port_name: String,
    options: Option<SerialOptions>,
) -> AppResult<()> {
    manager.open(id, port_name, options)
}

/// Запись байтов в порт. `data` — base64: протоколы плат бинарные, а не текст.
#[tauri::command]
pub fn serial_write(manager: State<'_, SerialManager>, id: String, data: String) -> AppResult<()> {
    let bytes = BASE64
        .decode(data.as_bytes())
        .map_err(|e| AppError::new(ErrorCode::InvalidArgument, format!("base64: {e}")))?;
    manager.write(&id, &bytes)
}

/// DTR/RTS: сброс платы в загрузчик.
#[tauri::command]
pub fn serial_set_signals(
    manager: State<'_, SerialManager>,
    id: String,
    dtr: Option<bool>,
    rts: Option<bool>,
) -> AppResult<()> {
    manager.set_signals(&id, dtr, rts)
}

#[tauri::command]
pub fn serial_close(manager: State<'_, SerialManager>, id: String) -> AppResult<()> {
    manager.close(&id)
}

#[tauri::command]
pub fn serial_list(manager: State<'_, SerialManager>) -> Vec<String> {
    manager.list()
}
