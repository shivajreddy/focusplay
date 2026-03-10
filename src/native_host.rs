//! Native messaging host bridge.
//! Launched by Chrome as `focusplay.exe --native-host`.
//! Bridges Chrome's stdin/stdout (native messaging protocol) to the
//! main FocusPlay instance via a named pipe.
//!
//! The pipe is opened with FILE_FLAG_OVERLAPPED so that concurrent
//! ReadFile/WriteFile from different threads don't block each other.

use crate::protocol::{self, ExtensionMessage, PIPE_NAME};
use anyhow::{Context, Result};
use std::io::{self, Write};
use std::process;
use std::sync::Mutex;
use std::thread;
use windows::core::HSTRING;
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_SHARE_NONE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::System::IO::OVERLAPPED;

/// Simple file logger for the bridge process
fn bridge_log(msg: &str) {
    static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);
    let mut guard = LOG_FILE.lock().unwrap();
    if guard.is_none() {
        let log_path = std::env::current_exe()
            .unwrap_or_else(|_| std::path::PathBuf::from("focusplay.exe"))
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("bridge.log");
        *guard = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .ok();
    }
    if let Some(ref mut f) = *guard {
        let _ = writeln!(f, "[bridge] {}", msg);
        let _ = f.flush();
    }
}

/// Run the native host bridge.
pub fn run() -> Result<()> {
    bridge_log("Starting native host bridge");

    // Open pipe with FILE_FLAG_OVERLAPPED so concurrent read/write work
    let pipe_handle = match open_pipe() {
        Ok(h) => h,
        Err(e) => {
            bridge_log(&format!(
                "Cannot connect to FocusPlay pipe: {}. Exiting.",
                e
            ));
            process::exit(0);
        }
    };

    bridge_log("Connected to main instance");

    // Thread: read from pipe -> write to stdout (host -> extension)
    let handle_raw = pipe_handle.0 as usize; // Convert to usize for Send
    let _stdout_thread = thread::Builder::new()
        .name("bridge-stdout".into())
        .spawn(move || {
            let pipe_handle = HANDLE(handle_raw as *mut _);
            bridge_log("Stdout thread started");
            let mut stdout = io::stdout().lock();

            loop {
                let mut len_buf = [0u8; 4];
                if !overlapped_read(pipe_handle, &mut len_buf) {
                    bridge_log("Pipe read failed (length)");
                    break;
                }

                let len = u32::from_le_bytes(len_buf) as usize;
                if len == 0 || len > 1024 * 1024 {
                    bridge_log(&format!("Invalid message length: {}", len));
                    break;
                }

                let mut buf = vec![0u8; len];
                if !overlapped_read(pipe_handle, &mut buf) {
                    bridge_log("Pipe read failed (payload)");
                    break;
                }

                let len_bytes = (buf.len() as u32).to_le_bytes();
                if stdout.write_all(&len_bytes).is_err()
                    || stdout.write_all(&buf).is_err()
                    || stdout.flush().is_err()
                {
                    bridge_log("Failed to write to stdout");
                    break;
                }
            }

            bridge_log("Pipe reader done, exiting process");
            process::exit(0);
        })
        .context("Failed to spawn stdout thread")?;

    // Main thread: read from stdin -> write to pipe
    bridge_log("Main thread: reading from Chrome stdin...");
    let mut stdin = io::stdin().lock();

    loop {
        match protocol::read_message::<ExtensionMessage>(&mut stdin) {
            Ok(Some(msg)) => {
                bridge_log(&format!("Stdin->pipe: {:?}", msg));

                let json = serde_json::to_vec(&msg).unwrap();
                let len_bytes = (json.len() as u32).to_le_bytes();

                if !overlapped_write(pipe_handle, &len_bytes) {
                    bridge_log("Failed to write length to pipe");
                    break;
                }
                if !overlapped_write(pipe_handle, &json) {
                    bridge_log("Failed to write payload to pipe");
                    break;
                }

                bridge_log("Message forwarded to pipe OK");
            }
            Ok(None) => {
                bridge_log("Chrome disconnected (stdin EOF)");
                break;
            }
            Err(e) => {
                bridge_log(&format!("Failed to read from stdin: {}", e));
                break;
            }
        }
    }

    bridge_log("Bridge shutting down");
    process::exit(0);
}

