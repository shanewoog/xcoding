//! Interactive terminal session backed by a pseudo-terminal (ConPTY on Windows).
//!
//! The panel keeps one long-lived shell instead of running each command in a
//! fresh process, so `cd`, environment changes and interactive programs behave
//! like a real terminal.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TerminalStartResult {
    pub session_id: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "kind", content = "data")]
pub enum TerminalOutput {
    Chunk(String),
    Exit { code: Option<i32> },
    Error(String),
}

pub struct TerminalState {
    pub session: Mutex<Option<Arc<TerminalSession>>>,
}

pub struct TerminalSession {
    pub id: String,
    pub master: Mutex<Box<dyn MasterPty + Send>>,
    pub writer: Mutex<Box<dyn Write + Send>>,
    pub killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pub exited: Arc<AtomicBool>,
}

fn shell_command(root: &Path) -> CommandBuilder {
    #[cfg(windows)]
    let mut cmd = {
        const GIT_BASH_CANDIDATES: [&str; 2] = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ];
        if let Some(bash) = GIT_BASH_CANDIDATES
            .iter()
            .map(Path::new)
            .find(|candidate| candidate.exists())
        {
            let mut cmd = CommandBuilder::new(bash);
            cmd.args(["--noprofile", "--norc", "-l"]);
            cmd
        } else {
            let mut cmd = CommandBuilder::new("cmd.exe");
            cmd.args(["/K"]);
            cmd
        }
    };
    #[cfg(not(windows))]
    let mut cmd = CommandBuilder::new("bash");
    cmd.cwd(root);
    cmd.env("TERM", "xterm-256color");
    cmd
}

fn take_session(state: &State<'_, TerminalState>) -> Option<Arc<TerminalSession>> {
    state.session.lock().ok()?.take()
}

fn stop_session(session: &TerminalSession) {
    session.exited.store(true, Ordering::SeqCst);
    if let Ok(mut killer) = session.killer.lock() {
        let _ = killer.kill();
    }
}

fn read_loop(
    mut reader: Box<dyn Read + Send>,
    channel: Channel<TerminalOutput>,
    exited: Arc<AtomicBool>,
) {
    let mut buffer = [0u8; 8192];
    loop {
        if exited.load(Ordering::SeqCst) {
            return;
        }
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buffer[..n]).to_string();
                if !exited.load(Ordering::SeqCst) {
                    let _ = channel.send(TerminalOutput::Chunk(text));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if !exited.load(Ordering::SeqCst) {
                    let _ = channel.send(TerminalOutput::Error(format!(
                        "terminal read failed: {error}"
                    )));
                }
                return;
            }
        }
    }
}

#[tauri::command]
pub fn terminal_start(
    state: State<'_, TerminalState>,
    workspace_root: String,
    seed_command: Option<String>,
    channel: Channel<TerminalOutput>,
) -> Result<TerminalStartResult, String> {
    let root = PathBuf::from(workspace_root.trim());
    if workspace_root.trim().is_empty() {
        return Err("workspace root is empty".to_owned());
    }
    if !root.is_absolute() {
        return Err("workspace root must be an absolute path".to_owned());
    }
    if !root.exists() {
        return Err(format!("workspace root does not exist: {}", root.display()));
    }

    if let Some(previous) = take_session(&state) {
        stop_session(&previous);
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 96,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("failed to open pseudo-terminal: {error}"))?;

    let mut child = pair
        .slave
        .spawn_command(shell_command(&root))
        .map_err(|error| format!("failed to spawn shell: {error}"))?;
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("failed to open terminal reader: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("failed to open terminal writer: {error}"))?;
    let killer = child.clone_killer();

    let session = Arc::new(TerminalSession {
        id: Uuid::new_v4().to_string(),
        master: Mutex::new(pair.master),
        writer: Mutex::new(writer),
        killer: Mutex::new(killer),
        exited: Arc::new(AtomicBool::new(false)),
    });
    *state
        .session
        .lock()
        .map_err(|_| "terminal state poisoned".to_owned())? = Some(session.clone());

    let exited = session.exited.clone();
    let waiter_channel = channel.clone();
    std::thread::Builder::new()
        .name("xcoding-terminal-wait".to_owned())
        .spawn(move || {
            let code = child.wait().ok().map(|status| status.exit_code() as i32);
            exited.store(true, Ordering::SeqCst);
            let _ = waiter_channel.send(TerminalOutput::Exit { code });
        })
        .map_err(|error| format!("failed to spawn terminal waiter thread: {error}"))?;

    let exited = session.exited.clone();
    let reader_channel = channel;
    std::thread::Builder::new()
        .name("xcoding-terminal-read".to_owned())
        .spawn(move || read_loop(reader, reader_channel, exited))
        .map_err(|error| format!("failed to spawn terminal reader thread: {error}"))?;

    if let Some(seed) = seed_command
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        let mut writer = session
            .writer
            .lock()
            .map_err(|_| "terminal writer poisoned".to_owned())?;
        let _ = writer.write_all(seed.as_bytes());
        let _ = writer.write_all(b"\r");
        let _ = writer.flush();
    }

    Ok(TerminalStartResult {
        session_id: session.id.clone(),
    })
}

