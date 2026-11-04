// SPDX-License-Identifier: MIT OR Apache-2.0
//! file-name and directory functions (`file-name-directory`,
//! `directory-files`, `directory-files-recursively`, `locate-dominating-file`,
//! `file-expand-wildcards`, `copy-directory`, `insert-directory`, ...), as
//! plain Rust over `&str` names, so the commands, the plugin API and the
//! scripts all share one behaviour.
//!
//! Names are text: a directory name ends in `/`, `~` is the
//! home directory, and a relative name is relative to a default directory
//! the caller gives.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use regex::Regex;

const fn is_sep(c: char) -> bool {
    c == '/' || (cfg!(windows) && c == '\\')
}

fn last_sep(s: &str) -> Option<usize> {
    s.rfind(is_sep)
}

// ------------------------------------------------------------ file names

/// `(file-name-directory F)`: everything up to and including the last `/`;
/// None when there is none.
pub fn file_name_directory(f: &str) -> Option<String> {
    last_sep(f).map(|i| f[..=i].to_string())
}

/// `(file-name-nondirectory F)`: what follows the last `/`.
pub fn file_name_nondirectory(f: &str) -> String {
    last_sep(f).map_or(f, |i| &f[i + 1..]).to_string()
}

/// `(file-name-as-directory F)`: F with a trailing `/` (`""` becomes `./`).
pub fn file_name_as_directory(f: &str) -> String {
    if f.is_empty() {
        "./".into()
    } else if f.ends_with(is_sep) {
        f.into()
    } else {
        format!("{f}/")
    }
}

/// `(directory-file-name D)`: D without its trailing `/` (a root stays).
pub fn directory_file_name(d: &str) -> String {
    let t = d.trim_end_matches(is_sep);
    if t.is_empty() && !d.is_empty() { d[..1].to_string() } else { t.to_string() }
}

/// `(file-name-extension F PERIOD)`: the extension of F's last component, or
/// None when it has none (a leading dot isn't one: `.hidden`). With `period`
/// the dot is kept.
pub fn file_name_extension(f: &str, period: bool) -> Option<String> {
    let name = file_name_nondirectory(f);
    let i = name.rfind('.').filter(|&i| i > 0)?;
    Some(if period { name[i..].to_string() } else { name[i + 1..].to_string() })
}

/// `(file-name-sans-extension F)`.
pub fn file_name_sans_extension(f: &str) -> String {
    let at = last_sep(f).map_or(0, |i| i + 1);
    match f[at..].rfind('.').filter(|&i| i > 0) {
        Some(i) => f[..at + i].to_string(),
        None => f.to_string(),
    }
}

/// `(file-name-base F)`: the last component without its extension.
pub fn file_name_base(f: &str) -> String {
    file_name_sans_extension(&file_name_nondirectory(f))
}

/// `(file-name-with-extension F EXT)`: F with its extension replaced (or
/// added) by EXT, which may start with a dot.
pub fn file_name_with_extension(f: &str, ext: &str) -> Result<String, String> {
    let ext = ext.strip_prefix('.').unwrap_or(ext);
    if f.is_empty() || f.ends_with(is_sep) {
        return Err(format!("Malformed filename: {f}"));
    }
    if ext.is_empty() || ext.contains(is_sep) {
        return Err(format!("Malformed extension: {ext}"));
    }
    Ok(format!("{}.{ext}", file_name_sans_extension(f)))
}

/// `(file-name-concat DIR &rest COMPONENTS)`: the parts joined with one `/`
/// between them; empty parts are skipped.
pub fn file_name_concat(parts: &[&str]) -> String {
    let parts: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        out.push_str(p);
        if i + 1 < parts.len() && !p.ends_with(is_sep) {
            out.push('/');
        }
    }
    out
}

/// `(file-name-absolute-p F)`: starts at the root or the home directory.
pub fn file_name_absolute_p(f: &str) -> bool {
    f.starts_with(is_sep) || f == "~" || f.starts_with("~/") || {
        let b = f.as_bytes();
        cfg!(windows) && b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && is_sep(b[2] as char)
    }
}

/// `(file-name-split F)`: the components; an absolute name starts with `""`
/// and a directory name ends with `""`.
pub fn file_name_split(f: &str) -> Vec<String> {
    f.split(is_sep).map(str::to_string).collect()
}

/// `(file-name-parent-directory F)`: the directory F is in, as a directory
/// name; None for the root, and for a bare relative name, `../`'s way up.
pub fn file_name_parent_directory(f: &str) -> Option<String> {
    let t = directory_file_name(f);
    if t == "/" || t.is_empty() {
        return None;
    }
    match file_name_directory(&t) {
        Some(d) => Some(d),
        None if t == "." || t == ".." || t.ends_with("/..") => Some(format!("{t}/../")),
        None => Some("./".to_string()).filter(|_| !file_name_absolute_p(&t)).or(None),
    }
}

/// The home directory, from the environment.
pub fn home() -> Option<String> {
    std::env::var("HOME").ok().or_else(|| std::env::var("USERPROFILE").ok()).filter(|h| !h.is_empty())
}

fn default_dir(dir: Option<&str>) -> String {
    match dir {
        Some(d) if !d.is_empty() => expand_file_name(d, None),
        _ => std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_else(|_| "/".into()),
    }
}

/// `(expand-file-name NAME DIR)`: an absolute name, with `~` expanded, a
/// relative NAME taken from DIR (default: the current directory), and `.`,
/// `..` and doubled slashes settled. A trailing `/` stays.
pub fn expand_file_name(name: &str, dir: Option<&str>) -> String {
    let full = if name == "~" || name.starts_with("~/") {
        match home() {
            Some(h) => format!("{}/{}", h.trim_end_matches(is_sep), name[1..].trim_start_matches(is_sep)),
            None => name.to_string(),
        }
    } else if file_name_absolute_p(name) {
        name.to_string()
    } else {
        format!("{}/{}", default_dir(dir).trim_end_matches(is_sep), name)
    };
    let trailing = full.ends_with(is_sep) || full.ends_with("/.") || full.ends_with("/..");
    let rooted = full.starts_with(is_sep);
    let mut parts: Vec<&str> = Vec::new();
    for c in full.split(is_sep) {
        match c {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != ".." && !(p.len() == 2 && p.ends_with(':'))) {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            c => parts.push(c),
        }
    }
    let mut out = if rooted { "/".to_string() } else { String::new() };
    out.push_str(&parts.join("/"));
    if trailing && !out.ends_with('/') {
        out.push('/');
    }
    out
}

