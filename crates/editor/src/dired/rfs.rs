// SPDX-License-Identifier: MIT OR Apache-2.0
//! File operations that also work on Tether names (`/ssh:host:/path`), so a
//! Dired listing of a remote directory can delete, rename, copy and make
//! directories as a local one does. A local path goes straight to `std::fs`.

use super::*;
use crate::tether::{self, Remote};

fn io_err(e: String) -> io::Error {
    io::Error::other(e)
}

/// The remote a path names, if it names one.
fn remote(p: &Path) -> Option<Remote> {
    tether::parse_path(p)
}

pub(super) fn is_remote(p: &Path) -> bool {
    remote(p).is_some()
}

fn quoted(r: &Remote) -> String {
    tether::rp(&r.path)
}

fn run(r: &Remote, script: &str) -> io::Result<Vec<u8>> {
    tether::run(&r.hops, script, None, 30).map_err(io_err)
}

pub(super) fn exists(p: &Path) -> bool {
    match remote(p) {
        Some(r) => run(
            &r,
            &format!("test -e {} || test -L {}", quoted(&r), quoted(&r)),
        )
        .is_ok(),
        None => p.exists(),
    }
}

pub(super) fn is_dir(p: &Path) -> bool {
    match remote(p) {
        Some(r) => run(&r, &format!("test -d {}", quoted(&r))).is_ok(),
        None => fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false),
    }
}

/// `rmdir` for a directory (which must be empty), else `rm`.
pub(super) fn remove(p: &Path, dir: bool) -> io::Result<()> {
    match remote(p) {
        Some(r) => {
            let tool = if dir { "rmdir" } else { "rm" };
            run(&r, &format!("{tool} -- {}", quoted(&r))).map(|_| ())
        }
        None if dir => fs::remove_dir(p),
        None => fs::remove_file(p),
    }
}

pub(super) fn mkdir(p: &Path) -> io::Result<()> {
    match remote(p) {
        Some(r) => run(&r, &format!("mkdir -p -- {}", quoted(&r))).map(|_| ()),
        None => fs::create_dir_all(p),
    }
}

/// Move `a` to `b`. On one machine that is `mv`; between machines (or to and
/// from a local path) a file is copied across and the original removed.
pub(super) fn rename(a: &Path, b: &Path) -> io::Result<()> {
    match (remote(a), remote(b)) {
        (None, None) => fs::rename(a, b),
        (Some(x), Some(y)) if x.hops == y.hops => {
            run(&x, &format!("mv -- {} {}", quoted(&x), quoted(&y))).map(|_| ())
        }
        _ => {
            copy(a, b)?;
            remove_tree(a)
        }
    }
}

fn remove_tree(p: &Path) -> io::Result<()> {
    match remote(p) {
        Some(r) => run(&r, &format!("rm -rf -- {}", quoted(&r))).map(|_| ()),
        None if p.is_dir() => fs::remove_dir_all(p),
        None => fs::remove_file(p),
    }
}

/// Copy `a` to `b`, a directory with everything in it.
pub(super) fn copy(a: &Path, b: &Path) -> io::Result<()> {
    match (remote(a), remote(b)) {
        (None, None) => super::command::copy_recursive(a, b),
        (Some(x), Some(y)) if x.hops == y.hops => {
            run(&x, &format!("cp -R -- {} {}", quoted(&x), quoted(&y))).map(|_| ())
        }
        (Some(x), None) => {
            if is_dir(a) {
                return Err(io_err(
                    "copying a remote directory to this machine is not supported".into(),
                ));
            }
            let data = tether::read(&x).map_err(io_err)?;
            fs::write(b, data)
        }
        (None, Some(y)) => {
            if a.is_dir() {
                return Err(io_err(
                    "copying a directory to another machine is not supported".into(),
                ));
            }
            tether::write(&y, fs::read(a)?).map_err(io_err)
        }
        (Some(x), Some(y)) => {
            if is_dir(a) {
                return Err(io_err(
                    "copying a directory between machines is not supported".into(),
                ));
            }
            let data = tether::read(&x).map_err(io_err)?;
            tether::write(&y, data).map_err(io_err)
        }
    }
}

