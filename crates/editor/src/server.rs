// SPDX-License-Identifier: MIT OR Apache-2.0
//! The edit server: `server-start`, for `forgeclient`.
//!
//! A running Forge listens on 127.0.0.1 (a random port) and writes a
//! server file, `<state dir>/server/NAME`, holding the port and a random
//! key. The file is readable by its owner only, and every request has to
//! carry the key, so another user on the machine cannot drive the editor.
//!
//!
//! The protocol is one JSON object per line in each direction:
//!
//! ```text
//! {"key":K,"op":"edit","dir":"/cwd","files":[{"path":"a.rs","line":3,"col":0}],"wait":true}
//! {"key":K,"op":"eval","src":"1 + 2"}
//! {"key":K,"op":"command","name":"save-some-buffers"}
//! {"key":K,"op":"kill"}
//! ```
//!
//! Each gets `{"ok":true,"value":".."}` or `{"ok":false,"error":".."}`.
//! An edit with `wait` is answered when its buffers are finished with:
//! killed, or ended with `server-edit` (C-x #).

use crate::app::App;
use forge_core::{BufferId, Pos};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

pub const COMMANDS: &[forge_docs::CommandDoc] = &[
    forge_docs::CommandDoc::new(
        "server-edit",
        "Finish with the buffer a forgeclient is waiting for (C-x #), saving it first.",
    ),
    forge_docs::CommandDoc::new(
        "server-force-delete",
        "Stop waiting for the clients of the current buffer without finishing the buffer.",
    ),
    forge_docs::CommandDoc::new(
        "server-mode",
        "Start the edit server if it is stopped, and stop it if it is running.",
    ),
    forge_docs::CommandDoc::new(
        "server-start",
        "Start the edit server, so forgeclient can open files and run code here.",
    ),
    forge_docs::CommandDoc::new("server-shutdown", "Stop the edit server."),
];

struct Client {
    stream: TcpStream,
    pending: Vec<u8>,
}

struct Waiter {
    stream: TcpStream,
    buffers: Vec<BufferId>,
}

pub struct Server {
    pub name: String,
    pub port: u16,
    key: String,
    file: Option<PathBuf>,
    listener: TcpListener,
    clients: Vec<Client>,
    waiters: Vec<Waiter>,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(f) = &self.file {
            let _ = std::fs::remove_file(f);
        }
    }
}

/// Where the server files live.
pub fn server_dir() -> Option<PathBuf> {
    forge_core::config::state_dir().map(|d| d.join("server"))
}

/// The server file for `name`; a name with a path in it is the file.
pub fn server_file(name: &str) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        Some(PathBuf::from(name))
    } else {
        server_dir().map(|d| d.join(name))
    }
}