/// `(file-relative-name F DIR)`: F as seen from DIR (default: the current
/// directory), with `..` where it must climb.
pub fn file_relative_name(f: &str, dir: Option<&str>) -> String {
    let (f, d) = (expand_file_name(f, None), expand_file_name(&file_name_as_directory(&default_dir(dir)), None));
    let fc: Vec<&str> = f.split(is_sep).filter(|s| !s.is_empty()).collect();
    let dc: Vec<&str> = d.split(is_sep).filter(|s| !s.is_empty()).collect();
    let common = fc.iter().zip(&dc).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = std::iter::repeat_n("..", dc.len() - common).collect();
    out.extend(&fc[common..]);
    let mut s = out.join("/");
    if s.is_empty() {
        s = ".".into();
    }
    if f.ends_with('/') && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// `(abbreviate-file-name F)`: F with the home directory as `~`.
pub fn abbreviate_file_name(f: &str) -> String {
    if let Some(h) = home() {
        let h = h.trim_end_matches(is_sep);
        if f == h {
            return "~".into();
        }
        if let Some(rest) = f.strip_prefix(h).filter(|r| r.starts_with(is_sep)) {
            return format!("~{rest}");
        }
    }
    f.to_string()
}

// ----------------------------------------------------------- directories

/// `directory-files-no-dot-files-regexp`: matches any file name except `.`
/// and `..`.
pub const NO_DOT_FILES_REGEXP: &str = r"[^.]|\.\.\.";

fn path_of(s: &str) -> PathBuf {
    PathBuf::from(expand_file_name(s, None))
}

/// The names in a directory, with `.` and `..` lists them, sorted
/// (by character code) unless `nosort`.
fn names_in(dir: &str, nosort: bool) -> io::Result<Vec<String>> {
    let mut v = vec![".".to_string(), "..".to_string()];
    for e in fs::read_dir(path_of(dir))? {
        v.push(e?.file_name().to_string_lossy().into_owned());
    }
    if !nosort {
        v.sort();
    }
    Ok(v)
}

/// `(directory-files DIR FULL MATCH NOSORT COUNT)`: the names in DIR, `.` and
/// `..` included. `full` makes them absolute; `matching` keeps those whose
/// own name (not the directory part) contains a match; `count` stops after
/// that many.
pub fn directory_files(dir: &str, full: bool, matching: Option<&Regex>, nosort: bool, count: Option<usize>) -> io::Result<Vec<String>> {
    let base = if full { Some(expand_file_name(&file_name_as_directory(dir), None)) } else { None };
    let mut out = Vec::new();
    for n in names_in(dir, nosort)? {
        if count.is_some_and(|c| out.len() >= c) {
            break;
        }
        if matching.is_some_and(|re| !re.is_match(&n)) {
            continue;
        }
        out.push(match &base {
            Some(b) => format!("{b}{n}"),
            None => n,
        });
    }
    Ok(out)
}

/// What `file-attributes` says, the parts Forge has: kind (`Dir`, `Link`,
/// `File`), link count, owner and group ids, size, times (seconds since
/// 1970), and the `ls`-style mode string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attrs {
    pub kind: Kind,
    pub links: u64,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub modified: i64,
    pub accessed: i64,
    pub changed: i64,
    pub mode: String,
    /// What a symbolic link points at.
    pub target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Link,
}

