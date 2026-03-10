//! Native messaging host bridge.
//! Launched by Chrome as `focusplay.exe --native-host`.
//! Bridges Chrome's stdin/stdout (native messaging protocol) to the
//! main FocusPlay instance via a named pipe.

use crate::protocol::{self, ExtensionMessage, HostMessage, PIPE_NAME};
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{self, BufReader};
use std::thread;

/// Run the native host bridge.
/// This function blocks until Chrome closes the connection (stdin EOF).
pub fn run() -> Result<()> {
    eprintln!("[FocusPlay bridge] Starting native host bridge");

    // Connect to the main FocusPlay instance via named pipe
    let pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE_NAME)
        .context("Failed to connect to FocusPlay pipe. Is FocusPlay running?")?;

    eprintln!("[FocusPlay bridge] Connected to main instance");

    let pipe_reader = pipe.try_clone().context("Failed to clone pipe handle")?;
    let mut pipe_writer = pipe;

    // Thread: read from pipe -> write to stdout (host -> extension)
    let stdout_thread = thread::Builder::new()
        .name("bridge-stdout".into())
        .spawn(move || {
            let mut pipe_reader = BufReader::new(pipe_reader);
            let mut stdout = io::stdout().lock();

            loop {
                match protocol::read_message::<HostMessage>(&mut pipe_reader) {
                    Ok(Some(msg)) => {
                        if let Err(e) = protocol::write_message(&mut stdout, &msg) {
                            eprintln!("[FocusPlay bridge] Failed to write to stdout: {}", e);
                            break;
                        }
                    }
                    Ok(None) => {
                        eprintln!("[FocusPlay bridge] Pipe closed");
                        break;
                    }
                    Err(e) => {
                        eprintln!("[FocusPlay bridge] Failed to read from pipe: {}", e);
                        break;
                    }
                }
            }
        })
        .context("Failed to spawn stdout thread")?;

    // Main thread: read from stdin -> write to pipe (extension -> host)
    let mut stdin = io::stdin().lock();
    loop {
        match protocol::read_message::<ExtensionMessage>(&mut stdin) {
            Ok(Some(msg)) => {
                if let Err(e) = protocol::write_message(&mut pipe_writer, &msg) {
                    eprintln!("[FocusPlay bridge] Failed to write to pipe: {}", e);
                    break;
                }
            }
            Ok(None) => {
                eprintln!("[FocusPlay bridge] Chrome disconnected (stdin EOF)");
                break;
            }
            Err(e) => {
                eprintln!("[FocusPlay bridge] Failed to read from stdin: {}", e);
                break;
            }
        }
    }

    // Wait for the stdout thread to finish
    let _ = stdout_thread.join();

    eprintln!("[FocusPlay bridge] Bridge shutting down");
    Ok(())
}
