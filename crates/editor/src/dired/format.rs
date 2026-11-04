// SPDX-License-Identifier: MIT OR Apache-2.0
use super::env::*;
use super::*;

/// What a layout toggle just did, for the echo area.
pub(super) fn describe(view: &View, what: &str) -> String {
    match what {
        "sort" => format!("Sorted by {}", view.sort.label()),
        "all" => format!("Dotfiles {}", if view.all { "shown" } else { "hidden" }),
        "dirs first" => format!(
            "Directories {}",
            if view.dirs_first {
                "listed first"
            } else {
                "sorted in with the rest"
            }
        ),
        "tree" => format!("Tree view {}", if view.tree { "on" } else { "off" }),
        "inode" => format!("Inode column {}", if view.inode { "on" } else { "off" }),
        "reverse" => format!("Reverse sort {}", if view.reverse { "on" } else { "off" }),
        "git ignore" => format!(
            "Git-ignored files {}",
            if view.git_ignore { "hidden" } else { "shown" }
        ),
        "only dirs" => format!(
            "Only directories {}",
            if view.only_dirs { "on" } else { "off" }
        ),
        "only files" => format!("Only files {}", if view.only_files { "on" } else { "off" }),
        "icons" => format!("Icons {}", if view.icons { "on" } else { "off" }),
        "details" => format!("Details {}", if view.details { "shown" } else { "hidden" }),
        "omit" => format!("Omit mode {}", if view.omit { "on" } else { "off" }),
        _ => format!(
            "Size and date gradient {}",
            if view.scale { "on" } else { "off" }
        ),
    }
}
/// eza's sizes: decimal units, one decimal below 10 ("1.4k", "88k").
pub(super) fn human_size(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    const UNITS: [&str; 5] = ["k", "M", "G", "T", "P"];
    let (mut v, mut i) = (n as f64 / 1000.0, 0);
    while v >= 999.95 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v < 9.95 {
        format!("{v:.1}{}", UNITS[i])
    } else {
        format!("{v:.0}{}", UNITS[i])
    }
}
/// Year, month, day of a day count since 1970-01-01 (Hinnant's algorithm).
pub(super) fn civil(days: i64) -> (i64, usize, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m as usize, d)
}
pub(super) fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}
/// eza's date: "4 Oct 21:46" for the last six months, else "4 Oct  2024".
pub(super) fn format_date(mtime: SystemTime, now: SystemTime, tz: i64) -> String {
    let utc = unix_secs(mtime);
    let local = utc + tz;
    let (year, month, day) = civil(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    let age = unix_secs(now) - utc;
    let mon = MONTHS[month - 1];
    if (0..15_778_476).contains(&age) {
        format!("{day:>2} {mon} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
    } else {
        format!("{day:>2} {mon}  {year}")
    }
}
#[cfg(unix)]
pub(super) fn mode_string(m: &fs::Metadata) -> String {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    let ft = m.file_type();
    let kind = if ft.is_dir() {
        'd'
    } else if ft.is_symlink() {
        'l'
    } else if ft.is_block_device() {
        'b'
    } else if ft.is_char_device() {
        'c'
    } else if ft.is_fifo() {
        'p'
    } else if ft.is_socket() {
        's'
    } else {
        '.'
    };
    let mode = m.permissions().mode();
    let bit = |b: u32, c: char| if mode & b != 0 { c } else { '-' };
    let exec =
        |x: u32, special: u32, on: char, off: char| match (mode & x != 0, mode & special != 0) {
            (true, true) => on,
            (false, true) => off,
            (true, false) => 'x',
            (false, false) => '-',
        };
    [
        kind,
        bit(0o400, 'r'),
        bit(0o200, 'w'),
        exec(0o100, 0o4000, 's', 'S'),
        bit(0o040, 'r'),
        bit(0o020, 'w'),
        exec(0o010, 0o2000, 's', 'S'),
        bit(0o004, 'r'),
        bit(0o002, 'w'),
        exec(0o001, 0o1000, 't', 'T'),
    ]
    .iter()
    .collect()
}
#[cfg(not(unix))]
pub(super) fn mode_string(m: &fs::Metadata) -> String {
    let kind = if m.is_dir() {
        'd'
    } else if m.file_type().is_symlink() {
        'l'
    } else {
        '.'
    };
    let w = if m.permissions().readonly() { '-' } else { 'w' };
    format!("{kind}r{w}xr{w}xr{w}x")
}
/// `ls -l`'s date: "Oct 12 14:13" for the last six months, else
/// "Oct 12  2023".
pub(super) fn format_ls_date(mtime: SystemTime, now: SystemTime, tz: i64) -> String {
    let utc = unix_secs(mtime);
    let local = utc + tz;
    let (year, month, day) = civil(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    let age = unix_secs(now) - utc;
    let mon = MONTHS[month - 1];
    if (0..15_778_476).contains(&age) {
        format!("{mon} {day:>2} {:02}:{:02}", secs / 3600, secs % 3600 / 60)
    } else {
        format!("{mon} {day:>2}  {year}")
    }
}
#[cfg(unix)]
pub(super) fn links_of(m: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    m.nlink().to_string()
}
#[cfg(not(unix))]
pub(super) fn links_of(_: &fs::Metadata) -> String {
    "1".into()
}
#[cfg(unix)]
pub(super) fn group_of(m: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    env()
        .groups
        .get(&m.gid())
        .cloned()
        .unwrap_or_else(|| m.gid().to_string())
}
#[cfg(not(unix))]
pub(super) fn group_of(_: &fs::Metadata) -> String {
    String::new()
}
/// Disk blocks in 1K units, for the "total" line.
#[cfg(unix)]
pub(super) fn blocks_of(m: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.blocks().div_ceil(2)
}
#[cfg(not(unix))]
pub(super) fn blocks_of(m: &fs::Metadata) -> u64 {
    m.len().div_ceil(1024)
}
#[cfg(unix)]
pub(super) fn owner(m: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    env()
        .users
        .get(&m.uid())
        .cloned()
        .unwrap_or_else(|| m.uid().to_string())
}
#[cfg(not(unix))]
pub(super) fn owner(_: &fs::Metadata) -> String {
    String::new()
}
pub(super) fn is_icon(c: char) -> bool {
    ('\u{e000}'..='\u{f8ff}').contains(&c)
}
/// A Nerd Font icon, picked like eza's: by kind, then name, then extension.
pub(super) fn icon(name: &str, kind: char, exec: bool) -> char {
    match (kind, name) {
        ('d', ".git") => return '\u{e5fb}',
        ('d', _) => return '\u{f115}',
        ('l', _) => return '\u{f481}',
        (_, ".gitignore" | ".gitattributes" | ".gitmodules") => return '\u{e702}',
        (_, "Makefile") => return '\u{e779}',
        (_, "LICENSE") => return '\u{e60a}',
        _ => {}
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => '\u{e7a8}',
        "py" => '\u{e606}',
        "js" | "mjs" | "cjs" => '\u{e74e}',
        "ts" => '\u{e628}',
        "json" => '\u{e60b}',
        "toml" => '\u{e6b2}',
        "yaml" | "yml" => '\u{e6a8}',
        "md" | "markdown" => '\u{e609}',
        "org" => '\u{e633}',
        "lua" => '\u{e620}',
        "c" | "h" => '\u{e61e}',
        "cpp" | "cc" | "hpp" => '\u{e61d}',
        "go" => '\u{e627}',
        "swift" => '\u{e755}',
        "sh" | "bash" | "zsh" => '\u{f489}',
        "html" => '\u{e736}',
        "css" => '\u{e749}',
        "txt" => '\u{f15c}',
        "lock" => '\u{f023}',
        "zip" | "gz" | "tar" | "xz" | "bz2" => '\u{f410}',
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => '\u{f1c5}',
        "pdf" => '\u{f1c1}',
        _ if exec => '\u{f489}',
        _ => '\u{f016}',
    }
}

// ----------------------------------------------------------- file types
/// What a file is by its name, like eza's `FileType`. eza decides this from
/// the name alone, since that is all that can be known without reading the
/// file, and shades the name accordingly.
///
/// eza also counts a file as compiled when it sits beside the source it was
/// built from — `styles.css` next to `styles.less`. That never happens in
/// practice, because `.css` and `.js` are in the extension table already and
/// this is checked first, so there is nothing to carry here for it.
pub(super) fn file_type(name: &str) -> Option<DiredFace> {
    // A case-insensitive readme, first of all, as eza has it.
    if name.to_lowercase().starts_with("readme") {
        return Some(DiredFace::Build);
    }
    if let Some(face) = by_name(name) {
        return Some(face);
    }
    if let Some(face) = extension(name) {
        return Some(face);
    }
    // The editor's and the archiver's leftovers.
    if name.ends_with('~') || (name.starts_with('#') && name.ends_with('#')) {
        Some(DiredFace::Temp)
    } else {
        None
    }
}
/// Files that kick off a build, matched on the whole name.
#[rustfmt::skip]
pub(super) fn by_name(name: &str) -> Option<DiredFace> {
    Some(match name {
        "Brewfile" | "bsconfig.json" | "BUILD" | "BUILD.bazel" | "build.gradle"
        | "build.sbt" | "build.xml" | "Cargo.toml" | "CMakeLists.txt"
        | "composer.json" | "configure" | "Containerfile" | "Dockerfile"
        | "Earthfile" | "flake.nix" | "Gemfile" | "GNUmakefile" | "Gruntfile.coffee"
        | "Gruntfile.js" | "jsconfig.json" | "Justfile" | "justfile" | "Makefile"
        | "makefile" | "meson.build" | "mix.exs" | "package.json" | "Pipfile"
        | "PKGBUILD" | "Podfile" | "pom.xml" | "Procfile" | "pyproject.toml"
        | "Rakefile" | "RoboFile.php" | "SConstruct" | "tsconfig.json"
        | "Vagrantfile" | "webpack.config.cjs" | "webpack.config.js" | "WORKSPACE" => {
            DiredFace::Build
        }
        "id_dsa" | "id_ecdsa" | "id_ecdsa_sk" | "id_ed25519" | "id_ed25519_sk" | "id_rsa" => {
            DiredFace::Crypto
        }
        _ => return None,
    })
}
/// Files that kick off a build, matched on their extension.
#[rustfmt::skip]
pub(super) fn by_extension(ext: &str) -> Option<DiredFace> {
    Some(match ext {
        "ninja" => DiredFace::Build,
        // Images.
        "arw" | "avif" | "bmp" | "cbr" | "cbz" | "cr2" | "dvi" | "eps" | "fodg" | "gif"
        | "heic" | "heif" | "ico" | "j2c" | "j2k" | "jfi" | "jfif" | "jif" | "jp2" | "jpe"
        | "jpeg" | "jpf" | "jpg" | "jpx" | "jxl" | "kra" | "krz" | "nef" | "odg" | "orf"
        | "pbm" | "pgm" | "png" | "pnm" | "ppm" | "ps" | "psd" | "pxm" | "qoi" | "raw"
        | "svg" | "tif" | "tiff" | "webp" | "xcf" | "xpm" => DiredFace::Image,
        // Videos.
        "avi" | "flv" | "h264" | "heics" | "m2ts" | "m2v" | "m4v" | "mkv" | "mov" | "mp4"
        | "mpeg" | "mpg" | "ogm" | "ogv" | "video" | "vob" | "webm" | "wmv" => DiredFace::Video,
        // Music.
        "aac" | "m4a" | "mka" | "mp2" | "mp3" | "ogg" | "opus" | "wma" => DiredFace::Music,
        // Lossless music, rather than any other kind of data.
        "aif" | "aifc" | "aiff" | "alac" | "ape" | "flac" | "pcm" | "wav" | "wv" => {
            DiredFace::Lossless
        }
        // Cryptology.
        "age" | "asc" | "cer" | "crt" | "csr" | "gpg" | "kbx" | "md5" | "p12" | "pem"
        | "pfx" | "pgp" | "pub" | "sha1" | "sha224" | "sha256" | "sha384" | "sha512"
        | "sig" | "signature" => DiredFace::Crypto,
        // Documents.
        "djvu" | "doc" | "docx" | "eml" | "fodp" | "fods" | "fodt" | "fotd" | "gdoc"
        | "key" | "keynote" | "numbers" | "odp" | "ods" | "odt" | "pages" | "pdf" | "ppt"
        | "pptx" | "rtf" | "xls" | "xlsm" | "xlsx" => DiredFace::Document,
        // Archives.
        "7z" | "ar" | "arj" | "br" | "bz" | "bz2" | "bz3" | "cpio" | "deb" | "dmg" | "gz"
        | "iso" | "lz" | "lz4" | "lzh" | "lzma" | "lzo" | "phar" | "qcow" | "qcow2" | "rar"
        | "rpm" | "tar" | "taz" | "tbz" | "tbz2" | "tc" | "tgz" | "tlz" | "txz" | "tz"
        | "xz" | "vdi" | "vhd" | "vhdx" | "vmdk" | "z" | "zip" | "zst" => DiredFace::Compressed,
        // Temporary files.
        "bak" | "bk" | "bkp" | "crdownload" | "download" | "fcbak" | "fcstd1"
        | "fdmdownload" | "part" | "swn" | "swo" | "swp" | "tmp" => DiredFace::Temp,
        // Compiler output.
        "a" | "bundle" | "class" | "cma" | "cmi" | "cmo" | "cmx" | "dll" | "dylib" | "elc"
        | "elf" | "ko" | "lib" | "o" | "obj" | "pyc" | "pyd" | "pyo" | "so" | "zwc" => {
            DiredFace::Compiled
        }
        // Source code.
        "applescript" | "as" | "asa" | "awk" | "c" | "c++" | "c++m" | "cabal" | "cc" | "ccm"
        | "clj" | "cp" | "cpp" | "cppm" | "cr" | "cs" | "css" | "csx" | "cu" | "cxx"
        | "cxxm" | "cypher" | "d" | "dart" | "di" | "dpr" | "el" | "elm" | "erl" | "ex"
        | "exs" | "f" | "f90" | "fcmacro" | "fcscript" | "fnl" | "for" | "fs" | "fsh" | "fsi"
        | "fsx" | "gd" | "go" | "gradle" | "groovy" | "gvy" | "h" | "h++" | "hh" | "hpp"
        | "hc" | "hs" | "htc" | "hxx" | "inc" | "inl" | "ino" | "ipynb" | "ixx" | "java"
        | "jl" | "js" | "jsx" | "kt" | "kts" | "kusto" | "less" | "lhs" | "lisp" | "ltx"
        | "lua" | "m" | "malloy" | "matlab" | "ml" | "mli" | "mn" | "nb" | "p" | "pas"
        | "php" | "pl" | "pm" | "pod" | "pp" | "prql" | "ps1" | "psd1" | "psm1" | "purs"
        | "py" | "r" | "rb" | "rs" | "rq" | "sass" | "scala" | "scm" | "scad" | "scss"
        | "sld" | "sql" | "ss" | "swift" | "tcl" | "tex" | "ts" | "v" | "vb" | "vsh"
        | "zig" => DiredFace::Source,
        _ => return None,
    })
}
/// The lowercase extension of `name`, if it has one that isn't the whole name.
pub(super) fn extension(name: &str) -> Option<DiredFace> {
    let (_, ext) = name.rsplit_once('.')?;
    by_extension(&ext.to_lowercase())
}

// ------------------------------------------------------------------ git
#[cfg(unix)]
pub(super) fn inode(m: &fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    m.ino().to_string()
}
#[cfg(not(unix))]
pub(super) fn inode(_: &fs::Metadata) -> String {
    String::new()
}
/// Sort like eza: ignoring case and leading dots.
pub(super) fn sort_key(name: &str) -> (String, String) {
    (
        name.trim_start_matches('.').to_lowercase(),
        name.to_string(),
    )
}

// ---------------------------------------------------------------- a tree
/// Put a directory's lines in the buffer's order, eza's way: by name, date,
/// size or extension, reversed when asked, with the directories pulled to
/// one end when asked (they stay first even when reversed).
pub(super) fn sort_lines(lines: &mut [Line], view: View) {
    match view.sort {
        Sort::Name => lines.sort_by_key(|l| sort_key(&l.plain)),
        Sort::Date => lines.sort_by_key(|l| (std::cmp::Reverse(l.row.mtime), sort_key(&l.plain))),
        Sort::Size => lines.sort_by_key(|l| (std::cmp::Reverse(l.row.len), sort_key(&l.plain))),
        Sort::Extension => lines.sort_by_key(|l| (extension_key(&l.plain), sort_key(&l.plain))),
    }
    if view.reverse {
        lines.reverse();
    }
    if view.dirs_first {
        lines.sort_by_key(|l| !l.row.perms.starts_with('d'));
    }
}
/// eza's `--sort=extension`: the lowercase extension, or empty when there
/// is none.
pub(super) fn extension_key(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}
/// The bytes a size cell stands for. [`human_size`] rounds to one decimal,
/// which is close enough: the gradient only compares sizes with each other.
pub(super) fn bytes_in(cell: &str) -> Option<u64> {
    let (digits, scale) = match cell.as_bytes().last()? {
        b'k' => (&cell[..cell.len() - 1], 1e3),
        b'M' => (&cell[..cell.len() - 1], 1e6),
        b'G' => (&cell[..cell.len() - 1], 1e9),
        b'T' => (&cell[..cell.len() - 1], 1e12),
        b'P' => (&cell[..cell.len() - 1], 1e15),
        _ => (cell, 1.0),
    };
    digits.parse::<f64>().ok().map(|v| (v * scale) as u64)
}
/// Day count since 1970-01-01 of a date (Hinnant's algorithm, the other way
/// round from [`civil`]).
pub(super) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}
/// The seconds a date cell stands for, read back the way [`format_date`]
/// wrote it. A cell with a time of day carries no year, so it gets this
/// one — or last year's, when that would otherwise put it in the future.
pub(super) fn seconds_in(cell: &str, now: i64, tz: i64) -> Option<i64> {
    let mut it = cell.split_whitespace();
    let first = it.next()?;
    // eza writes "4 Oct 21:46"; ls writes "Oct  4 21:46".
    let (day, shown): (i64, &str) = match first.parse::<i64>() {
        Ok(day) => (day, it.next()?),
        Err(_) => (it.next()?.parse().ok()?, first),
    };
    let month = MONTHS.iter().position(|m| *m == shown)? as i64 + 1;
    // What is left is a time of day, a year, both or neither.
    let (year, hour, minute) = match it.next() {
        None => (civil(now.div_euclid(86_400)).0, 0, 0),
        Some(rest) => match rest.split_once(':') {
            Some((h, m)) => (
                civil(now.div_euclid(86_400)).0,
                h.parse().ok()?,
                m.parse().ok()?,
            ),
            None => (rest.parse().ok()?, 0, 0),
        },
    };
    let at = |y: i64| days_from_civil(y, month, day) * 86_400 + hour * 3600 + minute * 60 - tz;
    // Pick the most recent reading of this date that isn't in the future.
    [at(year), at(year - 1)]
        .into_iter()
        .filter(|t| *t <= now)
        .max()
}