fn secs(t: io::Result<std::time::SystemTime>) -> i64 {
    t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

fn mode_string(meta: &fs::Metadata, kind: Kind) -> String {
    let lead = match kind {
        Kind::Dir => 'd',
        Kind::Link => 'l',
        Kind::File => '-',
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = meta.permissions().mode();
        let bit = |b: u32, c: char| if m & b != 0 { c } else { '-' };
        let x = |b: u32, s: u32, on: char, off: char| match (m & b != 0, m & s != 0) {
            (true, true) => on,
            (false, true) => off,
            (true, false) => 'x',
            (false, false) => '-',
        };
        format!(
            "{lead}{}{}{}{}{}{}{}{}{}",
            bit(0o400, 'r'),
            bit(0o200, 'w'),
            x(0o100, 0o4000, 's', 'S'),
            bit(0o040, 'r'),
            bit(0o020, 'w'),
            x(0o010, 0o2000, 's', 'S'),
            bit(0o004, 'r'),
            bit(0o002, 'w'),
            x(0o001, 0o1000, 't', 'T'),
        )
    }
    #[cfg(not(unix))]
    {
        let w = if meta.permissions().readonly() { '-' } else { 'w' };
        format!("{lead}r{w}x------")
    }
}

/// `(file-attributes F)`, without following a final symbolic link.
pub fn file_attributes(f: &str) -> io::Result<Attrs> {
    let p = path_of(f);
    let meta = fs::symlink_metadata(&p)?;
    let kind = if meta.file_type().is_symlink() {
        Kind::Link
    } else if meta.is_dir() {
        Kind::Dir
    } else {
        Kind::File
    };
    #[cfg(unix)]
    let (links, uid, gid, changed) = {
        use std::os::unix::fs::MetadataExt;
        (meta.nlink(), meta.uid(), meta.gid(), meta.ctime())
    };
    #[cfg(not(unix))]
    let (links, uid, gid, changed) = (1, 0, 0, secs(meta.modified()));
    Ok(Attrs {
        kind,
        links,
        uid,
        gid,
        size: meta.len(),
        modified: secs(meta.modified()),
        accessed: secs(meta.accessed()),
        changed,
        mode: mode_string(&meta, kind),
        target: (kind == Kind::Link).then(|| fs::read_link(&p).map(|t| t.display().to_string()).unwrap_or_default()),
    })
}

/// `(directory-files-and-attributes DIR FULL MATCH NOSORT ID-FORMAT COUNT)`:
/// each name with its attributes.
pub fn directory_files_and_attributes(dir: &str, full: bool, matching: Option<&Regex>, nosort: bool, count: Option<usize>) -> io::Result<Vec<(String, Attrs)>> {
    let base = expand_file_name(&file_name_as_directory(dir), None);
    let mut out = Vec::new();
    for name in directory_files(dir, full, matching, nosort, count)? {
        let at = if full { name.clone() } else { format!("{base}{name}") };
        if let Ok(a) = file_attributes(&at) {
            out.push((name, a));
        }
    }
    Ok(out)
}

/// `(directory-empty-p D)`: an accessible directory with nothing in it
/// (a symbolic link to one counts).
pub fn directory_empty_p(d: &str) -> bool {
    let p = path_of(d);
    p.is_dir() && fs::read_dir(&p).is_ok_and(|mut r| r.next().is_none())
}

/// `(directory-files-recursively DIR REGEXP INCLUDE-DIRECTORIES PREDICATE
/// FOLLOW-SYMLINKS)`: the absolute names of the files under DIR whose own
/// name matches, depth first, each directory's entries sorted by name.
/// `descend` says whether to go into a subdirectory (default: all);
/// an unreadable directory is skipped.
pub fn directory_files_recursively(
    dir: &str,
    regexp: &Regex,
    include_dirs: bool,
    descend: Option<&dyn Fn(&str) -> bool>,
    follow_symlinks: bool,
) -> Vec<String> {
    fn walk(dir: &str, re: &Regex, inc: bool, descend: Option<&dyn Fn(&str) -> bool>, follow: bool, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(path_of(dir)) else { return };
        let mut names: Vec<String> = rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        let base = file_name_as_directory(dir);
        for n in names {
            let full = format!("{base}{n}");
            let p = path_of(&full);
            let is_link = fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink());
            if p.is_dir() {
                if (follow || !is_link) && descend.is_none_or(|f| f(&full)) {
                    walk(&full, re, inc, descend, follow, out);
                }
                if inc && re.is_match(&n) {
                    out.push(full);
                }
            } else if re.is_match(&n) {
                out.push(full);
            }
        }
    }
    let mut out = Vec::new();
    walk(&expand_file_name(dir, None), regexp, include_dirs, descend, follow_symlinks, &mut out);
    out
}

/// `(locate-dominating-file FILE NAME)`: from FILE (or the directory it is
/// in) up through its parents, the first directory where `found` says yes
/// (it is called with the directory name). The directory comes back with a
/// trailing `/`; None at the top without a hit.
pub fn locate_dominating_file(file: &str, found: &dyn Fn(&str) -> bool) -> Option<String> {
    let abs = expand_file_name(file, None);
    let mut dir = if path_of(&abs).is_dir() { file_name_as_directory(&abs) } else { file_name_directory(&abs).unwrap_or_else(|| "/".into()) };
    loop {
        if found(&dir) {
            return Some(dir);
        }
        let t = directory_file_name(&dir);
        let parent = file_name_directory(&t)?;
        if parent == dir || t == "/" || t.is_empty() {
            return None;
        }
        dir = parent;
    }
}

/// `locate-dominating-file` for a file name: the first directory holding
/// something called `name`.
pub fn locate_dominating_name(file: &str, name: &str) -> Option<String> {
    locate_dominating_file(file, &|d| fs::symlink_metadata(Path::new(d).join(name)).is_ok())
}

/// The real name of a file, even one that doesn't exist yet: the nearest
/// parent that does is resolved (links and all) and the rest stays.
fn truename(f: &str) -> PathBuf {
    let p = path_of(f);
    let mut rest = Vec::new();
    let mut cur = p.as_path();
    loop {
        if let Ok(c) = cur.canonicalize() {
            return rest.iter().rev().fold(c, |acc, part: &std::ffi::OsString| acc.join(part));
        }
        match (cur.file_name(), cur.parent()) {
            (Some(n), Some(par)) => {
                rest.push(n.to_os_string());
                cur = par;
            }
            _ => return p,
        }
    }
}

/// `(file-in-directory-p FILE DIR)`: FILE is in DIR, or below it, or is it,
/// by true name. False when DIR isn't a directory.
pub fn file_in_directory_p(file: &str, dir: &str) -> bool {
    let d = path_of(dir);
    if !d.is_dir() {
        return false;
    }
    let (f, d) = (truename(file), truename(dir));
    f.starts_with(d)
}

/// A shell wildcard as a regexp: `*`, `?`, `[a-z]`, `[!a]`; anything else is
/// itself.
pub fn wildcard_to_regexp(w: &str) -> String {
    let mut re = String::from("^");
    let mut it = w.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '[' => {
                let mut set = String::new();
                let mut closed = false;
                for d in it.by_ref() {
                    if d == ']' && !set.is_empty() {
                        closed = true;
                        break;
                    }
                    set.push(d);
                }
                if closed {
                    re.push('[');
                    if let Some(rest) = set.strip_prefix('!') {
                        re.push('^');
                        re.push_str(&rest.replace('\\', "\\\\"));
                    } else {
                        re.push_str(&set.replace('\\', "\\\\"));
                    }
                    re.push(']');
                } else {
                    re.push_str("\\[");
                    re.push_str(&regex::escape(&set));
                }
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
    }
    re.push('$');
    re
}