/// Open the named pipe as a client with overlapped I/O
fn open_pipe() -> Result<HANDLE> {
    let pipe_path = HSTRING::from(PIPE_NAME);
    let handle = unsafe {
        CreateFileW(
            &pipe_path,
            (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            None,
        )
        .context("Failed to open pipe")?
    };
    Ok(handle)
}

/// Overlapped read: creates a per-call event and waits for completion
fn overlapped_read(handle: HANDLE, buf: &mut [u8]) -> bool {
    let mut total = 0;
    while total < buf.len() {
        let event = unsafe { CreateEventW(None, true, false, None) };
        let event = match event {
            Ok(e) => e,
            Err(_) => return false,
        };

        let mut overlapped = OVERLAPPED::default();
        overlapped.hEvent = event;

        let mut bytes_read: u32 = 0;
        let result = unsafe {
            ReadFile(
                handle,
                Some(&mut buf[total..]),
                Some(&mut bytes_read),
                Some(&mut overlapped),
            )
        };

        if result.is_err() {
            let err = unsafe { windows::Win32::Foundation::GetLastError() };
            if err == windows::Win32::Foundation::ERROR_IO_PENDING {
                // Wait for the I/O to complete
                let wait = unsafe { WaitForSingleObject(event, 30000) }; // 30s timeout
                if wait != WAIT_OBJECT_0 {
                    unsafe {
                        let _ = windows::Win32::Foundation::CloseHandle(event);
                    }
                    return false;
                }
                // Get the result
                let mut transferred: u32 = 0;
                let ok = unsafe {
                    windows::Win32::System::IO::GetOverlappedResult(
                        handle,
                        &overlapped,
                        &mut transferred,
                        false,
                    )
                };
                if ok.is_err() || transferred == 0 {
                    unsafe {
                        let _ = windows::Win32::Foundation::CloseHandle(event);
                    }
                    return false;
                }
                bytes_read = transferred;
            } else {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(event);
                }
                return false;
            }
        }

        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(event);
        }

        if bytes_read == 0 {
            return false;
        }
        total += bytes_read as usize;
    }
    true
}

/// Overlapped write: creates a per-call event and waits for completion
fn overlapped_write(handle: HANDLE, buf: &[u8]) -> bool {
    let mut total = 0;
    while total < buf.len() {
        let event = unsafe { CreateEventW(None, true, false, None) };
        let event = match event {
            Ok(e) => e,
            Err(_) => return false,
        };

        let mut overlapped = OVERLAPPED::default();
        overlapped.hEvent = event;

        let mut bytes_written: u32 = 0;
        let result = unsafe {
            WriteFile(
                handle,
                Some(&buf[total..]),
                Some(&mut bytes_written),
                Some(&mut overlapped),
            )
        };

        if result.is_err() {
            let err = unsafe { windows::Win32::Foundation::GetLastError() };
            if err == windows::Win32::Foundation::ERROR_IO_PENDING {
                let wait = unsafe { WaitForSingleObject(event, 30000) };
                if wait != WAIT_OBJECT_0 {
                    unsafe {
                        let _ = windows::Win32::Foundation::CloseHandle(event);
                    }
                    return false;
                }
                let mut transferred: u32 = 0;
                let ok = unsafe {
                    windows::Win32::System::IO::GetOverlappedResult(
                        handle,
                        &overlapped,
                        &mut transferred,
                        false,
                    )
                };
                if ok.is_err() || transferred == 0 {
                    unsafe {
                        let _ = windows::Win32::Foundation::CloseHandle(event);
                    }
                    return false;
                }
                bytes_written = transferred;
            } else {
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(event);
                }
                return false;
            }
        }

        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(event);
        }

        if bytes_written == 0 {
            return false;
        }
        total += bytes_written as usize;
    }
    true
}
