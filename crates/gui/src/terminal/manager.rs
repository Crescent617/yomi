use portable_pty::{Child, CommandBuilder, NativePtySystem, PtySize, PtySystem};
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
    #[allow(dead_code)]
    pub child: Box<dyn Child + Send + Sync>,
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
        let child = pair.slave.spawn_command(cmd).map_err(GuiError::unknown)?;

        let mut reader = pair.master.try_clone_reader().map_err(GuiError::unknown)?;
        let writer = pair.master.take_writer().map_err(GuiError::unknown)?;

        let id_clone = id.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        // Spawn the blocking reader loop on the blocking pool so it does not
        // pin a Tokio worker thread.
        let _read_handle = tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = String::from_utf8_lossy(&buf[..n]).to_string();
                        if tx.send(data).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Async forwarder from the channel to the frontend.
        let forward_handle = tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                let payload = serde_json::json!({
                    "id": id_clone,
                    "data": data,
                });
                let _ = app_handle.emit("terminal:data", payload);
            }
        });

        // read_handle is spawn_blocking; when it ends, tx drops, rx returns None,
        // and forward_handle exits naturally — no need for an explicit abort task.

        let session = TerminalSession {
            id: id.clone(),
            pty_pair: pair,
            writer: Arc::new(Mutex::new(writer)),
            child,
            reader_handle: forward_handle,
        };

        let mut sessions = self.sessions.lock().await;
        sessions.insert(id, session);
        Ok(())
    }

    pub async fn write(&self, id: &str, data: &str) -> Result<(), GuiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or(GuiError::unknown("Terminal not found"))?;
        let mut writer = session.writer.lock().await;
        writer
            .write_all(data.as_bytes())
            .map_err(GuiError::unknown)?;
        writer.flush().map_err(GuiError::unknown)?;
        Ok(())
    }

    pub async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), GuiError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .get(id)
            .ok_or(GuiError::unknown("Terminal not found"))?;
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
        if let Some(mut session) = sessions.remove(id) {
            // Kill the shell child first so it does not outlive the panel.
            let _ = session.child.kill();
            // Abort the async forwarder task.
            session.reader_handle.abort();
        }
        Ok(())
    }
}
