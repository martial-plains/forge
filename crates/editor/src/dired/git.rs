// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

pub(super) fn git_enabled() -> bool {
    !std::env::var("FORGE_DIRED_GIT")
        .is_ok_and(|v| matches!(v.as_str(), "0" | "false" | "no" | "off"))
}
/// How bad a letter is; a directory shows the worst one inside it.
pub(super) fn git_rank(c: char) -> u8 {
    match c {
        'U' => 6,
        'D' => 5,
        'M' => 4,
        'R' => 3,
        'T' => 2,
        'N' => 1,
        _ => 0,
    }
}
pub(super) fn worse(a: char, b: char) -> char {
    if git_rank(b) > git_rank(a) { b } else { a }
}
/// The letters for a porcelain status pair: (staged, unstaged).
pub(super) fn git_letters(x: char, y: char) -> (char, char) {
    if x == 'U' || y == 'U' || (x, y) == ('A', 'A') || (x, y) == ('D', 'D') {
        return ('U', 'U');
    }
    let staged = match x {
        'M' => 'M',
        'A' => 'N',
        'D' => 'D',
        'R' | 'C' => 'R',
        'T' => 'T',
        _ => '-',
    };
    let unstaged = match y {
        'M' => 'M',
        'D' => 'D',
        '?' | 'A' => 'N',
        'R' | 'C' => 'R',
        'T' => 'T',
        _ => '-',
    };
    (staged, unstaged)
}
/// Read `git status --porcelain=v1 -z` output.
pub(super) fn parse_git_status(root: PathBuf, out: &[u8]) -> GitInfo {
    let mut info = GitInfo {
        root,
        status: HashMap::new(),
        ignored: HashSet::new(),
    };
    let mut records = out.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(rec) = records.next() {
        let rec = String::from_utf8_lossy(rec);
        let mut chars = rec.chars();
        let (Some(x), Some(y)) = (chars.next(), chars.next()) else {
            continue;
        };
        let path = rec.get(3..).unwrap_or("").to_string();
        // A rename or copy is followed by the path it came from.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            records.next();
        }
        if (x, y) == ('!', '!') {
            info.ignored.insert(path.trim_end_matches('/').to_string());
            continue;
        }
        let (staged, unstaged) = git_letters(x, y);
        // The file, and every directory above it.
        let mut prefix = String::new();
        for part in path.trim_end_matches('/').split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let cell = info.status.entry(prefix.clone()).or_insert(('-', '-'));
            *cell = (worse(cell.0, staged), worse(cell.1, unstaged));
        }
    }
    info
}
pub(super) fn git_output(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    out.status.success().then_some(out.stdout)
}
/// The repository `dir` is in, if any.
pub(super) fn git_info(dir: &Path) -> Option<GitInfo> {
    if !git_enabled() {
        return None;
    }
    let root = String::from_utf8(git_output(dir, &["rev-parse", "--show-toplevel"])?).ok()?;
    let root = PathBuf::from(root.trim());
    // `git` writes a plain path; a listing's directory may be verbatim.
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let status = git_output(
        dir,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored",
        ],
    )?;
    Some(parse_git_status(root, &status))
}
