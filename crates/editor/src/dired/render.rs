// SPDX-License-Identifier: MIT OR Apache-2.0
use super::env::*;
use super::format::*;
use super::git::*;
use super::listing::*;
use super::*;

/// One directory's listing: its header, column headers and entries. In tree
/// view the same thing, but recursing into every subdirectory.
pub(super) fn render_section(
    dir: &Path,
    marks: &HashMap<PathBuf, char>,
    view: View,
) -> io::Result<String> {
    if rfs::is_remote(dir) {
        return rfs::render(dir, marks, view);
    }
    let icons = icons_enabled() && view.icons;
    let now = SystemTime::now();
    let git = git_info(dir);
    let mut lines = Vec::new();
    Listing {
        icons,
        now,
        git: git.as_ref(),
        view,
    }
    .read(dir, &mut Trunk::default(), 0, &mut lines);
    if view.ls {
        return Ok(render_ls(dir, marks, view, &lines));
    }
    let w = Widths::of(&lines);
    let inode_head = match view.inode {
        true => format!("{:>w$} ", "Inode", w = w.inode),
        false => String::new(),
    };
    // The Git column only exists inside a repository.
    let git_head = match git.is_some() {
        true => "Git ",
        false => "",
    };
    let mut text = format!("  {}{}\n", dir.display(), view.suffix());
    if !view.details {
        // Only the type and the owner's bits: enough to tell what an entry
        // is, which the colours and the parser need.
        let git_head = if git.is_some() { "Git " } else { "" };
        text.push_str(&format!("  {inode_head}Mode {git_head}Name\n"));
        if !view.hidden {
            for line in &lines {
                let tree: String = line.branches.iter().flat_map(|b| b.art().chars()).collect();
                let mark = marks.get(&line.path).copied().unwrap_or(' ');
                let inode_cell = match view.inode {
                    true => format!("{:>w$} ", line.row.inode, w = w.inode),
                    false => String::new(),
                };
                let git_cell = match git.is_some() {
                    true => format!("{:<3} ", line.row.git),
                    false => String::new(),
                };
                let mode: String = line.row.perms.chars().take(4).collect();
                text.push_str(&format!(
                    "{mark} {inode_cell}{mode:<4} {git_cell}{tree}{}\n",
                    line.row.name
                ));
            }
        }
        return Ok(text);
    }
    text.push_str(&format!(
        "  {inode_head}{:<PERMS_W$} {:>size_w$} {:<user_w$} {:<DATE_W$} {git_head}Name\n",
        "Permissions",
        "Size",
        "User",
        "Date Modified",
        size_w = w.size,
        user_w = w.user
    ));
    for line in lines.iter().filter(|_| !view.hidden) {
        let tree: String = line.branches.iter().flat_map(|b| b.art().chars()).collect();
        let mark = marks.get(&line.path).copied().unwrap_or(' ');
        let inode_cell = match view.inode {
            true => format!("{:>w$} ", line.row.inode, w = w.inode),
            false => String::new(),
        };
        let git_cell = match git.is_some() {
            true => format!("{:<3} ", line.row.git),
            false => String::new(),
        };
        text.push_str(&format!(
            "{mark} {inode_cell}{:<PERMS_W$} {:>size_w$} {:<user_w$} {:<DATE_W$} {git_cell}{tree}{}\n",
            line.row.perms, line.row.size, line.row.user, line.row.date, line.row.name,
            size_w = w.size, user_w = w.user
        ));
    }
    Ok(text)
}
/// A listing as `ls -l` writes it, which is how Dired shows one:
/// "total N" under the directory, then a line per file with its mode,
/// links, owner, group, size in bytes, date and name.
pub(super) fn render_ls(
    dir: &Path,
    marks: &HashMap<PathBuf, char>,
    view: View,
    lines: &[Line],
) -> String {
    let widest = |f: fn(&Row) -> String| {
        lines
            .iter()
            .map(|l| f(&l.row).chars().count())
            .max()
            .unwrap_or(0)
    };
    let (lw, uw, gw, sw) = (
        widest(|r| r.links.clone()),
        widest(|r| r.user.clone()),
        widest(|r| r.group.clone()),
        widest(|r| r.raw.to_string()),
    );
    let total: u64 = lines.iter().map(|l| l.row.blocks).sum();
    let hide = if view.details { "" } else { HIDDEN_DETAILS };
    let mut text = format!(
        "  {}{}\n  total {total}{hide}\n",
        dir.display(),
        view.suffix()
    );
    if view.hidden {
        return text;
    }
    for line in lines {
        let mark = marks.get(&line.path).copied().unwrap_or(' ');
        let mut perms = line.row.perms.clone();
        // A plain file is `-` in ls (eza writes a dot).
        if perms.starts_with('.') {
            perms.replace_range(..1, "-");
        }
        // `dired-hide-details-mode`: the mode and the name, which is all
        // that colours and commands need.
        if !view.details {
            text.push_str(&format!("{mark} {perms:<PERMS_LEN$} {}\n", line.row.name));
            continue;
        }
        text.push_str(&format!(
            "{mark} {perms:<PERMS_LEN$} {:>lw$} {:<uw$} {:<gw$} {:>sw$} {:<12} {}\n",
            line.row.links,
            line.row.user,
            line.row.group,
            line.row.raw,
            line.row.ls_date,
            line.row.name,
        ));
    }
    text
}
/// `dired-omit-files` and `dired-omit-extensions`: backups, auto-saves and
/// the usual build leftovers.
pub(super) fn omitted(name: &str) -> bool {
    name.ends_with('~')
        || (name.starts_with('#') && name.ends_with('#'))
        || name.starts_with(".#")
        || [
            ".bak", ".o", ".pyc", ".class", ".swp", ".elc", ".orig", ".rej",
        ]
        .iter()
        .any(|e| name.ends_with(e))
}
/// The names in one directory, with the dotfiles dropped unless the view
/// asks to see them.
pub(super) fn names_of(dir: &Path, view: View) -> io::Result<Vec<String>> {
    let names: Vec<String> = fs::read_dir(dir)?
        .map(|e| Ok(e?.file_name().to_string_lossy().replace('\n', "?")))
        .collect::<io::Result<_>>()?;
    let names: Vec<String> = match view.all {
        true => names,
        false => names.into_iter().filter(|n| !n.starts_with('.')).collect(),
    };
    Ok(match view.omit {
        false => names,
        true => names.into_iter().filter(|n| !omitted(n)).collect(),
    })
}
/// The text of a Dired buffer: the directory, then any subdirectories
/// listed inline (a subdirectory that can't be read any more is dropped).
/// A blank line separates the listings. Directories in `trees` are drawn
/// as eza trees (`i`); everything else uses `view` as is (`T` draws all).
pub(super) fn render_all(
    dirs: &[PathBuf],
    marks: &HashMap<PathBuf, char>,
    view: View,
    trees: &HashSet<PathBuf>,
) -> io::Result<String> {
    render_all_with(dirs, marks, view, trees, &HashSet::new())
}
/// [`render_all`], with the inline listings in `hidden` drawn without
/// entries (`$`).
pub(super) fn render_all_with(
    dirs: &[PathBuf],
    marks: &HashMap<PathBuf, char>,
    view: View,
    trees: &HashSet<PathBuf>,
    hidden: &HashSet<PathBuf>,
) -> io::Result<String> {
    let mut text = String::new();
    for (i, dir) in dirs.iter().enumerate() {
        let section_view = match i == 0 {
            true => view,
            false if trees.contains(dir) => View {
                tree: true,
                hidden: hidden.contains(dir),
                ..view
            },
            false => View {
                tree: false,
                hidden: hidden.contains(dir),
                ..view
            },
        };
        match render_section(dir, marks, section_view) {
            Ok(section) => {
                if i > 0 {
                    text.push('\n');
                }
                text.push_str(&section);
            }
            Err(e) if i == 0 => return Err(e),
            Err(_) => {}
        }
    }
    Ok(text)
}
/// Inline subdirectories whose entries are hidden (`$`).
pub(super) fn hidden_dirs_of(b: &Buffer) -> Vec<PathBuf> {
    (0..b.line_count())
        .filter_map(|l| parse_header_line(&b.line(l)))
        .filter(|(_, v)| v.hidden)
        .map(|(d, _)| d)
        .collect()
}
/// Inline subdirectories drawn as trees (`i`): the sections below the top
/// whose header asks for a tree.
pub(super) fn tree_dirs_of(b: &Buffer) -> Vec<PathBuf> {
    sections(b)
        .into_iter()
        .skip(1)
        .filter_map(|s| {
            let line = match s.header < b.line_count() {
                true => b.line(s.header),
                false => return None,
            };
            match parse_header_line(&line) {
                Some((_, v)) if v.tree => Some(s.dir),
                _ => None,
            }
        })
        .collect()
}

