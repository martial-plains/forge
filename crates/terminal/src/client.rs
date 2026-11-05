// SPDX-License-Identifier: MIT OR Apache-2.0
//! forgeclient: open files and run code in a running Forge, like
//! a client. The server is started with `M-x server-start` or
//! `forge --daemon` (see forge_editor::server for the protocol).

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;

const USAGE: &str = "\
usage: forgeclient [OPTIONS] [+LINE[:COL]] FILE...
       forgeclient [OPTIONS] --eval EXPR...

  -n, --no-wait          return at once, without waiting for the files to be finished
  -e, --eval             evaluate the arguments as Rhai and print the values
  -u, --suppress-output  don't print the values of --eval
  -a, --alternate-editor EDITOR
                         run EDITOR if no server is running; with an empty
                         value, start `forge --daemon` and try again
  -s, --socket-name NAME the server's name (default: server, or $FORGE_SERVER_NAME)
  -f, --server-file FILE the server file to use
  -q, --quiet            don't print the informational messages
  -d, --display D, -F, --frame-parameters P, -T, --tramp-prefix P,
  -w, --timeout N, --parent-id ID, --frame-parameters=P
                         accepted and ignored
  -V, --version          show the version
  -H, --help             show this help

Finish a file you waited for with C-x # in Forge (or by killing its buffer).";

#[derive(Default)]
struct Opts {
    no_wait: bool,
    eval: bool,
    suppress: bool,
    quiet: bool,
    alternate: Option<String>,
    name: Option<String>,
    server_file: Option<String>,
    files: Vec<(String, u32, u32)>,
    exprs: Vec<String>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    let mut pos = (0u32, 0u32);
    let mut rest = false;
    while let Some(a) = it.next() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) if a.starts_with("--") => (n, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let value = |it: &mut std::slice::Iter<String>| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        if rest {
            o.exprs.push(a.clone());
            continue;
        }
        match name {
            "-n" | "--no-wait" => o.no_wait = true,
            "-e" | "--eval" => {
                o.eval = true;
                rest = true;
            }
            "-u" | "--suppress-output" => o.suppress = true,
            "-q" | "--quiet" => o.quiet = true,
            "-a" | "--alternate-editor" => o.alternate = Some(value(&mut it)?),
            "-s" | "--socket-name" => o.name = Some(value(&mut it)?),
            "-f" | "--server-file" => o.server_file = Some(value(&mut it)?),
            "-V" | "--version" => {
                println!("forgeclient (Forge {})", forge_core::VERSION);
                std::process::exit(0);
            }
            "-H" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with('+') && s.len() > 1 => {
                let spec = &s[1..];
                let (l, c) = spec.split_once(':').unwrap_or((spec, "0"));
                pos = (
                    l.parse().map_err(|_| format!("bad position {s}"))?,
                    c.parse().map_err(|_| format!("bad position {s}"))?,
                );
            }
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option {s}")),
            _ => {
                o.files.push((a.clone(), pos.0, pos.1));
                pos = (0, 0);
            }
        }
    }
    if o.eval && o.exprs.is_empty() {
        return Err("--eval needs an expression".into());
    }
    if !o.eval && o.files.is_empty() {
        return Err("no file given; see --help".into());
    }
    Ok(o)
}

fn server_file(o: &Opts) -> PathBuf {
    if let Some(f) = &o.server_file {
        return PathBuf::from(f);
    }
    let name = o
        .name
        .clone()
        .or_else(|| std::env::var("FORGE_SERVER_NAME").ok())
        .unwrap_or_else(|| "server".into());
    forge_editor::server::server_file(&name).unwrap_or_else(|| PathBuf::from(name))
}

fn connect(o: &Opts) -> Result<(TcpStream, String), String> {
    let file = server_file(o);
    let text = std::fs::read_to_string(&file).map_err(|_| {
        format!(
            "can't find the server file {}; is Forge running with a server (M-x server-start)?",
            file.display()
        )
    })?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "the server file is damaged")?;
    let port = v["port"].as_u64().ok_or("the server file has no port")? as u16;
    let key = v["key"]
        .as_str()
        .ok_or("the server file has no key")?
        .to_string();
    let s = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("can't connect to the server on port {port}: {e}"))?;
    Ok((s, key))
}

fn ask(o: &Opts, mut req: Value) -> Result<Value, String> {
    let (mut s, key) = connect(o)?;
    req["key"] = json!(key);
    s.write_all(format!("{req}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|_| "the server closed the connection".to_string())
}

fn alternate(o: &Opts, why: &str) -> ! {
    match &o.alternate {
        Some(editor) if !editor.is_empty() => {
            let mut cmd = std::process::Command::new(editor);
            for (f, _, _) in &o.files {
                cmd.arg(f);
            }
            let status = cmd.status().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
            std::process::exit(status);
        }
        Some(_) => {
            // An empty alternate editor: start a daemon, then try again.
            let exe = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("forge")))
                .filter(|p| p.exists())
                .unwrap_or_else(|| PathBuf::from("forge"));
            let ok = std::process::Command::new(exe)
                .arg("--daemon")
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                run(o);
            }
            eprintln!("forgeclient: could not start a daemon");
            std::process::exit(1);
        }
        None => {
            eprintln!("forgeclient: {why}");
            std::process::exit(1);
        }
    }
}

fn run(o: &Opts) -> ! {
    if o.eval {
        let mut failed = false;
        for expr in &o.exprs {
            match ask(o, json!({"op": "eval", "src": expr})) {
                Ok(v) if v["ok"] == true => {
                    if !o.suppress {
                        println!("{}", v["value"].as_str().unwrap_or(""));
                    }
                }
                Ok(v) => {
                    eprintln!("forgeclient: {}", v["error"].as_str().unwrap_or("error"));
                    failed = true;
                }
                Err(e) if e.starts_with("can't") => alternate(o, &e),
                Err(e) => {
                    eprintln!("forgeclient: {e}");
                    failed = true;
                }
            }
        }
        std::process::exit(failed as i32);
    }
    let dir = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    let files: Vec<Value> = o
        .files
        .iter()
        .map(|(p, l, c)| json!({"path": p, "line": l, "col": c}))
        .collect();
    let req = json!({"op": "edit", "dir": dir, "files": files, "wait": !o.no_wait});
    if !o.quiet && !o.no_wait {
        eprintln!("Waiting for Forge to finish editing (C-x # in Forge when done)...");
    }
    match ask(o, req) {
        Ok(v) if v["ok"] == true => std::process::exit(0),
        Ok(v) => {
            eprintln!("forgeclient: {}", v["error"].as_str().unwrap_or("error"));
            std::process::exit(1);
        }
        Err(e) if e.starts_with("can't") => alternate(o, &e),
        Err(e) => {
            eprintln!("forgeclient: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(o) => run(&o),
        Err(e) => {
            eprintln!("forgeclient: {e}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Opts, String> {
        parse(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn files_take_positions_and_eval_takes_the_rest() {
        let o = p(&["-n", "+4:2", "a.txt", "b.txt"]).unwrap();
        assert!(o.no_wait);
        assert_eq!(
            o.files,
            vec![("a.txt".into(), 4, 2), ("b.txt".into(), 0, 0)]
        );
        let o = p(&["-u", "--eval", "1+1", "-x"]).unwrap();
        assert!(o.suppress);
        assert_eq!(o.exprs, vec!["1+1", "-x"]);
        assert!(p(&[]).is_err());
        assert!(p(&["--bogus", "f"]).is_err());
    }
}
