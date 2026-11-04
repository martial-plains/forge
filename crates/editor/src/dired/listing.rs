// SPDX-License-Identifier: MIT OR Apache-2.0
use super::env::*;
use super::format::*;
use super::render::*;
use super::*;

pub(super) fn make_row(
    path: &Path,
    name: &str,
    icons: bool,
    now: SystemTime,
    git: Option<&GitInfo>,
) -> Row {
    let unknown = || Row {
        links: "?".into(),
        group: "?".into(),
        raw: 0,
        blocks: 0,
        ls_date: "?".into(),
        mtime: 0,
        len: 0,
        inode: "?".into(),
        perms: "?".repeat(PERMS_LEN),
        size: "-".into(),
        user: "?".into(),
        date: "?".into(),
        git: String::new(),
        name: name.into(),
    };
    let Ok(meta) = fs::symlink_metadata(path) else {
        return unknown();
    };
    let perms = mode_string(&meta);
    let kind = perms.chars().next().unwrap_or('.');
    let exec = matches!(perms.chars().nth(3), Some('x' | 's'));
    let size = if meta.is_file() || meta.file_type().is_symlink() {
        human_size(meta.len())
    } else {
        "-".into()
    };
    let date = meta
        .modified()
        .map(|t| format_date(t, now, env().tz))
        .unwrap_or_default();
    let mut shown = String::new();
    if icons {
        shown.push(icon(name, kind, exec));
        shown.push(' ');
    }
    shown.push_str(name);
    if kind == 'l'
        && let Ok(target) = fs::read_link(path)
    {
        shown.push_str(&format!(" -> {}", target.display()));
    }
    let mtime = meta.modified().map(unix_secs).unwrap_or(0);
    let git = git.map(|g| g.letters(path)).unwrap_or_default();
    let len = match meta.is_file() || meta.file_type().is_symlink() {
        true => meta.len(),
        false => 0,
    };
    Row {
        links: links_of(&meta),
        group: group_of(&meta),
        raw: meta.len(),
        blocks: blocks_of(&meta),
        ls_date: meta
            .modified()
            .map(|t| format_ls_date(t, now, env().tz))
            .unwrap_or_default(),
        mtime,
        len,
        inode: inode(&meta),
        perms,
        size,
        user: owner(&meta),
        date,
        git,
        name: shown,
    }
}
/// The columns of the listing whose header is on line `header`: from the
/// column headers under it, or — in the `ls` style, which has "total N"
/// there instead — from its first entry.
pub(super) fn layout_for(b: &Buffer, header: usize) -> Option<Layout> {
    let head = b.line(header + 1);
    if head.trim_start().starts_with("total ") && head.ends_with(HIDDEN_DETAILS) {
        // Only the mode (after the mark) and the name.
        Some(Layout {
            inode: None,
            links: None,
            group: None,
            perms: 2,
            size: None,
            user: None,
            date: None,
            git: None,
            name: 2 + PERMS_LEN + 1,
        })
    } else if head.trim_start().starts_with("total ") {
        Layout::of_ls(&b.line(header + 2))
    } else {
        Layout::of(&head)
    }
}
/// A listing's header line, like "  /home/me/src:" — or with "  [by date,
/// all, tree]" after it for everything that isn't as eza has it.
pub(super) fn parse_header_line(line: &str) -> Option<(PathBuf, View)> {
    if !line.starts_with("  /") {
        return None;
    }
    let line = line.trim();
    let (dir, options) = match line.split_once(OPTIONS) {
        Some((dir, rest)) => (dir, rest.strip_suffix(']')?),
        None => (line.strip_suffix(':')?, ""),
    };
    let mut view = View::default();
    for option in options.split(", ").filter(|o| !o.is_empty()) {
        match option {
            "by date" => view.sort = Sort::Date,
            "by size" => view.sort = Sort::Size,
            "by extension" => view.sort = Sort::Extension,
            "all" => view.all = true,
            "dirs first" => view.dirs_first = true,
            "tree" => view.tree = true,
            "inode" => view.inode = true,
            "scale" => view.scale = true,
            "reverse" => view.reverse = true,
            "git ignore" => view.git_ignore = true,
            "only dirs" => view.only_dirs = true,
            "only files" => view.only_files = true,
            "no icons" => view.icons = false,
            "no details" => view.details = false,
            "omit" => view.omit = true,
            "hidden" => view.hidden = true,
            "ls" => view.ls = true,
            l if l.starts_with("level=") => {
                if let Ok(n) = l["level=".len()..].parse::<usize>() {
                    view.level = n.clamp(1, TREE_MAX_DEPTH);
                }
            }
            _ => {}
        }
    }
    Some((PathBuf::from(dir), view))
}
/// The listings in the buffer, top to bottom. The first is the directory
/// the buffer is for; the rest were inserted with `i`.
pub(super) fn sections(b: &Buffer) -> Vec<Section> {
    (0..b.line_count())
        .filter_map(|header| {
            parse_header_line(&b.line(header)).map(|(dir, _)| Section { header, dir })
        })
        .collect()
}
/// The listing that `line` belongs to.
pub(super) fn section_at(b: &Buffer, line: usize) -> Option<Section> {
    let line = line.min(b.line_count().checked_sub(1)?);
    (0..=line).rev().find_map(|header| {
        parse_header_line(&b.line(header)).map(|(dir, _)| Section { header, dir })
    })
}
pub(super) fn header_dir(b: &Buffer) -> Option<PathBuf> {
    parse_header_line(&b.line(0)).map(|(dir, _)| dir)
}
/// How the buffer is laid out, as its first line says.
pub(super) fn current_view(b: &Buffer) -> View {
    parse_header_line(&b.line(0)).map_or(View::default(), |(_, view)| view)
}
/// The directory the buffer is for.
pub(super) fn current_dir(app: &App) -> Option<PathBuf> {
    let b = app.buf_ref();
    if b.major_mode() == MODE {
        header_dir(b)
    } else {
        None
    }
}
/// The directory of the listing point is in.
pub(super) fn point_dir(app: &App) -> Option<PathBuf> {
    let b = app.buf_ref();
    if b.major_mode() != MODE {
        return None;
    }
    section_at(b, b.point().line).map(|s| s.dir)
}
/// Where the names start in the listing `line` belongs to.
pub(super) fn name_col_at(b: &Buffer, line: usize) -> usize {
    section_at(b, line)
        .and_then(|s| {
            (s.header + 1 < b.line_count())
                .then(|| layout_for(b, s.header).map(|l| l.name))
                .flatten()
        })
        .unwrap_or(0)
}
/// A file from one line of a listing. `ancestors` are the directories the
/// line's tree cells stand for, outermost first, so that a file several
/// levels down keeps its whole path.
pub(super) fn parse_entry(
    dir: &Path,
    layout: &Layout,
    chars: &[char],
    ancestors: &[String],
) -> Option<Entry> {
    let kind = *chars.get(layout.perms)?;
    let mut rest: Vec<char> = chars.iter().skip(layout.name).copied().collect();
    // The tree cells in front of the name aren't part of it.
    for _ in 0..tree_cells(chars, layout.name) / 4 {
        rest.drain(..4);
    }
    if rest.len() >= 2 && is_icon(rest[0]) && rest[1] == ' ' {
        rest.drain(..2);
    }
    let mut name: String = rest.into_iter().collect();
    // A symlink's target is printed after its name.
    if kind == 'l'
        && let Some(i) = name.find(" -> ")
    {
        name.truncate(i);
    }
    if name.is_empty() {
        return None;
    }
    let mut path = dir.to_path_buf();
    for a in ancestors {
        path.push(a);
    }
    path.push(&name);
    Some(Entry { path, name, kind })
}
/// One listing's entries in the order they are printed, each with its line
/// and mark. The tree ancestors are kept as the walk goes, since an entry
/// says only how deep it sits, not which directories it is under.
pub(super) fn section_entries(b: &Buffer, sec: &Section, end: usize) -> Vec<(usize, char, Entry)> {
    let mut out = Vec::new();
    let Some(layout) = layout_for(b, sec.header) else {
        return out;
    };
    let mut stack: Vec<String> = Vec::new();
    for li in sec.header + 2..end {
        let chars: Vec<char> = b.line(li).chars().collect();
        // An entry whose cells number n sits under n - 1 directories.
        stack.truncate((tree_cells(&chars, layout.name) / 4).saturating_sub(1));
        let Some(e) = parse_entry(&sec.dir, &layout, &chars, &stack) else {
            continue;
        };
        if e.kind == 'd' {
            stack.push(e.name.clone());
        }
        out.push((li, chars.first().copied().unwrap_or(' '), e));
    }
    out
}
/// The file on a line.
pub(super) fn entry_on_line(b: &Buffer, line: usize) -> Option<Entry> {
    let sec = section_at(b, line)?;
    if line < sec.header + 2 {
        return None;
    }
    let secs = sections(b);
    let end = secs
        .iter()
        .find(|s| s.header > sec.header)
        .map_or(b.line_count(), |s| s.header);
    section_entries(b, &sec, end)
        .into_iter()
        .find(|(li, _, _)| *li == line)
        .map(|(_, _, e)| e)
}
/// Every file in the buffer with its line and mark, in one pass.
pub(super) fn entries(b: &Buffer) -> Vec<(usize, char, Entry)> {
    let secs = sections(b);
    secs.iter()
        .enumerate()
        .filter(|(_, sec)| sec.header + 1 < b.line_count())
        .flat_map(|(i, sec)| {
            let end = secs.get(i + 1).map_or(b.line_count(), |next| next.header);
            section_entries(b, sec, end)
        })
        .collect()
}
pub(super) fn marks_of(b: &Buffer) -> HashMap<PathBuf, char> {
    entries(b)
        .into_iter()
        .filter(|(_, mark, _)| *mark != ' ')
        .map(|(_, mark, e)| (e.path, mark))
        .collect()
}
/// Move point to `path`'s line in whichever listing has it. False if it
/// isn't listed.
pub(super) fn goto_path(app: &mut App, path: &Path) -> bool {
    let b = app.buf();
    match entries(b).into_iter().find(|(_, _, e)| e.path == path) {
        Some((line, _, _)) => {
            let col = name_col_at(b, line);
            b.set_point(Pos::new(line, col));
            true
        }
        None => false,
    }
}
/// The last line that holds an entry (the text ends in a newline, so the
/// buffer's last line is empty).
pub(super) fn last_entry_line(b: &Buffer) -> usize {
    b.line_count().saturating_sub(2).max(FIRST_ENTRY)
}

// ------------------------------------------------------------- opening