#[tauri::command]
pub fn terminal_input(
    state: State<'_, TerminalState>,
    session_id: String,
    input: String,
) -> Result<(), String> {
    let guard = state
        .session
        .lock()
        .map_err(|_| "terminal state poisoned".to_owned())?;
    let Some(session) = guard.as_ref() else {
        return Err("no terminal session is running".to_owned());
    };
    if session.id != session_id {
        return Err("terminal session changed".to_owned());
    }
    if session.exited.load(Ordering::SeqCst) {
        return Err("terminal session has exited".to_owned());
    }
    let mut writer = session
        .writer
        .lock()
        .map_err(|_| "terminal writer poisoned".to_owned())?;
    writer
        .write_all(input.as_bytes())
        .map_err(|error| format!("failed to write terminal input: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("failed to flush terminal input: {error}"))
}

#[tauri::command]
pub fn terminal_resize(
    state: State<'_, TerminalState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let guard = state
        .session
        .lock()
        .map_err(|_| "terminal state poisoned".to_owned())?;
    let Some(session) = guard.as_ref() else {
        return Err("no terminal session is running".to_owned());
    };
    if session.id != session_id {
        return Err("terminal session changed".to_owned());
    }
    let cols = cols.max(2);
    let rows = rows.max(2);
    let master = session
        .master
        .lock()
        .map_err(|_| "terminal master poisoned".to_owned())?;
    master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("failed to resize terminal: {error}"))
}

#[tauri::command]
pub fn terminal_stop(
    state: State<'_, TerminalState>,
    session_id: Option<String>,
) -> Result<(), String> {
    // Only stop the session the caller owns. A stale cleanup must not kill a
    // newer session that replaced it (React StrictMode/dev remounts can race).
    let should_stop = {
        let guard = state
            .session
            .lock()
            .map_err(|_| "terminal state poisoned".to_owned())?;
        match guard.as_ref() {
            None => false,
            Some(session) => session_id
                .as_deref()
                .map_or(true, |expected| session.id == expected),
        }
    };
    if !should_stop {
        return Ok(());
    }
    if let Some(session) = take_session(&state) {
        stop_session(&session);
        // Give the reader/wait threads a moment to observe the exit flag.
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::path::Path;
    use std::time::{Duration, Instant};

    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    use super::{shell_command, TerminalStartResult};

    #[test]
    fn terminal_start_result_uses_camel_case() {
        let value = serde_json::to_value(TerminalStartResult {
            session_id: "session-1".to_owned(),
        })
        .expect("serialize terminal start result");

        assert_eq!(value, serde_json::json!({ "sessionId": "session-1" }));
    }

    #[test]
    fn conpty_round_trips_shell_input() {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 96,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open pseudo-terminal");
        let mut child = pair
            .slave
            .spawn_command(shell_command(Path::new(
                r"D:\WORK\BittyData\XCoding",
            )))
            .expect("spawn shell");
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("clone terminal reader");
        let mut writer = pair.master.take_writer().expect("take terminal writer");
        let marker = "XCODING_CONPTY_TEST_123";
        let mut output = String::new();
        let deadline = Instant::now() + Duration::from_secs(12);
        let mut buffer = [0u8; 4096];
        let mut sent = false;
        let mut dsr_answered = false;

        while Instant::now() < deadline {
            if !sent {
                writer
                    .write_all(format!("echo {marker}\r").as_bytes())
                    .expect("write test command");
                writer.flush().expect("flush test command");
                sent = true;
            }
            if let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                output.push_str(&String::from_utf8_lossy(&buffer[..n]));
                if !dsr_answered && output.contains("\x1b[6n") {
                    writer
                        .write_all(b"\x1b[1;1R")
                        .expect("answer cursor position request");
                    writer.flush().expect("flush cursor position response");
                    dsr_answered = true;
                }
                if output.contains(marker) {
                    let _ = child.kill();
                    return;
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        let _ = child.kill();
        panic!(
            "ConPTY did not echo the command; output so far:\n{output}"
        );
    }
}
