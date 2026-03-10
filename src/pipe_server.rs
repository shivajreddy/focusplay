//! Named pipe server for the tray app.
//! Listens for connections from the native host bridge process.
//! Receives ExtensionMessages, sends HostMessages.

use crate::protocol::{ExtensionMessage, HostMessage, PIPE_NAME};
use anyhow::{Context, Result};
use std::sync::mpsc;
use std::thread;
use tracing::{error, info, warn};
use windows::Win32::Foundation::{CloseHandle, HANDLE};

// Declare the raw FFI functions we need
#[link(name = "kernel32")]
extern "system" {
    fn CreateNamedPipeA(
        lpName: *const u8,
        dwOpenMode: u32,
        dwPipeMode: u32,
        nMaxInstances: u32,
        nOutBufferSize: u32,
        nInBufferSize: u32,
        nDefaultTimeOut: u32,
        lpSecurityAttributes: *const std::ffi::c_void,
    ) -> isize;

    fn ConnectNamedPipe(hNamedPipe: isize, lpOverlapped: *const std::ffi::c_void) -> i32;

    fn DisconnectNamedPipe(hNamedPipe: isize) -> i32;
}

const PIPE_ACCESS_DUPLEX: u32 = 0x00000003;
const PIPE_TYPE_BYTE: u32 = 0x00000000;
const PIPE_READMODE_BYTE: u32 = 0x00000000;
const PIPE_WAIT: u32 = 0x00000000;
const PIPE_UNLIMITED_INSTANCES: u32 = 255;

/// Pipe server that runs in the tray app
pub struct PipeServer {
    /// Receives messages from extension (via bridge)
    receiver: parking_lot::Mutex<mpsc::Receiver<ExtensionMessage>>,
    /// Sends messages to extension (via bridge)
    sender: mpsc::Sender<HostMessage>,
}

impl PipeServer {
    /// Start the pipe server on a background thread
    pub fn start() -> Result<Self> {
        let (ext_tx, ext_rx) = mpsc::channel::<ExtensionMessage>();
        let (host_tx, host_rx) = mpsc::channel::<HostMessage>();

        thread::Builder::new()
            .name("pipe-server".into())
            .spawn(move || loop {
                info!("Pipe server waiting for connection...");

                match Self::accept_and_handle(ext_tx.clone(), &host_rx) {
                    Ok(()) => info!("Pipe client disconnected"),
                    Err(e) => warn!("Pipe session error: {}", e),
                }

                thread::sleep(std::time::Duration::from_millis(100));
            })
            .context("Failed to spawn pipe server thread")?;

        Ok(Self {
            receiver: parking_lot::Mutex::new(ext_rx),
            sender: host_tx,
        })
    }

    fn accept_and_handle(
        ext_tx: mpsc::Sender<ExtensionMessage>,
        host_rx: &mpsc::Receiver<HostMessage>,
    ) -> Result<()> {
        let pipe_name = format!("{}\0", PIPE_NAME);

        // Create named pipe
        let handle = unsafe {
            CreateNamedPipeA(
                pipe_name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                std::ptr::null(),
            )
        };

        if handle == -1 {
            return Err(anyhow::anyhow!(
                "Failed to create named pipe: {}",
                std::io::Error::last_os_error()
            ));
        }

        // Wait for client connection (blocking)
        let result = unsafe { ConnectNamedPipe(handle, std::ptr::null()) };
        if result == 0 {
            let err = std::io::Error::last_os_error();
            // ERROR_PIPE_CONNECTED (535) is OK - client connected before we called ConnectNamedPipe
            if err.raw_os_error() != Some(535) {
                unsafe { CloseHandle(HANDLE(handle as *mut _)).ok() };
                return Err(anyhow::anyhow!("ConnectNamedPipe failed: {}", err));
            }
        }

        info!("Pipe client connected");

        // Create File handles from the raw handle for reading/writing
        let read_handle = handle;
        let write_handle = handle;

        // Reader thread for this connection
        let reader = thread::Builder::new()
            .name("pipe-reader".into())
            .spawn(move || {
                // Use raw reads via Win32
                let mut len_buf = [0u8; 4];
                loop {
                    // Read length prefix
                    if !raw_read(read_handle, &mut len_buf) {
                        break;
                    }

                    let len = u32::from_le_bytes(len_buf) as usize;
                    if len == 0 || len > 1024 * 1024 {
                        break;
                    }

                    let mut buf = vec![0u8; len];
                    if !raw_read(read_handle, &mut buf) {
                        break;
                    }

                    match serde_json::from_slice::<ExtensionMessage>(&buf) {
                        Ok(msg) => {
                            if ext_tx.send(msg).is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("Failed to parse pipe message: {}", e);
                        }
                    }
                }
            })
            .context("Failed to spawn pipe reader")?;

        // Write messages from host_rx to pipe
        while let Ok(msg) = host_rx.recv() {
            let json = match serde_json::to_vec(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize: {}", e);
                    continue;
                }
            };

            let len_bytes = (json.len() as u32).to_le_bytes();
            if !raw_write(write_handle, &len_bytes) {
                break;
            }
            if !raw_write(write_handle, &json) {
                break;
            }
        }

        // Cleanup
        unsafe {
            DisconnectNamedPipe(handle);
            CloseHandle(HANDLE(handle as *mut _)).ok();
        }
        let _ = reader.join();

        Ok(())
    }

    /// Send a message to the extension via the pipe
    pub fn send(&self, msg: HostMessage) -> Result<()> {
        self.sender
            .send(msg)
            .context("Failed to send message to pipe")?;
        Ok(())
    }

    /// Try to receive a message without blocking
    pub fn try_recv(&self) -> Option<ExtensionMessage> {
        self.receiver.lock().try_recv().ok()
    }
}

/// Raw read from a pipe handle
fn raw_read(handle: isize, buf: &mut [u8]) -> bool {
    let mut total = 0;
    while total < buf.len() {
        let mut bytes_read: u32 = 0;
        let ok = unsafe {
            windows::Win32::Storage::FileSystem::ReadFile(
                HANDLE(handle as *mut _),
                Some(&mut buf[total..]),
                Some(&mut bytes_read),
                None,
            )
        };
        if ok.is_err() || bytes_read == 0 {
            return false;
        }
        total += bytes_read as usize;
    }
    true
}

/// Raw write to a pipe handle
fn raw_write(handle: isize, buf: &[u8]) -> bool {
    let mut total = 0;
    while total < buf.len() {
        let mut bytes_written: u32 = 0;
        let ok = unsafe {
            windows::Win32::Storage::FileSystem::WriteFile(
                HANDLE(handle as *mut _),
                Some(&buf[total..]),
                Some(&mut bytes_written),
                None,
            )
        };
        if ok.is_err() || bytes_written == 0 {
            return false;
        }
        total += bytes_written as usize;
    }
    true
}