/// The parent of a directory that may be remote.
pub(super) fn parent(p: &Path) -> Option<PathBuf> {
    match remote(p) {
        Some(r) => {
            let path = r.path.trim_end_matches('/');
            if path.is_empty() || path == "/" {
                return None;
            }
            let up = match path.rfind('/') {
                Some(0) | None => "/".to_string(),
                Some(i) => path[..i].to_string(),
            };
            let mut dir = r.with_path(up).display();
            if !dir.ends_with('/') {
                dir.push('/');
            }
            Some(PathBuf::from(dir))
        }
        None => p.parent().map(Path::to_path_buf),
    }
}

/// `chmod MODE path` on a remote file; the mode is whatever `chmod` takes.
pub(super) fn chmod(p: &Path, mode: &str) -> io::Result<()> {
    let r = remote(p).ok_or_else(|| io_err("not a remote file".into()))?;
    if mode.is_empty()
        || !mode
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ",+-=".contains(c))
    {
        return Err(io_err(format!("bad mode: {mode}")));
    }
    run(&r, &format!("chmod {mode} -- {}", quoted(&r))).map(|_| ())
}

// ---------------------------------------------------------------- listing

/// One `ls -l` line as a row, or None for `total` lines and noise.
fn parse_ls_line(line: &str) -> Option<(Row, String)> {
    let mut rest = line.trim_end();
    let mut f: Vec<&str> = Vec::new();
    for _ in 0..8 {
        rest = rest.trim_start();
        let end = rest.find(char::is_whitespace)?;
        f.push(&rest[..end]);
        rest = &rest[end..];
    }
    let name = rest.strip_prefix(' ')?.trim_start_matches(' ');
    if name.is_empty() || f[0].len() < 10 || !"-dlcbpsD".contains(f[0].chars().next()?) {
        return None;
    }
    let perms: String = f[0].chars().take(10).collect();
    let raw = f[4].trim_end_matches(',').parse().unwrap_or(0);
    let plain = name.split(" -> ").next().unwrap_or(name).to_string();
    let row = Row {
        links: f[1].to_string(),
        group: f[3].to_string(),
        raw,
        blocks: 0,
        ls_date: format!("{} {:>2} {:>5}", f[5], f[6], f[7]),
        mtime: 0,
        len: raw,
        inode: String::new(),
        perms,
        size: format!("{raw}"),
        user: f[2].to_string(),
        date: String::new(),
        git: String::new(),
        name: name.to_string(),
    };
    Some((row, plain))
}

/// A remote directory as `ls -l` shows it, in Dired's `ls` layout.
pub(super) fn render(dir: &Path, marks: &HashMap<PathBuf, char>, view: View) -> io::Result<String> {
    let r = remote(dir).ok_or_else(|| io_err("not a remote directory".into()))?;
    let flag = if view.all { "A" } else { "" };
    let out = run(&r, &format!("LC_ALL=C ls -l{flag} -- {}", quoted(&r)))?;
    let mut lines: Vec<Line> = String::from_utf8_lossy(&out)
        .lines()
        .filter_map(parse_ls_line)
        .filter(|(_, plain)| plain != "." && plain != "..")
        .filter(|(_, plain)| !view.omit || !super::render::omitted(plain))
        .map(|(row, plain)| Line {
            path: dir.join(&plain),
            plain,
            row,
            branches: Vec::new(),
        })
        .collect();
    lines.sort_by(|a, b| a.plain.cmp(&b.plain));
    if view.dirs_first {
        lines.sort_by_key(|l| !l.row.perms.starts_with('d'));
    }
    if view.reverse {
        lines.reverse();
    }
    Ok(super::render::render_ls(dir, marks, view, &lines))
}