fn has_wildcard(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

/// `(file-expand-wildcards PATTERN FULL REGEXP)`: the files a wildcard
/// names, sorted. The match is made a directory at a time, so it can't span
/// a `/`. With `regexp`, only the last component is a regexp. An absolute
/// PATTERN gives absolute names; a relative one gives names relative to
/// `dir` (default: the current directory), or absolute with `full`.
pub fn file_expand_wildcards(pattern: &str, full: bool, regexp: bool, dir: Option<&str>) -> Result<Vec<String>, String> {
    let absolute = file_name_absolute_p(pattern);
    let root = if absolute { String::new() } else { file_name_as_directory(&default_dir(dir)) };
    let pat = if pattern == "~" || pattern.starts_with("~/") { expand_file_name(pattern, None) } else { pattern.to_string() };
    let comps: Vec<&str> = pat.split(is_sep).collect();
    let n = comps.len();
    let mut hits: Vec<String> = vec![if pat.starts_with(is_sep) { "/".to_string() } else { String::new() }];
    for (i, c) in comps.iter().enumerate() {
        if c.is_empty() {
            continue;
        }
        let last = i + 1 == n;
        let re = if regexp && last {
            Some(Regex::new(c).map_err(|e| e.to_string())?)
        } else if !regexp && has_wildcard(c) {
            Some(Regex::new(&wildcard_to_regexp(c)).map_err(|e| e.to_string())?)
        } else {
            None
        };
        let mut next = Vec::new();
        for h in &hits {
            let at = format!("{root}{h}");
            match &re {
                Some(re) => {
                    let Ok(rd) = fs::read_dir(path_of(if at.is_empty() { "." } else { &at })) else { continue };
                    let mut names: Vec<String> = rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
                    names.sort();
                    for name in names.into_iter().filter(|nm| re.is_match(nm)) {
                        next.push(format!("{h}{name}{}", if last { "" } else { "/" }));
                    }
                }
                None => {
                    let cand = format!("{h}{c}{}", if last { "" } else { "/" });
                    if fs::symlink_metadata(path_of(&format!("{root}{cand}"))).is_ok() {
                        next.push(cand);
                    }
                }
            }
        }
        hits = next;
    }
    hits.retain(|h| !h.is_empty());
    hits.sort();
    Ok(hits
        .into_iter()
        .map(|h| {
            let h = h.trim_end_matches('/').to_string();
            if full && !absolute { format!("{root}{h}") } else { h }
        })
        .collect())
}

// ------------------------------------------------------------- changes

/// `(make-directory DIR PARENTS)`: make a directory. Without `parents` the
/// parent must exist. Ok(true) when made, Ok(false) when it was already
/// there and `parents` is set (without it that is an error).
pub fn make_directory(dir: &str, parents: bool) -> io::Result<bool> {
    let p = path_of(dir);
    if p.is_dir() && parents {
        return Ok(false);
    }
    if parents { fs::create_dir_all(&p) } else { fs::create_dir(&p) }.map(|()| true)
}

/// `(make-empty-file FILE PARENTS)`: make an empty file; it is an error if
/// it exists.
pub fn make_empty_file(file: &str, parents: bool) -> io::Result<()> {
    let p = path_of(file);
    if parents && let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    fs::OpenOptions::new().write(true).create_new(true).open(p).map(|_| ())
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_tree(&src, &dst)?;
        } else if ft.is_symlink() {
            #[cfg(unix)]
            {
                let _ = fs::remove_file(&dst);
                std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
            }
            #[cfg(not(unix))]
            fs::copy(&src, &dst).map(|_| ())?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// `(copy-directory FROM TO KEEP-TIME PARENTS COPY-CONTENTS)`. When TO is a
/// directory that exists, FROM is copied *into* it (as `TO/NAME`) unless
/// `copy_contents`, which copies what is inside FROM into TO. Returns the
/// directory that was written.
pub fn copy_directory(from: &str, to: &str, parents: bool, copy_contents: bool) -> io::Result<String> {
    let src = path_of(from);
    if !src.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{from}: not a directory")));
    }
    let mut dst = path_of(to);
    let into_existing = dst.is_dir() && !copy_contents && !to.is_empty();
    if into_existing || to.ends_with(is_sep) && !copy_contents {
        dst = dst.join(src.file_name().unwrap_or_default());
    }
    if dst.starts_with(&src) && dst != src {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a directory into itself"));
    }
    if !parents && let Some(par) = dst.parent() && !par.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{}: no such directory", par.display())));
    }
    copy_tree(&src, &dst)?;
    Ok(dst.display().to_string())
}

/// `(delete-directory D RECURSIVE)`: remove a directory; without
/// `recursive` it must be empty.
pub fn delete_directory(dir: &str, recursive: bool) -> io::Result<()> {
    let p = path_of(dir);
    if fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
        return fs::remove_file(p);
    }
    if recursive { fs::remove_dir_all(p) } else { fs::remove_dir(p) }
}

// ------------------------------------------------------- insert-directory

fn civil(days: i64) -> (i64, usize, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), (m - 1) as usize, d)
}

/// `ls`'s date: `Mon DD HH:MM` for the last six months, else `Mon DD  YYYY`.
fn ls_date(t: i64, now: i64) -> String {
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (y, m, d) = civil(t.div_euclid(86_400));
    let rem = t.rem_euclid(86_400);
    if (now - t).abs() < 182 * 86_400 {
        format!("{} {d:>2} {:02}:{:02}", M[m], rem / 3600, rem % 3600 / 60)
    } else {
        format!("{} {d:>2}  {y}", M[m])
    }
}

fn human(n: u64) -> String {
    let units = ["", "K", "M", "G", "T", "P"];
    let (mut v, mut u) = (n as f64, 0);
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        n.to_string()
    } else if v < 10.0 {
        format!("{:.1}{}", (v * 10.0).ceil() / 10.0, units[u])
    } else {
        format!("{}{}", v.ceil() as u64, units[u])
    }
}

#[cfg(unix)]
fn id_name(file: &str, id: u32) -> String {
    fs::read_to_string(file)
        .ok()
        .and_then(|t| t.lines().find_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?;
            f.next()?;
            (f.next()?.parse::<u32>().ok()? == id).then(|| name.to_string())
        }))
        .unwrap_or_else(|| id.to_string())
}

