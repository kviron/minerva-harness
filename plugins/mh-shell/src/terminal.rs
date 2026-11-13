//! Interactive PTY terminals for the shell plugin.
//! Output is buffered per terminal and read by cursor, so any transport (Tauri, HTTP) can poll it.
use base64::{prelude::BASE64_STANDARD, Engine};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const MAX_TERMINALS: usize = 16;
const MAX_BUFFER: usize = 1 << 20;
const MAX_INPUT: usize = 64 * 1024;

#[derive(Default)]
struct Output {
    bytes: Vec<u8>,
    /// Absolute offset of `bytes[0]`; older output was dropped to bound memory.
    start: u64,
    exited: bool,
}
struct Terminal {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Arc<Mutex<Output>>,
    reader: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[derive(Default)]
struct State {
    open: HashMap<String, Terminal>,
    stopped: bool,
}

#[derive(Default)]
pub struct Terminals {
    state: Mutex<State>,
    next: AtomicU64,
}

fn dispose(mut terminals: HashMap<String, Terminal>) {
    if terminals.is_empty() { return; }
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        for terminal in terminals.values_mut() { let _ = terminal.child.kill(); }
        for (_, mut terminal) in terminals {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
            while matches!(terminal.child.try_wait(), Ok(None)) && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let reader = terminal.reader.take().unwrap();
            // Keep draining during ClosePseudoConsole; closing it can block on output.
            drop(terminal);
            // shortcut: portable-pty cannot cancel a cloned blocking reader; do not
            // join a reader retained by descendants until the API supports cancellation.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
            while !reader.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            if reader.is_finished() { let _ = reader.join(); }
        }
        let _ = done.send(());
    });
    // Unix writer Drop and Windows ClosePseudoConsole can themselves block.
    // Keep their resources owned by the worker rather than hanging fiber disposal.
    if finished.recv_timeout(std::time::Duration::from_millis(250)).is_ok() {
        let _ = worker.join();
    }
}

impl Drop for Terminals {
    fn drop(&mut self) { self.shutdown(); }
}

fn size(cols: u16, rows: u16) -> Result<PtySize, String> {
    if !(2..=1000).contains(&cols) || !(1..=500).contains(&rows) {
        return Err("Invalid terminal size".into());
    }
    Ok(PtySize { cols, rows, pixel_width: 0, pixel_height: 0 })
}

