use std::fs;
use std::io::ErrorKind;
use std::io::Read;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::adapters::HookMetadata;
use crate::app::logging::{append_app_log, RuntimeLogger};

pub struct SocketServer {
    socket_path: PathBuf,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SocketServer {
    pub fn start(socket_path: PathBuf, sender: Sender<HookMetadata>) -> anyhow::Result<Self> {
        Self::start_inner(socket_path, sender, None)
    }

    pub fn start_with_logger(
        socket_path: PathBuf,
        sender: Sender<HookMetadata>,
        logger: RuntimeLogger,
    ) -> anyhow::Result<Self> {
        Self::start_inner(socket_path, sender, Some(logger))
    }

    fn start_inner(
        socket_path: PathBuf,
        sender: Sender<HookMetadata>,
        logger: Option<RuntimeLogger>,
    ) -> anyhow::Result<Self> {
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }
        let listener = UnixListener::bind(&socket_path)?;
        listener.set_nonblocking(true)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = shutdown.clone();
        let handle = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::Relaxed) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    Err(_) => continue,
                };
                let mut body = String::new();
                if stream.read_to_string(&mut body).is_ok() {
                    if let Ok(metadata) = serde_json::from_str::<HookMetadata>(&body) {
                        if let Some(logger) = &logger {
                            let _ = append_app_log(
                                logger,
                                "info",
                                "hook_received",
                                serde_json::json!({
                                    "source": metadata.source.as_deref(),
                                    "hook_event_name": metadata.hook_event_name.as_deref(),
                                    "session_id": metadata.session_id.as_deref(),
                                    "transcript_path_present": metadata.transcript_path.is_some()
                                }),
                            );
                        }
                        let _ = sender.send(metadata);
                    }
                }
            }
        });
        Ok(Self {
            socket_path,
            shutdown,
            handle: Some(handle),
        })
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = fs::remove_file(&self.socket_path);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub fn forward_hook_metadata(socket_path: &Path, metadata: &HookMetadata) -> anyhow::Result<()> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket_path)?;
    let payload = serde_json::to_vec(metadata)?;
    use std::io::Write;
    stream.write_all(&payload)?;
    Ok(())
}
