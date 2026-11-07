// SPDX-License-Identifier: MIT OR Apache-2.0
//! auth-source for Tether: passwords from `~/.authinfo.gpg`, `~/.authinfo`
//! and `~/.netrc`, as TRAMP finds them.
//!
//! A line is `machine HOST login USER port PORT password SECRET`
//! (`host`, `user`, `login`, `account`, `secret` and `default` are also
//! understood; values may be "quoted"; `#` starts a comment line). For
//! ssh the port is the one in the file name or `ssh`; for sudo the machine
//! is the host the hop runs on, the login is the user it becomes, and the
//! port is `sudo`. An entry without a login or port matches any.
//!
//! `.gpg` files are decrypted with `gpg --decrypt` (its agent asks for the
//! passphrase). Set `forge.opt.auth_source_files` to name other files.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    pub machine: Option<String>,
    pub login: Option<String>,
    pub port: Option<String>,
    pub password: Option<String>,
}

/// Split one line into words, honouring "double quotes" and backslashes.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut has) = (false, false);
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Parse authinfo/netrc text. A new entry starts at each `machine`,
/// `host` or `default`, so entries may span lines.
pub fn parse(text: &str) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let w = words(t);
        let mut i = 0;
        while i < w.len() {
            let key = w[i].as_str();
            match key {
                "default" => entries.push(Entry::default()),
                "machine" | "host" | "login" | "user" | "port" | "password" | "secret"
                | "account" | "protocol" | "force" | "macdef" => {
                    let val = w.get(i + 1).cloned();
                    i += 1;
                    if matches!(key, "machine" | "host") {
                        entries.push(Entry {
                            machine: val,
                            ..Entry::default()
                        });
                    } else if let Some(e) = entries.last_mut() {
                        match key {
                            "login" | "user" | "account" => e.login = val,
                            "port" | "protocol" => e.port = val,
                            "password" | "secret" => e.password = val,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
    entries
}

/// What a lookup asks for.
pub struct Query<'a> {
    pub host: &'a str,
    pub user: Option<&'a str>,
    /// Port number as typed, if any.
    pub port: Option<u16>,
    /// The service name: "ssh", "sudo".
    pub service: &'a str,
}

/// The password of the best entry in `entries` for `q`: a machine match
/// beats `default`, and a login or port that matches beats one left out.
pub fn find(entries: &[Entry], q: &Query) -> Option<String> {
    let mut best: Option<(u8, &Entry)> = None;
    for e in entries {
        let Some(pw) = &e.password else { continue };
        let _ = pw;
        let mut score = 0u8;
        match &e.machine {
            Some(m) if m == q.host => score += 4,
            Some(_) => continue,
            None => {}
        }
        if let (Some(l), Some(u)) = (&e.login, q.user) {
            if l == u {
                score += 2;
            } else {
                continue;
            }
        } else if e.login.is_some() && q.user.is_none() && q.service == "sudo" {
            // A sudo hop always names its user ("root").
            continue;
        }
        if let Some(p) = &e.port {
            let numeric = q.port.map(|n| n.to_string());
            if p == q.service || numeric.as_deref() == Some(p.as_str()) {
                score += 1;
            } else {
                continue;
            }
        }
        if best.is_none_or(|(s, _)| score > s) {
            best = Some((score, e));
        }
    }
    best.and_then(|(_, e)| e.password.clone())
}

// ------------------------------------------------------------ the files

static FILES: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// `forge.opt.auth_source_files`.
pub fn set_files(files: Option<Vec<String>>) {
    if let Ok(mut f) = FILES.lock() {
        *f = files;
    }
}

pub(crate) fn files() -> Vec<PathBuf> {
    let list = FILES
        .lock()
        .ok()
        .and_then(|f| f.clone())
        .unwrap_or_else(|| {
            vec![
                "~/.authinfo.gpg".into(),
                "~/.authinfo".into(),
                "~/.netrc".into(),
            ]
        });
    list.iter().map(|s| crate::vc::expand_home(s)).collect()
}

type Cache = HashMap<PathBuf, (SystemTime, Vec<Entry>)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn read(path: &PathBuf) -> Vec<Entry> {
    let Ok(meta) = std::fs::metadata(path) else {
        return Vec::new();
    };
    let stamp = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(c) = cache.lock()
        && let Some((t, e)) = c.get(path)
        && *t == stamp
    {
        return e.clone();
    }
    let text = if path.extension().is_some_and(|e| e == "gpg" || e == "asc") {
        std::process::Command::new("gpg")
            .args(["--quiet", "--batch", "--decrypt"])
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    } else {
        std::fs::read_to_string(path).unwrap_or_default()
    };
    let entries = parse(&text);
    // A decryption that failed is not cached: try again next time.
    if !entries.is_empty()
        && let Ok(mut c) = cache.lock()
    {
        c.insert(path.clone(), (stamp, entries.clone()));
    }
    entries
}

/// The password for `q` from the first file that has one.
pub fn lookup(q: &Query) -> Option<String> {
    files().iter().find_map(|f| find(&read(f), q))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
# comment
machine box.example login me port ssh password \"p w\\\"d\"
machine box.example login root port sudo password rootpw
machine other password otherpw
default login anyone password fallback
machine multi
  login ml
  password mlpw
";

    #[test]
    fn parses_quotes_comments_and_multiline_entries() {
        let e = parse(TEXT);
        assert_eq!(e.len(), 5);
        assert_eq!(e[0].password.as_deref(), Some("p w\"d"));
        assert_eq!(e[4].login.as_deref(), Some("ml"));
        assert_eq!(e[4].password.as_deref(), Some("mlpw"));
    }

    #[test]
    fn finds_by_machine_login_and_service() {
        let e = parse(TEXT);
        let q = |host, user, service| Query {
            host,
            user,
            port: None,
            service,
        };
        assert_eq!(
            find(&e, &q("box.example", Some("me"), "ssh")).as_deref(),
            Some("p w\"d")
        );
        assert_eq!(
            find(&e, &q("box.example", Some("root"), "sudo")).as_deref(),
            Some("rootpw")
        );
        assert_eq!(
            find(&e, &q("box.example", None, "ssh")).as_deref(),
            Some("p w\"d")
        );
        assert_eq!(
            find(&e, &q("other", None, "ssh")).as_deref(),
            Some("otherpw")
        );
        assert_eq!(
            find(&e, &q("multi", Some("ml"), "ssh")).as_deref(),
            Some("mlpw")
        );
        // A different login does not borrow another's password.
        assert_eq!(find(&e, &q("box.example", Some("zed"), "ssh")), None);
        // Nothing for an unknown host but the default entry's own login.
        assert_eq!(
            find(&e, &q("nowhere", Some("anyone"), "ssh")).as_deref(),
            Some("fallback")
        );
    }

    #[test]
    fn a_numeric_port_matches() {
        let e = parse("machine h port 2222 password x\nmachine h port 22 password y");
        let q = Query {
            host: "h",
            user: None,
            port: Some(2222),
            service: "ssh",
        };
        assert_eq!(find(&e, &q).as_deref(), Some("x"));
    }
}