pub fn random_key() -> String {
    let mut bytes = [0u8; 16];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok();
    if !ok {
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            h.write_u32(std::process::id());
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Server {
    /// Listen and write the server file. With `file` None nothing is
    /// written (tests).
    pub fn start(name: &str, file: Option<PathBuf>) -> std::io::Result<Server> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let key = random_key();
        if let Some(f) = &file {
            if let Some(dir) = f.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let text = json!({"port": port, "key": key, "pid": std::process::id()}).to_string();
            write_private(f, &text)?;
        }
        Ok(Server {
            name: name.to_string(),
            port,
            key,
            file,
            listener,
            clients: Vec::new(),
            waiters: Vec::new(),
        })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// Something to check on soon (a connection or a waiting client).
    pub fn busy(&self) -> bool {
        true
    }
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)
}

fn reply(stream: &mut TcpStream, v: Value) {
    let mut line = v.to_string();
    line.push('\n');
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = stream.write_all(line.as_bytes());
    let _ = stream.flush();
}

fn ok(value: &str) -> Value {
    json!({"ok": true, "value": value})
}

fn fail(error: impl Into<String>) -> Value {
    json!({"ok": false, "error": error.into()})
}

/// `server-start`.
pub fn start(app: &mut App, name: &str) {
    if app.server.is_some() {
        app.msg("Server is already running");
        return;
    }
    match Server::start(name, server_file(name)) {
        Ok(s) => {
            let port = s.port;
            app.server = Some(s);
            app.msg(format!("Server \"{name}\" listening on port {port}"));
        }
        Err(e) => app.error(format!("Can't start the server: {e}")),
    }
}

pub fn stop(app: &mut App) {
    if let Some(mut s) = app.server.take() {
        for mut w in s.waiters.drain(..) {
            reply(&mut w.stream, fail("server stopped"));
        }
        app.msg("Server stopped");
    } else {
        app.msg("Server is not running");
    }
}

pub fn execute(app: &mut App, cmd: &str) -> bool {
    match cmd {
        "server-start" => start(app, "server"),
        "server-shutdown" => stop(app),
        "server-mode" => {
            if app.server.is_some() {
                stop(app)
            } else {
                start(app, "server")
            }
        }
        "server-edit" => server_edit(app),
        "server-force-delete" => {
            if let Some(id) = app.editor.state.active_buffer {
                finish_buffer(app, id, Some("cancelled"));
            }
        }
        _ => return false,
    }
    true
}

/// C-x #: the client waiting for this buffer is done with it.
fn server_edit(app: &mut App) {
    let Some(id) = app.editor.state.active_buffer else {
        return;
    };
    let waited = app
        .server
        .as_ref()
        .is_some_and(|s| s.waiters.iter().any(|w| w.buffers.contains(&id)));
    if !waited {
        app.msg("No server buffer here");
        return;
    }
    if app.buf_ref().is_modified() && app.buf_ref().path().is_some() {
        app.run_command("save-buffer");
    }
    finish_buffer(app, id, None);
    app.msg("Done");
}

/// Take `id` off the clients waiting for it; a client with nothing left
/// to wait for is answered.
fn finish_buffer(app: &mut App, id: BufferId, error: Option<&str>) {
    let Some(s) = app.server.as_mut() else { return };
    let mut i = 0;
    while i < s.waiters.len() {
        s.waiters[i].buffers.retain(|b| *b != id);
        if s.waiters[i].buffers.is_empty() {
            let mut w = s.waiters.remove(i);
            reply(
                &mut w.stream,
                match error {
                    Some(e) => fail(e),
                    None => ok("done"),
                },
            );
        } else {
            i += 1;
        }
    }
}

/// Called when idle: accept, read requests, answer finished waits.
pub fn poll(app: &mut App) {
    let Some(server) = app.server.as_mut() else {
        return;
    };
    while let Ok((stream, _)) = server.listener.accept() {
        if stream.set_nonblocking(true).is_ok() {
            server.clients.push(Client {
                stream,
                pending: Vec::new(),
            });
        }
    }
    let mut requests: Vec<(TcpStream, String)> = Vec::new();
    server.clients.retain_mut(|c| {
        let mut chunk = [0u8; 4096];
        loop {
            match c.stream.read(&mut chunk) {
                Ok(0) => return false,
                Ok(n) => {
                    c.pending.extend_from_slice(&chunk[..n]);
                    // A request is a line; nothing legitimate is this big.
                    if c.pending.len() > 8 << 20 {
                        return false;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => return false,
            }
        }
        if let Some(end) = c.pending.iter().position(|b| *b == b'\n') {
            let line = String::from_utf8_lossy(&c.pending[..end]).into_owned();
            if let Ok(s) = c.stream.try_clone() {
                requests.push((s, line));
            }
            return false;
        }
        true
    });
    for (mut stream, line) in requests {
        match handle(app, &line, &stream) {
            Handled::Reply(v) => reply(&mut stream, v),
            Handled::Waiting => {}
        }
    }
    // Buffers killed since last time end their waits.
    let Some(server) = app.server.as_ref() else {
        return;
    };
    let gone: Vec<BufferId> = server
        .waiters
        .iter()
        .flat_map(|w| w.buffers.iter().copied())
        .filter(|id| app.editor.state.get_buffer(*id).is_none())
        .collect();
    for id in gone {
        finish_buffer(app, id, None);
    }
}

enum Handled {
    Reply(Value),
    Waiting,
}

fn handle(app: &mut App, line: &str, stream: &TcpStream) -> Handled {
    let Ok(req) = serde_json::from_str::<Value>(line) else {
        return Handled::Reply(fail("not JSON"));
    };
    let key_ok = {
        let want = app.server.as_ref().map(|s| s.key.as_str()).unwrap_or("");
        let got = req.get("key").and_then(Value::as_str).unwrap_or("");
        // Same length check first so the comparison below does not stop
        // early on the first differing byte.
        want.len() == got.len()
            && want
                .bytes()
                .zip(got.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    };
    if !key_ok {
        return Handled::Reply(fail("bad key"));
    }
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    match op {
        "ping" => Handled::Reply(ok("pong")),
        "eval" => {
            let src = req.get("src").and_then(Value::as_str).unwrap_or("");
            match crate::plugins::eval_rhai_string(app, src) {
                Ok(v) => Handled::Reply(ok(&v)),
                Err(e) => Handled::Reply(fail(e)),
            }
        }
        "command" => {
            let name = req.get("name").and_then(Value::as_str).unwrap_or("");
            app.message = None;
            app.run_command(name);
            Handled::Reply(ok(app.message.as_deref().unwrap_or("")))
        }
        "kill" => {
            app.quit = true;
            Handled::Reply(ok("bye"))
        }
        "edit" => edit(app, &req, stream),
        other => Handled::Reply(fail(format!("unknown request {other:?}"))),
    }
}

fn edit(app: &mut App, req: &Value, stream: &TcpStream) -> Handled {
    let dir = req.get("dir").and_then(Value::as_str).map(PathBuf::from);
    let wait = req.get("wait").and_then(Value::as_bool).unwrap_or(false);
    let Some(files) = req.get("files").and_then(Value::as_array) else {
        return Handled::Reply(fail("no files"));
    };
    let mut ids = Vec::new();
    for f in files {
        let Some(path) = f.get("path").and_then(Value::as_str) else {
            continue;
        };
        let line = f.get("line").and_then(Value::as_u64).unwrap_or(0) as usize;
        let col = f.get("col").and_then(Value::as_u64).unwrap_or(0) as usize;
        let mut full = PathBuf::from(path);
        if full.is_relative()
            && let Some(d) = &dir
        {
            full = d.join(full);
        }
        if full.is_dir() {
            crate::dired::open(app, &full);
            continue;
        }
        match app.editor.open_file(&full) {
            Ok((id, _)) => {
                app.switch_to(id);
                if line > 0 {
                    app.buf()
                        .set_point(Pos::new(line.saturating_sub(1), col));
                }
                ids.push(id);
            }
            Err(e) => return Handled::Reply(fail(format!("{}: {e}", full.display()))),
        }
    }
    if wait && !ids.is_empty() {
        if let (Some(s), Ok(clone)) = (app.server.as_mut(), stream.try_clone()) {
            s.waiters.push(Waiter {
                stream: clone,
                buffers: ids,
            });
            return Handled::Waiting;
        }
    }
    Handled::Reply(ok(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    fn ask(port: u16, v: Value) -> BufReader<TcpStream> {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.write_all(format!("{v}\n").as_bytes()).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        BufReader::new(s)
    }

    fn line(r: &mut BufReader<TcpStream>) -> Value {
        let mut l = String::new();
        r.read_line(&mut l).unwrap();
        serde_json::from_str(&l).unwrap()
    }

    fn app() -> App {
        let mut editor = forge_core::Editor::new();
        editor.create_buffer("*scratch*");
        let mut app = App::new(editor);
        app.server = Some(Server::start("t", None).unwrap());
        app
    }

    #[test]
    fn requests_need_the_key_and_are_answered() {
        let mut app = app();
        let (port, key) = {
            let s = app.server.as_ref().unwrap();
            (s.port, s.key().to_string())
        };
        let mut bad = ask(port, json!({"key": "nope", "op": "ping"}));
        let mut good = ask(port, json!({"key": key, "op": "ping"}));
        for _ in 0..20 {
            poll(&mut app);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(line(&mut bad)["ok"], false);
        assert_eq!(line(&mut good)["value"], "pong");
    }

    #[test]
    fn edit_opens_the_file_at_the_line_and_wait_ends_when_it_is_killed() {
        let dir = std::env::temp_dir().join(format!("forge-server-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        let mut app = app();
        let (port, key) = {
            let s = app.server.as_ref().unwrap();
            (s.port, s.key().to_string())
        };
        let mut r = ask(
            port,
            json!({"key": key, "op": "edit", "dir": dir.to_str().unwrap(),
                   "files": [{"path": "a.txt", "line": 3, "col": 1}], "wait": true}),
        );
        for _ in 0..20 {
            poll(&mut app);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(app.buf_ref().point().line, 2);
        assert_eq!(app.buf_ref().point().col, 1);
        let id = app.editor.state.active_buffer.unwrap();
        app.kill_buffer(id);
        poll(&mut app);
        assert_eq!(line(&mut r)["ok"], true);
        let _ = std::fs::remove_dir_all(dir);
    }
}