#[cfg(not(unix))]
fn id_name(_file: &str, id: u32) -> String {
    id.to_string()
}

/// The switches `insert-directory` understands when it makes the listing
/// itself (the rest are ignored, as an `ls` without them would).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LsSwitches {
    pub long: bool,
    pub all: bool,
    pub almost_all: bool,
    pub human: bool,
    pub reverse: bool,
    pub by_time: bool,
    pub by_size: bool,
    pub directory_itself: bool,
    pub one_per_line: bool,
    pub classify: bool,
    pub recursive: bool,
}

impl LsSwitches {
    /// From `"-alh"`, `"-l -a"` or `"--all"` style switches.
    pub fn parse(switches: &[String]) -> LsSwitches {
        let mut s = LsSwitches::default();
        for w in switches.iter().flat_map(|w| w.split_whitespace()) {
            if let Some(long) = w.strip_prefix("--") {
                match long {
                    "all" => s.all = true,
                    "almost-all" => s.almost_all = true,
                    "human-readable" => s.human = true,
                    "reverse" => s.reverse = true,
                    "directory" => s.directory_itself = true,
                    "classify" => s.classify = true,
                    "recursive" => s.recursive = true,
                    _ => {}
                }
            } else if let Some(short) = w.strip_prefix('-') {
                for c in short.chars() {
                    match c {
                        'l' => s.long = true,
                        'a' => s.all = true,
                        'A' => s.almost_all = true,
                        'h' => s.human = true,
                        'r' => s.reverse = true,
                        't' => s.by_time = true,
                        'S' => s.by_size = true,
                        'd' => s.directory_itself = true,
                        '1' => s.one_per_line = true,
                        'F' => s.classify = true,
                        'R' => s.recursive = true,
                        _ => {}
                    }
                }
            }
        }
        s
    }
}

fn ls_line(dir: &str, name: &str, sw: &LsSwitches, now: i64) -> Option<String> {
    let at = if name.is_empty() { dir.to_string() } else if dir.is_empty() { name.to_string() } else { format!("{}{name}", file_name_as_directory(dir)) };
    let shown = if name.is_empty() { at.clone() } else { name.to_string() };
    let a = file_attributes(&at).ok()?;
    let mark = if sw.classify {
        match a.kind {
            Kind::Dir => "/",
            Kind::Link => "@",
            Kind::File if a.mode.contains('x') && !a.mode.starts_with('d') => "*",
            Kind::File => "",
        }
    } else {
        ""
    };
    if !sw.long {
        return Some(format!("{shown}{mark}"));
    }
    let size = if sw.human { human(a.size) } else { a.size.to_string() };
    let target = a.target.as_ref().map(|t| format!(" -> {t}")).unwrap_or_default();
    Some(format!(
        "{} {:>2} {} {} {:>6} {} {shown}{mark}{target}",
        a.mode,
        a.links,
        id_name("/etc/passwd", a.uid),
        id_name("/etc/group", a.gid),
        size,
        ls_date(a.modified, now),
    ))
}

/// What `(insert-directory FILE SWITCHES WILDCARD FULL-DIRECTORY-P)` inserts,
/// made by Forge (so it works where there is no `ls`, as on Windows):
/// FILE's listing -- the entries of a directory when `full_directory`,
/// FILE itself otherwise (or with `-d`) -- or, with `wildcard`, every file the
/// pattern names. Switches: `-l -a -A -h -r -t -S -d -1 -F -R`.
pub fn insert_directory(file: &str, switches: &[String], wildcard: bool, full_directory: bool, now: i64) -> Result<String, String> {
    let sw = LsSwitches::parse(switches);
    let mut out = String::new();
    let sort = |names: &mut Vec<(String, i64, u64)>| {
        if sw.by_size {
            names.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
        } else if sw.by_time {
            names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        } else {
            names.sort_by(|a, b| a.0.cmp(&b.0));
        }
        if sw.reverse {
            names.reverse();
        }
    };
    if wildcard {
        let mut names: Vec<(String, i64, u64)> = file_expand_wildcards(file, false, false, None)?
            .into_iter()
            .map(|n| {
                let a = file_attributes(&n).ok();
                (n, a.as_ref().map_or(0, |a| a.modified), a.map_or(0, |a| a.size))
            })
            .collect();
        sort(&mut names);
        for (n, ..) in names {
            if let Some(l) = ls_line("", &n, &sw, now) {
                out.push_str(&l);
                out.push('\n');
            }
        }
        return Ok(out);
    }
    let p = path_of(file);
    if !p.exists() && fs::symlink_metadata(&p).is_err() {
        return Err(format!("{file}: No such file or directory"));
    }
    if !(full_directory && p.is_dir()) || sw.directory_itself {
        if let Some(l) = ls_line(&expand_file_name(file, None), "", &sw, now) {
            out.push_str(&l);
            out.push('\n');
        }
        return Ok(out);
    }
    fn one(dir: &str, sw: &LsSwitches, now: i64, out: &mut String, header: bool, sort: &dyn Fn(&mut Vec<(String, i64, u64)>)) -> Result<(), String> {
        let mut names: Vec<(String, i64, u64)> = Vec::new();
        for n in names_in(dir, true).map_err(|e| format!("{dir}: {e}"))? {
            if !(sw.all || n.starts_with('.') == false || sw.almost_all && n != "." && n != "..") {
                continue;
            }
            let a = file_attributes(&format!("{}{n}", file_name_as_directory(dir))).ok();
            names.push((n, a.as_ref().map_or(0, |a| a.modified), a.map_or(0, |a| a.size)));
        }
        sort(&mut names);
        if header {
            out.push_str(&format!("{}:\n", directory_file_name(dir)));
        }
        if sw.long {
            let total: u64 = names.iter().filter_map(|(n, ..)| fs::symlink_metadata(path_of(&format!("{}{n}", file_name_as_directory(dir)))).ok()).map(|m| m.len().div_ceil(1024)).sum();
            out.push_str(&format!("total {total}\n"));
        }
        for (n, ..) in &names {
            if let Some(l) = ls_line(dir, n, sw, now) {
                out.push_str(&l);
                out.push('\n');
            }
        }
        if sw.recursive {
            for (n, ..) in &names {
                let sub = format!("{}{n}", file_name_as_directory(dir));
                if n != "." && n != ".." && path_of(&sub).is_dir() && !fs::symlink_metadata(path_of(&sub)).is_ok_and(|m| m.file_type().is_symlink()) {
                    out.push('\n');
                    one(&sub, sw, now, out, true, sort)?;
                }
            }
        }
        Ok(())
    }
    one(&expand_file_name(file, None), &sw, now, &mut out, sw.recursive, &sort)?;
    Ok(out)
}