// ------------------------------------------------------------- the shades
/// Add one face over `start..end`, merging it into the run before when
/// they have the same face and meet, so a listing's decorations come out
/// as runs rather than one per character. A run that carries a keymap is
/// left alone: the two aren't the same thing.
pub(super) fn push_face(items: &mut Vec<DecoSpec>, start: usize, end: usize, face: DiredFace) {
    if start >= end {
        return;
    }
    let face = DecoFace::Dired(face);
    match items.last_mut() {
        Some(last) if last.end == start && last.face == face && last.keymap.is_none() => {
            last.end = end
        }
        _ => items.push(DecoSpec::new(start, end, face)),
    }
}
/// Give `start..end` a region keymap. It is a decoration of its own: the
/// face a name has is about what the file is, the keymap about what the
/// name does.
pub(super) fn push_region(items: &mut Vec<DecoSpec>, start: usize, end: usize, region: &str) {
    if start >= end {
        return;
    }
    items.push(DecoSpec::region(start, end, region));
}
/// Color the buffer from its own text.
pub(super) fn decorate(b: &mut Buffer) {
    let mut items = Vec::new();
    let secs = sections(b);
    let now = unix_secs(SystemTime::now());
    for (si, sec) in secs.iter().enumerate() {
        let end = secs.get(si + 1).map_or(b.line_count(), |next| next.header);
        let base = b.pos_to_offset(Pos::new(sec.header, 0));
        push_face(
            &mut items,
            base,
            base + b.line_len(sec.header),
            DiredFace::Header,
        );
        push_region(
            &mut items,
            base,
            base + b.line_len(sec.header),
            SECTION_REGION,
        );
        if sec.header + 1 >= b.line_count() {
            continue;
        }
        let base = b.pos_to_offset(Pos::new(sec.header + 1, 0));
        let total_line = b.line(sec.header + 1).trim_start().starts_with("total ");
        if total_line {
            // "total N" is plain text.
            push_face(
                &mut items,
                base,
                base + b.line_len(sec.header + 1),
                DiredFace::Dim,
            );
        } else {
            push_face(
                &mut items,
                base,
                base + b.line_len(sec.header + 1),
                DiredFace::ColumnHeader,
            );
            push_region(
                &mut items,
                base,
                base + b.line_len(sec.header + 1),
                COLUMNS_REGION,
            );
        }
        let Some(layout) = layout_for(b, sec.header) else {
            continue;
        };
        let ctx = Context::of(b, sec, end, layout, now);
        for li in sec.header + 2..end {
            decorate_entry(b, &mut items, li, &ctx);
        }
    }
    b.set_decorations(OWNER, items);
}
/// The tree cells in front of a name, in characters: four per level, and
/// only in a listing drawn as a tree.
pub(super) fn tree_cells(chars: &[char], from: usize) -> usize {
    let mut cells = 0;
    while from + (cells + 1) * 4 <= chars.len()
        && Branch::parse(
            &chars[from + cells * 4..from + (cells + 1) * 4]
                .iter()
                .collect::<String>(),
        )
        .is_some()
    {
        cells += 1;
    }
    cells * 4
}
/// One entry's faces.
pub(super) fn decorate_entry(b: &Buffer, items: &mut Vec<DecoSpec>, li: usize, ctx: &Context) {
    let layout = ctx.layout;
    let chars: Vec<char> = b.line(li).chars().collect();
    if chars.len() < layout.name {
        return;
    }
    let base = b.pos_to_offset(Pos::new(li, 0));
    let at = |c: usize| base + c;
    let starts = &ctx.starts;
    let marked = match chars.get(MARK_COL) {
        Some('*') => Some(DiredFace::Marked),
        Some('D') => Some(DiredFace::Flagged),
        _ => None,
    };
    if let Some(f) = marked {
        push_face(items, at(MARK_COL), at(MARK_COL + 1), f);
    }
    for i in 0..PERMS_LEN {
        let face = match chars.get(layout.perms + i) {
            Some('r') => DiredFace::PermRead,
            Some('w') => DiredFace::PermWrite,
            Some('x' | 's' | 'S' | 't' | 'T') => DiredFace::PermExec,
            Some('d') => DiredFace::PermDir,
            Some('l') => DiredFace::PermLink,
            _ => DiredFace::Dim,
        };
        push_face(items, at(layout.perms + i), at(layout.perms + i + 1), face);
    }
    let cell = |from: usize| {
        let to = layout.end_of(from, starts).min(chars.len());
        let seg = chars.get(from..to)?;
        let lead = seg.iter().take_while(|c| **c == ' ').count();
        let used = seg.iter().rposition(|c| *c != ' ')? + 1;
        (lead < used).then(|| {
            (
                from + lead,
                from + used,
                seg[lead..used].iter().collect::<String>(),
            )
        })
    };
    if let Some((a, z, text)) = layout.size.and_then(cell) {
        let face = bytes_in(&text).map_or(DiredFace::Dim, |bytes| ctx.size_face(bytes));
        push_face(items, at(a), at(z), face);
    }
    if let Some((a, z, _)) = layout.user.and_then(cell) {
        push_face(items, at(a), at(z), DiredFace::User);
    }
    if let Some((a, z, _)) = layout.links.and_then(cell) {
        push_face(items, at(a), at(z), DiredFace::Size);
    }
    if let Some((a, z, _)) = layout.group.and_then(cell) {
        push_face(items, at(a), at(z), DiredFace::Group);
    }
    if let Some((a, z, text)) = layout.date.and_then(cell) {
        push_face(items, at(a), at(z), ctx.date_face(&text));
    }
    if let Some((a, z, _)) = layout.inode.and_then(cell) {
        push_face(items, at(a), at(z), DiredFace::Inode);
    }
    // Git: a letter each for staged and unstaged changes.
    if let Some(g) = layout.git {
        for i in 0..2 {
            let face = match chars.get(g + i) {
                Some('N') => DiredFace::GitNew,
                Some('M') => DiredFace::GitModified,
                Some('D') => DiredFace::GitDeleted,
                Some('R') => DiredFace::GitRenamed,
                Some('T') => DiredFace::GitTypeChange,
                Some('U') => DiredFace::GitConflicted,
                Some('I') => DiredFace::GitIgnored,
                _ => DiredFace::Dim,
            };
            push_face(items, at(g + i), at(g + i + 1), face);
        }
    }
    // The name, colored by what it is (a mark wins). In a tree the cells
    // in front of it are not part of it.
    let name_col = layout.name + tree_cells(&chars, layout.name);
    let kind = chars.get(layout.perms).copied().unwrap_or('.');
    let exec = matches!(chars.get(layout.perms + 3), Some('x' | 's'));
    let shown: String = chars[name_col..].iter().collect();
    // An icon may sit in front of the name and a symlink's target behind it;
    // neither is part of the name.
    let icon = shown.chars().count()
        - shown
            .trim_start_matches(|c: char| is_icon(c) || c == ' ')
            .chars()
            .count();
    let rest: String = shown.chars().skip(icon).collect();
    let arrow = rest.find(" -> ").filter(|_| kind == 'l');
    let plain = &rest[..arrow.unwrap_or(rest.len())];
    let start = name_col + icon;
    let end = start + plain.chars().count();
    let name_face = marked.or(match kind {
        'd' => Some(DiredFace::Directory),
        'l' => Some(DiredFace::Symlink),
        _ if exec => Some(DiredFace::Executable),
        _ => file_type(plain),
    });
    if let Some(f) = name_face {
        push_face(items, at(start), at(end), f);
    }
    // The name is the entry's link: a click on it visits the file, and
    // the region map is where a user can bind that key to something else.
    // The region's left edge is the name column rather than the name
    // itself, so a click on a Nerd Font icon in front of it counts too;
    // the tree cells in front of that belong to the directories above.
    push_region(items, at(name_col), at(end), ENTRY_REGION);
    // The target has a colour of its own (eza's `symlink_path`), with the
    // arrow between them just punctuation.
    if arrow.is_some() {
        push_face(items, at(end), at(end + 4), DiredFace::Dim);
        push_face(
            items,
            at(end + 4),
            at(chars.len()),
            DiredFace::SymlinkTarget,
        );
    }
}

// ---------------------------------------------------------- the buffer
