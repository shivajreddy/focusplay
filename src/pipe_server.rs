//! Named pipe server for the tray app.
//! Listens for connections from the native host bridge process.
//! Receives ExtensionMessages, sends HostMessages.

use crate::protocol::{self, ExtensionMessage, HostMessage, PIPE_NAME};
use anyhow::{Context, Result};
use std::io::BufReader;
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;
use tracing::{error, info, warn};
use windows::core::PCSTR;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeA, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

// PIPE_ACCESS_DUPLEX is not directly exported in some windows-rs versions
const PIPE_ACCESS_DUPLEX: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES =
    windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(0x00000003);

/// Pipe server that runs in the tray app
pub struct PipeServer {
    /// Receives messages from extension (via bridge)
    receiver: parking_lot::Mutex<mpsc::Receiver<ExtensionMessage>>,
    /// Sends messages to extension (via bridge) - shared with server thread
    /// which swaps the inner sender when a new connection is accepted
    sender: Arc<parking_lot::Mutex<mpsc::Sender<HostMessage>>>,
}

impl PipeServer {
    /// Start the pipe server on a background thread
    pub fn start() -> Result<Self> {
        let (ext_tx, ext_rx) = mpsc::channel::<ExtensionMessage>();

        // We use a sender wrapped in a Mutex so we can swap it when a new
        // connection comes in. Each connection gets its own channel.
        let (initial_host_tx, initial_host_rx) = mpsc::channel::<HostMessage>();
        let shared_host_tx = Arc::new(parking_lot::Mutex::new(initial_host_tx));

        let shared_host_tx_for_server = shared_host_tx.clone();

        thread::Builder::new()
            .name("pipe-server".into())
            .spawn(move || {
                // Drop the initial rx immediately - no client is connected yet
                drop(initial_host_rx);

                loop {
                    info!("Pipe server waiting for connection...");

                    // Create a new channel for this connection
                    let (host_tx, host_rx) = mpsc::channel::<HostMessage>();

                    // Swap the shared sender so new messages go to this connection
                    {
                        let mut locked = shared_host_tx_for_server.lock();
                        *locked = host_tx;
                    }

                    match Self::accept_and_handle(ext_tx.clone(), host_rx) {
                        Ok(()) => info!("Pipe client disconnected normally"),
                        Err(e) => warn!("Pipe session error: {}", e),
                    }

                    thread::sleep(Duration::from_millis(100));
                }
            })
            .context("Failed to spawn pipe server thread")?;

        Ok(Self {
            receiver: parking_lot::Mutex::new(ext_rx),
            sender: shared_host_tx,
        })
    }

    fn accept_and_handle(
        ext_tx: mpsc::Sender<ExtensionMessage>,
        host_rx: mpsc::Receiver<HostMessage>,
    ) -> Result<()> {
        let pipe_name = format!("{}\0", PIPE_NAME);

        // Create named pipe using windows crate
        let handle = unsafe {
            CreateNamedPipeA(
                PCSTR::from_raw(pipe_name.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                65536, // 64KB buffer
                65536, // 64KB buffer
                0,
                None,
            )?
        };

        info!("Created named pipe, waiting for client...");

        // Wait for client connection (blocking)
        let connect_result = unsafe { ConnectNamedPipe(handle, None) };
        if connect_result.is_err() {
            let err = std::io::Error::last_os_error();
            // ERROR_PIPE_CONNECTED (535) is OK - client connected before we called ConnectNamedPipe
            if err.raw_os_error() != Some(535) {
                unsafe {
                    let _ = CloseHandle(handle);
                }
                return Err(anyhow::anyhow!("ConnectNamedPipe failed: {}", err));
            }
        }

        info!("Pipe client connected");

        // Flag set by reader when client disconnects
        let client_disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_for_reader = client_disconnected.clone();

        // Convert the pipe handle to Rust File for standard I/O.
        // We need two File handles (one for read thread, one for write in main).
        // DuplicateHandle gives us a second independent handle.
        let raw_handle = handle.0 as usize as std::os::windows::io::RawHandle;
        let read_file = unsafe { std::fs::File::from_raw_handle(raw_handle) };
        let write_file = read_file
            .try_clone()
            .context("Failed to clone pipe file handle")?;

        // Reader thread for this connection
        let reader = thread::Builder::new()
            .name("pipe-reader".into())
            .spawn(move || {
                info!("Pipe reader thread started");
                let mut reader = BufReader::new(read_file);
                loop {
                    match protocol::read_message::<ExtensionMessage>(&mut reader) {
                        Ok(Some(msg)) => {
                            info!("Pipe reader: got message, sending to channel");
                            if ext_tx.send(msg).is_err() {
                                warn!("Pipe reader: channel send failed");
                                break;
                            }
                        }
                        Ok(None) => {
                            info!("Pipe reader: client disconnected (EOF)");
                            break;
                        }
                        Err(e) => {
                            warn!("Pipe reader: error: {}", e);
                            break;
                        }
                    }
                }

                // Signal that the client has disconnected
                disconnected_for_reader.store(true, Ordering::SeqCst);
                info!("Pipe reader: signaled disconnect");

                // IMPORTANT: Don't drop read_file here - that would close the handle.
                // We'll let the cleanup below handle it via DisconnectNamedPipe.
                // Actually, we need to forget the File so it doesn't close the handle.
                // The handle is owned by accept_and_handle's cleanup.
                std::mem::forget(reader.into_inner());
            })
            .context("Failed to spawn pipe reader")?;

        // Writer: use write_file to send messages from host_rx
        let mut writer = write_file;

        // Writer loop: poll host_rx with timeout, check disconnect flag
        loop {
            if client_disconnected.load(Ordering::SeqCst) {
                break;
            }

            match host_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(msg) => {
                    if let Err(e) = protocol::write_message(&mut writer, &msg) {
                        error!("Pipe writer: failed to write: {}", e);
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Pipe writer: host channel replaced by new connection");
                    break;
                }
            }
        }

        // Cleanup: forget the write File so it doesn't double-close
        std::mem::forget(writer);

        // Disconnect and close the pipe handle
        unsafe {
            let _ = DisconnectNamedPipe(handle);
            let _ = CloseHandle(handle);
        }
        let _ = reader.join();

        Ok(())
    }

    /// Send a message to the extension via the pipe
    pub fn send(&self, msg: HostMessage) -> Result<()> {
        self.sender
            .lock()
            .send(msg)
            .context("Failed to send message to pipe")?;
        Ok(())
    }

    /// Try to receive a message without blocking
    pub fn try_recv(&self) -> Option<ExtensionMessage> {
        self.receiver.lock().try_recv().ok()
    }
}
