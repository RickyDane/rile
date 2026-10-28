//! One Rile per Windows session. macOS gets this from LaunchServices; on
//! Windows each launch (Explorer's "Open in Rile", a `rile://` link, the
//! tray helper) starts a new process, which hands its arguments to the
//! running instance over a named pipe and exits.

use std::io::{Read, Write};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_INBOUND};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::HSTRING;

fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!(r"\\.\pipe\dev.rile.instance.{user}")
}

/// Becomes the primary instance (returning a channel of argument lists that
/// later launches forward), or forwards `args` to the running one and returns
/// `None`, in which case the caller should exit.
pub fn claim(args: &[String]) -> Option<smol::channel::Receiver<Vec<String>>> {
    // Session-local; the handle stays open for the process lifetime.
    let mutex = unsafe { CreateMutexW(None, false, &HSTRING::from(r"Local\dev.rile.instance")) };
    let already_running = mutex.is_ok() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_running && forward(args) {
        return None;
    }
    let (tx, rx) = smol::channel::unbounded();
    std::thread::Builder::new().name("rile-instance".into()).spawn(move || serve(tx)).ok()?;
    Some(rx)
}

fn forward(args: &[String]) -> bool {
    // Let the running instance bring its window to the front.
    unsafe { windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(windows::Win32::UI::WindowsAndMessaging::ASFW_ANY).ok() };
    // The first instance may still be creating its pipe.
    for _ in 0..40 {
        if let Ok(mut pipe) = std::fs::OpenOptions::new().write(true).open(pipe_name()) {
            return pipe.write_all(args.join("\n").as_bytes()).is_ok();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn serve(tx: smol::channel::Sender<Vec<String>>) {
    let name = HSTRING::from(pipe_name());
    let mut first = true;
    loop {
        let flags = if first { PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE } else { PIPE_ACCESS_INBOUND };
        first = false;
        let pipe = unsafe {
            CreateNamedPipeW(&name, flags, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS, 1, 0, 64 * 1024, 0, None)
        };
        if pipe.is_invalid() {
            return;
        }
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok() || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if connected {
            let mut message = String::new();
            let mut reader = PipeReader(pipe);
            if reader.read_to_string(&mut message).is_ok() {
                let args = message.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect();
                if tx.send_blocking(args).is_err() {
                    return;
                }
            }
            unsafe { DisconnectNamedPipe(pipe).ok() };
        }
        unsafe { CloseHandle(pipe).ok() };
    }
}

struct PipeReader(HANDLE);

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut read = 0u32;
        match unsafe { windows::Win32::Storage::FileSystem::ReadFile(self.0, Some(buf), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            // The client closed its end: end of message.
            Err(e) if e.code() == windows::Win32::Foundation::ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
            Err(e) => Err(std::io::Error::other(e)),
        }
    }
}
