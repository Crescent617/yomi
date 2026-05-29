use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::error::GuiError;

pub struct TerminalSession {
    #[allow(dead_code)]
    pub id: String,
    pub pty_pair: portable_pty::PtyPair,
    pub writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    pub reader_handle: tokio::task::JoinHandle<()>,
}

pub struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn spawn(
        &self,
        id: String,
        cwd: &std::path::Path,
        app_handle: AppHandle,
        cols: u16,
        rows: u16,
    ) -> Result<(), GuiError> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(GuiError::unknown)?;

        let mut cmd = CommandBuilder::new_default_prog();
        cmd.cwd(cwd);
        let _child = pair.slave.spawn_command(cmd).map_err(GuiError::unknown)?;

        let mut reader = pair.master.try_clone_reader().map_err(GuiError::unknown)?;
        let writer = pair.master.take_writer().map_err(GuiError::unknown)?;

        let id_clone = id.clone();
        // NOTE: portable_pty readers are synchronous blocking I/O.
        // In production with heavy terminal traffic, consider wrapping
        // the read loop in `tokio::task::spawn_blocking`.
        let read_handle = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = String::from_utf8_lossy(&buf[..n]);
                        let payload = serde_json::json!({
                            "id": id_clone,
                            "data": data.to_string(),
                        });
                        let _ = app_handle.emit("terminal:data", payload);
                    }
                }
            }
        });

        let session = TerminalSession {
            id: id.clone(),
            pty_pair: pair,
            writer: Arc::new(Mutex::new(writer)),
            reader_handle: read_handle,
        };

        let mut sessions = self.sessions.lock().await;
        sessions.insert(id, session);
        Ok(())
    }

    pub async fn write(&self, id: &str, data: &str) -> Result<(), GuiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions.get(id).ok_or(GuiError::unknown("Terminal not found"))?;
        let mut writer = session.writer.lock().await;
        writer
            .write_all(data.as_bytes())
            .map_err(GuiError::unknown)?;
        writer.flush().map_err(GuiError::unknown)?;
        Ok(())
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), GuiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions.get(id).ok_or(GuiError::unknown("Terminal not found"))?;
        session
            .pty_pair
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(GuiError::unknown)?;
        Ok(())
    }

    pub async fn kill(&self, id: &str) -> Result<(), GuiError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.remove(id) {
            session.reader_handle.abort();
        }
        Ok(())
    }
}