impl Terminals {
    /// Starts the user's default shell in `cwd` and returns the terminal id.
    pub fn open(&self, cwd: &Path, cols: u16, rows: u16) -> Result<String, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        if state.stopped { return Err("Shell plugin is stopped".into()); }
        let open = &mut state.open;
        if open.len() >= MAX_TERMINALS {
            return Err("Слишком много открытых терминалов".into());
        }
        let pair = native_pty_system().openpty(size(cols, rows)?).map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let child = pair.slave.spawn_command(command).map_err(|e| e.to_string())?;
        drop(pair.slave);
        let output = Arc::new(Mutex::new(Output::default()));
        let sink = output.clone();
        let reader = std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                let read = reader.read(&mut chunk).unwrap_or(0);
                let Ok(mut out) = sink.lock() else { return };
                if read == 0 {
                    out.exited = true;
                    return;
                }
                out.bytes.extend_from_slice(&chunk[..read]);
                if out.bytes.len() > MAX_BUFFER {
                    let drop = out.bytes.len() - MAX_BUFFER;
                    out.bytes.drain(..drop);
                    out.start += drop as u64;
                }
            }
        });
        // Ids embed the process and its start so a client holding an id from a previous run never reaches a new terminal.
        let epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let id = format!("t{}-{epoch:x}-{}", std::process::id(), self.next.fetch_add(1, Ordering::Relaxed) + 1);
        open.insert(id.clone(), Terminal { master: pair.master, writer, child, output, reader: Some(reader) });
        Ok(id)
    }
    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        if data.len() > MAX_INPUT {
            return Err("Terminal input too large".into());
        }
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        let terminal = state.open.get_mut(id).ok_or("Terminal not found")?;
        terminal.writer.write_all(data.as_bytes()).and_then(|_| terminal.writer.flush()).map_err(|e| e.to_string())
    }
    /// Output after `cursor` (base64, since a chunk may split a UTF-8 sequence) and the next cursor.
    pub fn read(&self, id: &str, cursor: u64) -> Result<serde_json::Value, String> {
        let output = self.state.lock().map_err(|e| e.to_string())?.open.get(id).ok_or("Terminal not found")?.output.clone();
        let out = output.lock().map_err(|e| e.to_string())?;
        let from = cursor.max(out.start);
        let offset = usize::try_from(from - out.start).map_err(|e| e.to_string())?.min(out.bytes.len());
        Ok(serde_json::json!({
            "data": BASE64_STANDARD.encode(&out.bytes[offset..]),
            "cursor": out.start + out.bytes.len() as u64,
            "exited": out.exited,
        }))
    }
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        state.open.get(id).ok_or("Terminal not found")?.master.resize(size(cols, rows)?).map_err(|e| e.to_string())
    }
    pub fn close(&self, id: &str) -> Result<(), String> {
        let terminal = self.state.lock().map_err(|e| e.to_string())?.open.remove(id);
        dispose(terminal.map(|terminal| HashMap::from([(id.to_owned(), terminal)])).unwrap_or_default());
        Ok(())
    }

    /// Permanently stop admission, including callers retaining the service Arc.
    pub fn shutdown(&self) {
        let terminals = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.stopped = true;
            std::mem::take(&mut state.open)
        };
        dispose(terminals);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shutdown_kills_owned_child() {
        let terminals = Terminals::default();
        let dir = tempfile::tempdir().unwrap();
        let id = terminals.open(dir.path(), 80, 24).unwrap();
        let output = terminals.state.lock().unwrap().open[&id].output.clone();
        terminals.shutdown();
        // A retained output buffer also must not keep the ordinary reader alive.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !output.lock().unwrap().exited && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(output.lock().unwrap().exited);
    }
    #[test]
    fn terminal_limit_and_shutdown_admission() {
        let terminals = Terminals::default();
        let dir = tempfile::tempdir().unwrap();
        let ids: Vec<_> = (0..MAX_TERMINALS).map(|_| terminals.open(dir.path(), 80, 24).unwrap()).collect();
        assert!(terminals.open(dir.path(), 80, 24).unwrap_err().contains("терминалов"));
        terminals.close(&ids[0]).unwrap();
        terminals.open(dir.path(), 80, 24).unwrap();
        terminals.shutdown();
        assert!(terminals.state.lock().unwrap().open.is_empty());
        assert!(terminals.open(dir.path(), 80, 24).unwrap_err().contains("stopped"));
    }
    #[test]
    fn runs_input_and_streams_output_by_cursor() {
        let terminals = Terminals::default();
        let dir = tempfile::tempdir().unwrap();
        let id = terminals.open(dir.path(), 80, 24).unwrap();
        #[cfg(windows)]
        terminals.write(&id, "\x1b[1;1R").unwrap();
        #[cfg(windows)]
        let command = "echo minerva-42\r";
        #[cfg(not(windows))]
        let command = "echo minerva-$((40+2))\r";
        terminals.write(&id, command).unwrap();
        let mut seen = String::new();
        let mut cursor = 0;
        for _ in 0..100 {
            let chunk = terminals.read(&id, cursor).unwrap();
            cursor = chunk["cursor"].as_u64().unwrap();
            seen += &String::from_utf8_lossy(&BASE64_STANDARD.decode(chunk["data"].as_str().unwrap()).unwrap());
            if seen.contains("minerva-42") { break; }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(seen.contains("minerva-42"), "{seen}");
        assert!(terminals.read(&id, cursor).unwrap()["data"].as_str().unwrap().is_empty());
        terminals.resize(&id, 100, 30).unwrap();
        assert!(terminals.resize(&id, 0, 30).is_err());
        terminals.close(&id).unwrap();
        assert!(terminals.read(&id, 0).is_err());
    }
}