/// Run an external `ls` (the `insert-directory-program` setting) the way
/// Forge does and return its output.
pub fn insert_directory_with(program: &str, file: &str, switches: &[String], full_directory: bool) -> Result<String, String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(switches.iter().flat_map(|s| s.split_whitespace()));
    if full_directory && !switches.iter().any(|s| s.contains('d')) {
        cmd.arg("--");
    } else {
        cmd.arg("-d").arg("--");
    }
    cmd.arg(expand_file_name(file, None));
    let o = cmd.output().map_err(|e| format!("{program}: {e}"))?;
    if !o.status.success() && o.stdout.is_empty() {
        return Err(String::from_utf8_lossy(&o.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

static PROGRAM: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// `insert-directory-program`: an `ls` to run for listings; empty (the
/// default) means Forge makes them itself.
pub fn set_insert_directory_program(program: &str) {
    *PROGRAM.lock().unwrap_or_else(|e| e.into_inner()) = program.to_string();
}

pub fn insert_directory_program() -> String {
    PROGRAM.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The listing `insert-directory` makes now, by the program setting or by
/// Forge's own.
pub fn directory_listing(file: &str, switches: &[String], wildcard: bool, full_directory: bool) -> Result<String, String> {
    let program = insert_directory_program();
    if !program.is_empty() && !wildcard {
        return insert_directory_with(&program, file, switches, full_directory);
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    insert_directory(file, switches, wildcard, full_directory, now)
}

// ----------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("forge-fileops-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_are_taken_apart() {
        assert_eq!(file_name_directory("/a/b/c.txt").as_deref(), Some("/a/b/"));
        assert_eq!(file_name_directory("c.txt"), None);
        assert_eq!(file_name_nondirectory("/a/b/"), "");
        assert_eq!(file_name_nondirectory("/a/b/c.txt"), "c.txt");
        assert_eq!(file_name_as_directory("/a/b"), "/a/b/");
        assert_eq!(file_name_as_directory(""), "./");
        assert_eq!(directory_file_name("/a/b//"), "/a/b");
        assert_eq!(directory_file_name("/"), "/");
        assert_eq!(file_name_extension("/a/b.tar.gz", false).as_deref(), Some("gz"));
        assert_eq!(file_name_extension("/a/b.tar.gz", true).as_deref(), Some(".gz"));
        assert_eq!(file_name_extension("/a/.hidden", false), None);
        assert_eq!(file_name_extension("/a/foo.", false).as_deref(), Some(""));
        assert_eq!(file_name_sans_extension("/a/b.tar.gz"), "/a/b.tar");
        assert_eq!(file_name_sans_extension("/a.d/noext"), "/a.d/noext");
        assert_eq!(file_name_base("/a/b.tar.gz"), "b.tar");
        assert_eq!(file_name_with_extension("/a/b.txt", ".md").unwrap(), "/a/b.md");
        assert_eq!(file_name_with_extension("/a/b", "md").unwrap(), "/a/b.md");
        assert!(file_name_with_extension("/a/", "md").is_err());
        assert_eq!(file_name_concat(&["/a", "b", "", "c/"]), "/a/b/c/");
        assert_eq!(file_name_split("/a/b/"), ["", "a", "b", ""]);
        assert!(file_name_absolute_p("/x") && file_name_absolute_p("~/x") && !file_name_absolute_p("x"));
        assert_eq!(file_name_parent_directory("/a/b/c").as_deref(), Some("/a/b/"));
        assert_eq!(file_name_parent_directory("/a/b/").as_deref(), Some("/a/"));
        assert_eq!(file_name_parent_directory("/"), None);
    }

    #[test]
    fn names_are_expanded_and_made_relative() {
        assert_eq!(expand_file_name("../x/./y", Some("/a/b")), "/a/x/y");
        assert_eq!(expand_file_name("/a//b/../c/", None), "/a/c/");
        assert_eq!(expand_file_name("/../a", None), "/a");
        assert_eq!(expand_file_name("c", Some("/a/b/")), "/a/b/c");
        assert_eq!(file_relative_name("/a/b/c", Some("/a/b")), "c");
        assert_eq!(file_relative_name("/a/x", Some("/a/b/c")), "../../x");
        assert_eq!(file_relative_name("/a/b", Some("/a/b")), ".");
        if let Some(h) = home() {
            let h = h.trim_end_matches('/').to_string();
            assert_eq!(expand_file_name("~/x", None), format!("{h}/x"));
            assert_eq!(abbreviate_file_name(&format!("{h}/x")), "~/x");
            assert_eq!(abbreviate_file_name(&h), "~");
        }
    }

    #[test]
    fn directory_files_list_sort_match_and_count() {
        let d = scratch("df");
        for n in ["b.txt", "a.org", "c.txt"] {
            fs::write(d.join(n), "").unwrap();
        }
        fs::create_dir(d.join("sub")).unwrap();
        let ds = d.display().to_string();
        assert_eq!(directory_files(&ds, false, None, false, None).unwrap(), [".", "..", "a.org", "b.txt", "c.txt", "sub"]);
        let txt = Regex::new(r"\.txt$").unwrap();
        assert_eq!(directory_files(&ds, false, Some(&txt), false, None).unwrap(), ["b.txt", "c.txt"]);
        assert_eq!(directory_files(&ds, true, Some(&txt), false, Some(1)).unwrap(), [format!("{ds}/b.txt")]);
        let nodot = Regex::new(NO_DOT_FILES_REGEXP).unwrap();
        assert_eq!(directory_files(&ds, false, Some(&nodot), false, None).unwrap().len(), 4);
        assert!(directory_files(&format!("{ds}/missing"), false, None, false, None).is_err());
        let with = directory_files_and_attributes(&ds, false, Some(&txt), false, None).unwrap();
        assert_eq!(with.len(), 2);
        assert_eq!(with[0].1.kind, Kind::File);
        assert!(directory_files_and_attributes(&ds, false, Some(&nodot), false, None).unwrap().iter().any(|(n, a)| n == "sub" && a.kind == Kind::Dir));
        assert!(!directory_empty_p(&ds));
        assert!(directory_empty_p(&d.join("sub").display().to_string()));
        assert!(!directory_empty_p(&d.join("nope").display().to_string()));
        assert!(!directory_empty_p(&d.join("a.org").display().to_string()));
        // The no-dot regexp on an empty directory finds nothing.
        let e = d.join("sub").display().to_string();
        assert!(directory_files(&e, false, Some(&nodot), false, None).unwrap().is_empty());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn recursion_goes_depth_first_and_sorted() {
        let d = scratch("rec");
        fs::create_dir_all(d.join("x/y")).unwrap();
        fs::create_dir_all(d.join("z")).unwrap();
        for f in ["b.org", "a.org", "x/m.org", "x/y/n.org", "x/skip.md", "z/o.org"] {
            fs::write(d.join(f), "").unwrap();
        }
        let ds = d.display().to_string();
        let re = Regex::new(r"\.org$").unwrap();
        let rel = |v: Vec<String>| v.into_iter().map(|p| p[ds.len() + 1..].to_string()).collect::<Vec<_>>();
        assert_eq!(rel(directory_files_recursively(&ds, &re, false, None, false)), ["a.org", "b.org", "x/m.org", "x/y/n.org", "z/o.org"]);
        let any = Regex::new("^[xyz]$").unwrap();
        assert_eq!(rel(directory_files_recursively(&ds, &any, true, None, false)), ["x/y", "x", "z"]);
        let skip_x = |p: &str| !p.ends_with("/x");
        assert_eq!(rel(directory_files_recursively(&ds, &re, false, Some(&skip_x), false)), ["a.org", "b.org", "z/o.org"]);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("z"), d.join("link")).unwrap();
            assert_eq!(directory_files_recursively(&ds, &re, false, None, false).len(), 5);
            assert_eq!(directory_files_recursively(&ds, &re, false, None, true).len(), 6);
        }
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn dominating_files_and_containment() {
        let d = scratch("dom");
        fs::create_dir_all(d.join("proj/src/deep")).unwrap();
        fs::write(d.join("proj/.git"), "").unwrap();
        fs::write(d.join("proj/src/deep/f.rs"), "").unwrap();
        let root = format!("{}/proj/", d.canonicalize().unwrap().display());
        let from_file = d.join("proj/src/deep/f.rs").display().to_string();
        assert_eq!(locate_dominating_name(&from_file, ".git").map(|r| r.replace(&d.display().to_string(), &d.canonicalize().unwrap().display().to_string())), Some(root.clone()));
        assert_eq!(locate_dominating_name(&d.join("proj").display().to_string(), ".git").is_some(), true);
        assert_eq!(locate_dominating_name(&from_file, "no-such-marker-file-xyz"), None);
        let pred = locate_dominating_file(&from_file, &|dir| dir.ends_with("src/"));
        assert!(pred.is_some_and(|p| p.ends_with("proj/src/")));
        let (dir, file) = (d.join("proj").display().to_string(), d.join("proj/src/deep/f.rs").display().to_string());
        assert!(file_in_directory_p(&file, &dir));
        assert!(file_in_directory_p(&dir, &dir));
        assert!(file_in_directory_p(&d.join("proj/src/new.rs").display().to_string(), &dir));
        assert!(!file_in_directory_p(&d.display().to_string(), &dir));
        assert!(!file_in_directory_p(&file, &d.join("missing").display().to_string()));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn wildcards_expand_a_directory_at_a_time() {
        let d = scratch("wild");
        fs::create_dir_all(d.join("a")).unwrap();
        fs::create_dir_all(d.join("b")).unwrap();
        for f in ["a/x.png", "a/y.jpg", "b/z.png", "top.png", "t1.txt", "t2.txt"] {
            fs::write(d.join(f), "").unwrap();
        }
        let ds = d.display().to_string();
        assert_eq!(file_expand_wildcards(&format!("{ds}/*.png"), false, false, None).unwrap(), [format!("{ds}/top.png")]);
        assert_eq!(file_expand_wildcards(&format!("{ds}/*/*.png"), false, false, None).unwrap(), [format!("{ds}/a/x.png"), format!("{ds}/b/z.png")]);
        assert_eq!(file_expand_wildcards("*/*.png", false, false, Some(&ds)).unwrap(), ["a/x.png", "b/z.png"]);
        assert_eq!(file_expand_wildcards("*/*.png", true, false, Some(&ds)).unwrap(), [format!("{ds}/a/x.png"), format!("{ds}/b/z.png")]);
        assert_eq!(file_expand_wildcards("t?.txt", false, false, Some(&ds)).unwrap(), ["t1.txt", "t2.txt"]);
        assert_eq!(file_expand_wildcards("t[1].txt", false, false, Some(&ds)).unwrap(), ["t1.txt"]);
        assert_eq!(file_expand_wildcards("t[!1].txt", false, false, Some(&ds)).unwrap(), ["t2.txt"]);
        assert_eq!(file_expand_wildcards(r"a/.*\.(png|jpg)$", false, true, Some(&ds)).unwrap(), ["a/x.png", "a/y.jpg"]);
        assert_eq!(file_expand_wildcards("top.png", false, false, Some(&ds)).unwrap(), ["top.png"]);
        assert!(file_expand_wildcards("nothing*", false, false, Some(&ds)).unwrap().is_empty());
        assert!(file_expand_wildcards("(", false, true, Some(&ds)).is_err());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn directories_are_made_copied_and_deleted() {
        let d = scratch("ops");
        let ds = d.display().to_string();
        assert!(make_directory(&format!("{ds}/a/b/c"), false).is_err());
        assert_eq!(make_directory(&format!("{ds}/a/b/c"), true).unwrap(), true);
        assert_eq!(make_directory(&format!("{ds}/a/b/c"), true).unwrap(), false);
        assert!(make_directory(&format!("{ds}/a/b/c"), false).is_err());
        make_empty_file(&format!("{ds}/a/b/c/f.txt"), false).unwrap();
        assert!(make_empty_file(&format!("{ds}/a/b/c/f.txt"), false).is_err());
        make_empty_file(&format!("{ds}/new/deep/g.txt"), true).unwrap();
        assert!(d.join("new/deep/g.txt").exists());
        assert!(make_empty_file(&format!("{ds}/nodir/h.txt"), false).is_err());
        fs::write(d.join("a/b/c/f.txt"), "hi").unwrap();

        // Into a directory that exists: as a child.
        fs::create_dir(d.join("dest")).unwrap();
        let wrote = copy_directory(&format!("{ds}/a"), &format!("{ds}/dest"), false, false).unwrap();
        assert_eq!(wrote, format!("{ds}/dest/a"));
        assert_eq!(fs::read_to_string(d.join("dest/a/b/c/f.txt")).unwrap(), "hi");
        // To a new name.
        copy_directory(&format!("{ds}/a"), &format!("{ds}/copy"), false, false).unwrap();
        assert!(d.join("copy/b/c/f.txt").exists());
        // Its contents into an existing one.
        copy_directory(&format!("{ds}/a"), &format!("{ds}/dest"), false, true).unwrap();
        assert!(d.join("dest/b/c/f.txt").exists());
        // Missing parent needs `parents`.
        assert!(copy_directory(&format!("{ds}/a"), &format!("{ds}/p/q/r"), false, false).is_err());
        copy_directory(&format!("{ds}/a"), &format!("{ds}/p/q/r"), true, false).unwrap();
        assert!(d.join("p/q/r/b").is_dir());
        // Not into itself, not a file.
        assert!(copy_directory(&format!("{ds}/a"), &format!("{ds}/a/b/inside"), true, false).is_err());
        assert!(copy_directory(&format!("{ds}/a/b/c/f.txt"), &format!("{ds}/x"), true, false).is_err());

        assert!(delete_directory(&format!("{ds}/a"), false).is_err());
        delete_directory(&format!("{ds}/a"), true).unwrap();
        assert!(!d.join("a").exists());
        delete_directory(&format!("{ds}/p/q/r/b/c"), false).unwrap_err();
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn insert_directory_lists_like_ls() {
        let d = scratch("ls");
        fs::write(d.join("small"), "12345").unwrap();
        fs::write(d.join(".hidden"), "").unwrap();
        fs::create_dir(d.join("dir")).unwrap();
        let ds = d.display().to_string();
        let sw = |s: &str| vec![s.to_string()];
        let plain = insert_directory(&ds, &sw("-1"), false, true, 0).unwrap();
        assert_eq!(plain, "dir\nsmall\n");
        let all = insert_directory(&ds, &sw("-a1"), false, true, 0).unwrap();
        assert_eq!(all, ".\n..\n.hidden\ndir\nsmall\n");
        let almost = insert_directory(&ds, &sw("-A1"), false, true, 0).unwrap();
        assert_eq!(almost, ".hidden\ndir\nsmall\n");
        let rev = insert_directory(&ds, &sw("-1r"), false, true, 0).unwrap();
        assert_eq!(rev, "small\ndir\n");
        let long = insert_directory(&ds, &sw("-l"), false, true, 0).unwrap();
        let lines: Vec<&str> = long.lines().collect();
        assert!(lines[0].starts_with("total "), "{long}");
        assert!(lines[1].starts_with('d') && lines[1].ends_with(" dir"), "{long}");
        assert!(lines[2].starts_with('-') && lines[2].contains(" 5 ") && lines[2].ends_with(" small"), "{long}");
        let class = insert_directory(&ds, &sw("-F1"), false, true, 0).unwrap();
        assert_eq!(class, "dir/\nsmall\n");
        // The directory itself, and a plain file.
        let itself = insert_directory(&ds, &sw("-ld"), false, true, 0).unwrap();
        assert!(itself.starts_with('d') && itself.trim_end().ends_with(&ds), "{itself}");
        let one = insert_directory(&format!("{ds}/small"), &sw("-l"), false, false, 0).unwrap();
        assert!(one.starts_with('-') && one.trim_end().ends_with("small"));
        // Wildcards.
        let wild = insert_directory(&format!("{ds}/s*"), &sw("-1"), true, false, 0).unwrap();
        assert_eq!(wild.trim_end(), format!("{ds}/small"));
        assert!(insert_directory(&format!("{ds}/nope"), &sw("-l"), false, true, 0).is_err());
        let rec = insert_directory(&ds, &sw("-1R"), false, true, 0).unwrap();
        assert!(rec.contains(&format!("{ds}:\n")) && rec.contains(&format!("{ds}/dir:\n")), "{rec}");
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn ls_dates_and_sizes() {
        // 2024-06-01 12:30:00 UTC, seen from the same day and from far later.
        let t = 1_717_245_000;
        assert_eq!(ls_date(t, t), "Jun  1 12:30");
        assert_eq!(ls_date(t, t + 400 * 86_400), "Jun  1  2024");
        assert_eq!(human(512), "512");
        assert_eq!(human(2048), "2.0K");
        assert_eq!(human(5 * 1024 * 1024), "5.0M");
        assert_eq!(human(300 * 1024), "300K");
    }
}
