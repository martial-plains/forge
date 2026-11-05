// SPDX-License-Identifier: MIT OR Apache-2.0
use super::git::*;
use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn detailed_style_sizes() {
    assert_eq!(human_size(0), "0");
    assert_eq!(human_size(999), "999");
    assert_eq!(human_size(1400), "1.4k");
    assert_eq!(human_size(88_000), "88k");
    assert_eq!(human_size(5_200_000), "5.2M");
}

#[test]
fn dates_use_detailed_style() {
    let now = UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000); // 2026-09-21
    let recent = now - std::time::Duration::from_secs(86_400);
    assert_eq!(format_date(recent, now, 0), "20 Sep 14:13");
    let old = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000); // 2023-11-14
    assert_eq!(format_date(old, now, 0), "14 Nov  2023");
    assert_eq!(civil(0), (1970, 1, 1));
}

#[test]
fn render_lays_out_columns_and_parses_back() {
    let dir = std::env::temp_dir().join(format!("forge-dired-render-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("b.txt"), "hello").unwrap();
    fs::write(dir.join("A.txt"), "").unwrap();
    fs::write(dir.join(".hidden"), "").unwrap();
    let mut marks = HashMap::new();
    marks.insert(dir.join("b.txt"), '*');
    // The old listing shows dotfiles; the column listing hides them by default now.
    let text = render_section(
        &dir,
        &marks,
        View {
            all: true,
            ..View::default()
        },
    )
    .unwrap();
    let mut b = Buffer::new("t");
    b.set_contents(&text);
    assert!(b.line(1).starts_with("  Permissions"));
    assert!(b.line(1).ends_with("Name"));
    // Column order: case-insensitive, ignoring the leading dot.
    let names: Vec<String> = (FIRST_ENTRY..b.line_count())
        .filter_map(|l| entry_on_line(&b, l))
        .map(|e| e.name)
        .collect();
    assert_eq!(names, ["A.txt", "b.txt", ".hidden", "sub"]);
    assert!(b.line(3).starts_with("* "), "{:?}", b.line(3));
    assert_eq!(entry_on_line(&b, 5).map(|e| e.kind), Some('d'));
    decorate(&mut b);
    assert!(
        b.decorations()
            .iter()
            .any(|d| d.face == DecoFace::Dired(DiredFace::Directory))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sorting_by_date_puts_the_newest_first() {
    let dir = std::env::temp_dir().join(format!("forge-dired-sort-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (name, secs) in [
        ("a.txt", 1_000_000),
        ("b.txt", 3_000_000),
        ("c.txt", 2_000_000),
    ] {
        let f = fs::File::create(dir.join(name)).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }
    let names = |sort| {
        let mut b = Buffer::new("t");
        b.set_contents(
            &render_section(
                &dir,
                &HashMap::new(),
                View {
                    sort,
                    ..View::default()
                },
            )
            .unwrap(),
        );
        let order: Vec<String> = (FIRST_ENTRY..b.line_count())
            .filter_map(|l| entry_on_line(&b, l))
            .map(|e| e.name)
            .collect();
        (order, current_view(&b).sort)
    };
    assert_eq!(
        names(Sort::Name),
        (
            vec!["a.txt".to_string(), "b.txt".into(), "c.txt".into()],
            Sort::Name
        )
    );
    assert_eq!(
        names(Sort::Date),
        (
            vec!["b.txt".to_string(), "c.txt".into(), "a.txt".into()],
            Sort::Date
        )
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn git_status_letters() {
    // staged new, modified in the work tree, renamed (with its old name), untracked, ignored
    let out =
        b"A  new.rs\0 M src/lib.rs\0R  moved.rs\0old.rs\0?? scratch.txt\0!! target/\0UU both.rs\0";
    let info = parse_git_status(PathBuf::from("/repo"), out);
    let at = |p: &str| info.letters(Path::new("/repo").join(p).as_path());
    assert_eq!(at("new.rs"), "N-");
    assert_eq!(at("src/lib.rs"), "-M");
    assert_eq!(at("src"), "-M", "a directory shows what is inside it");
    assert_eq!(at("moved.rs"), "R-");
    assert_eq!(
        at("old.rs"),
        "--",
        "the rename's old path isn't a record of its own"
    );
    assert_eq!(at("scratch.txt"), "-N");
    assert_eq!(at("target"), "-I");
    assert_eq!(at("target/debug/forge"), "-I");
    assert_eq!(at("both.rs"), "UU");
    assert_eq!(at("clean.rs"), "--");
    assert_eq!(info.letters(Path::new("/elsewhere/x")), "--");
}

#[test]
fn the_git_column_appears_inside_a_repository() {
    let git = |dir: &Path, args: &[&str]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    let dir = std::env::temp_dir().join(format!("forge-dired-git-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    if !git(&dir, &["init", "-q"]) {
        return; // no git here
    }
    fs::write(dir.join("staged.txt"), "a").unwrap();
    fs::write(dir.join("loose.txt"), "b").unwrap();
    fs::write(dir.join("junk.log"), "c").unwrap();
    fs::write(dir.join(".gitignore"), "*.log\n").unwrap();
    fs::write(dir.join("sub").join("inner.txt"), "d").unwrap();
    assert!(git(&dir, &["add", "staged.txt"]));
    let dir = fs::canonicalize(&dir).unwrap();
    let text = render_section(&dir, &HashMap::new(), View::default()).unwrap();
    let line = |name: &str| {
        text.lines()
            .find(|l| l.ends_with(name))
            .unwrap_or_else(|| panic!("{name} in {text}"))
            .to_string()
    };
    assert!(
        text.lines()
            .nth(1)
            .unwrap()
            .contains("Date Modified Git Name"),
        "{text}"
    );
    assert!(
        line("staged.txt").contains(" N- "),
        "{}",
        line("staged.txt")
    );
    assert!(line("loose.txt").contains(" -N "));
    assert!(line("junk.log").contains(" -I "));
    assert!(
        line("sub").contains(" -N "),
        "a directory with new files in it"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dotfiles_are_hidden_until_a_is_pressed() {
    let dir = std::env::temp_dir().join(format!("forge-dired-dot-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(".hidden"), "").unwrap();
    fs::write(dir.join("shown"), "").unwrap();
    let names = |view| {
        let mut b = Buffer::new("t");
        b.set_contents(&render_section(&dir, &HashMap::new(), view).unwrap());
        let out: Vec<String> = (FIRST_ENTRY..b.line_count())
            .filter_map(|l| entry_on_line(&b, l))
            .map(|e| e.name)
            .collect();
        (out, b.line(0).clone())
    };
    // The default: no dotfiles, and the header says nothing.
    assert_eq!(
        names(View::default()),
        (vec!["shown".to_string()], format!("  {}:", dir.display()))
    );
    // `a` turns them on and says so, so reverting keeps them.
    let (all, head) = names(View {
        all: true,
        ..View::default()
    });
    assert_eq!(all, [".hidden", "shown"]);
    assert_eq!(head, format!("  {}:  [all]", dir.display()));
    assert!(parse_header_line(&head).unwrap().1.all);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_layout_travels_in_the_header_line() {
    let view = View {
        sort: Sort::Date,
        all: true,
        dirs_first: true,
        tree: true,
        inode: true,
        scale: true,
        ..View::default()
    };
    assert_eq!(
        view.suffix(),
        ":  [by date, all, dirs first, tree, inode, scale]"
    );
    assert_eq!(View::default().suffix(), ":");
    for v in [
        View::default(),
        view,
        View {
            tree: true,
            ..View::default()
        },
        View {
            inode: true,
            ..View::default()
        },
    ] {
        let line = format!("  /tmp/somewhere{}", v.suffix());
        assert_eq!(
            parse_header_line(&line),
            Some((PathBuf::from("/tmp/somewhere"), v)),
            "{line}"
        );
    }
    assert_eq!(parse_header_line("  not a listing"), None);
}

#[test]
fn directories_can_be_listed_first() {
    let dir = std::env::temp_dir().join(format!("forge-dired-first-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("zzz")).unwrap();
    fs::write(dir.join("aaa.txt"), "").unwrap();
    let names = |view| {
        let mut b = Buffer::new("t");
        b.set_contents(&render_section(&dir, &HashMap::new(), view).unwrap());
        (FIRST_ENTRY..b.line_count())
            .filter_map(|l| entry_on_line(&b, l))
            .map(|e| e.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(View::default()), ["aaa.txt", "zzz"]);
    assert_eq!(
        names(View {
            dirs_first: true,
            ..View::default()
        }),
        ["zzz", "aaa.txt"]
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn names_are_coloured_by_what_they_are() {
    assert_eq!(file_type("Makefile"), Some(DiredFace::Build));
    assert_eq!(file_type("Cargo.toml"), Some(DiredFace::Build));
    assert_eq!(file_type("README.md"), Some(DiredFace::Build));
    assert_eq!(file_type("photo.png"), Some(DiredFace::Image));
    assert_eq!(file_type("clip.mp4"), Some(DiredFace::Video));
    assert_eq!(file_type("song.mp3"), Some(DiredFace::Music));
    assert_eq!(file_type("song.FLAC"), Some(DiredFace::Lossless));
    assert_eq!(file_type("id_rsa"), Some(DiredFace::Crypto));
    assert_eq!(file_type("key.pem"), Some(DiredFace::Crypto));
    assert_eq!(file_type("paper.pdf"), Some(DiredFace::Document));
    assert_eq!(file_type("bundle.tar.gz"), Some(DiredFace::Compressed));
    assert_eq!(file_type("scratch.swp"), Some(DiredFace::Temp));
    assert_eq!(file_type("backup~"), Some(DiredFace::Temp));
    assert_eq!(file_type("#x.lock#"), Some(DiredFace::Temp));
    assert_eq!(file_type("lib.rs"), Some(DiredFace::Source));
    assert_eq!(file_type("a.o"), Some(DiredFace::Compiled));
    assert_eq!(file_type("notes"), None, "no extension and no known name");
    // The extension wins, so the "built from the source next to it"
    // rule never gets a look in for a .css or a .js.
    assert_eq!(file_type("styles.css"), Some(DiredFace::Source));
    assert_eq!(file_type("scripts.js"), Some(DiredFace::Source));
}

#[test]
fn a_listing_gives_its_names_a_keymap_of_their_own() {
    let dir = std::env::temp_dir().join(format!("forge-dired-regions-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "").unwrap();
    let mut b = Buffer::new("t");
    b.set_contents(&render_section(&dir, &HashMap::new(), View::default()).unwrap());
    decorate(&mut b);
    let name_line = (FIRST_ENTRY..b.line_count())
        .find(|&l| entry_on_line(&b, l).is_some_and(|e| e.name == "a.txt"))
        .unwrap();
    let base = b.pos_to_offset(Pos::new(name_line, 0));
    fn maps_at(b: &Buffer, offset: usize) -> Vec<&'static str> {
        b.decoration_keymaps_at(offset)
            .into_iter()
            .filter_map(|k| match k {
                ENTRY_REGION => Some(ENTRY_REGION),
                COLUMNS_REGION => Some(COLUMNS_REGION),
                SECTION_REGION => Some(SECTION_REGION),
                _ => None,
            })
            .collect()
    }
    // The name is in the entry region, the column header in its own,
    // and the section header in a third.
    assert_eq!(
        maps_at(&b, base + b.line(name_line).chars().count() - 1),
        [ENTRY_REGION],
        "the name itself is the link"
    );
    let columns = b.pos_to_offset(Pos::new(1, 2));
    assert_eq!(maps_at(&b, columns), [COLUMNS_REGION]);
    let section = b.pos_to_offset(Pos::new(0, 4));
    assert_eq!(maps_at(&b, section), [SECTION_REGION]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_listing_colours_its_names_by_type() {
    let dir = std::env::temp_dir().join(format!("forge-dired-types-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Makefile"), "").unwrap();
    fs::write(dir.join("photo.png"), "").unwrap();
    fs::write(dir.join("notes"), "").unwrap();
    let mut b = Buffer::new("t");
    b.set_contents(&render_section(&dir, &HashMap::new(), View::default()).unwrap());
    decorate(&mut b);
    let layout = Layout::of(&b.line(1)).unwrap();
    let face_of = |name: &str| {
        let line = (FIRST_ENTRY..b.line_count())
            .find(|l| entry_on_line(&b, *l).is_some_and(|e| e.name == name))
            .unwrap();
        // An optional Nerd Font icon and its space sit before the name.
        let icon = b
            .line(line)
            .chars()
            .skip(layout.name)
            .take_while(|c| is_icon(*c) || *c == ' ')
            .count();
        let at = b.pos_to_offset(Pos::new(line, 0)) + layout.name + icon;
        b.decorations()
            .iter()
            // The region's own (plain) decoration sits on the same
            // text; this is about the face.
            .find(|d| d.owner == OWNER && d.start == at && d.face != DecoFace::Plain)
            .map(|d| d.face)
    };
    assert_eq!(face_of("Makefile"), Some(DecoFace::Dired(DiredFace::Build)));
    assert_eq!(
        face_of("photo.png"),
        Some(DecoFace::Dired(DiredFace::Image))
    );
    assert_eq!(face_of("notes"), None, "an ordinary file is left alone");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sizes_are_shaded_by_magnitude() {
    assert_eq!(size_face(0), DiredFace::Size);
    assert_eq!(size_face(999_999), DiredFace::Size);
    assert_eq!(size_face(1_000_000), DiredFace::SizeMega);
    assert_eq!(size_face(1_000_000_000), DiredFace::SizeGiga);
    assert_eq!(size_face(1_000_000_000_000), DiredFace::SizeHuge);
    assert_eq!(bytes_in("1.4k"), Some(1400));
    assert_eq!(bytes_in("88k"), Some(88_000));
    assert_eq!(bytes_in("5.2M"), Some(5_200_000));
    assert_eq!(bytes_in("340"), Some(340));
    assert_eq!(bytes_in("-"), None, "a directory has no size");
}

#[test]
fn the_gradient_keeps_the_colour_and_moves_only_its_lightness() {
    // The ramp is on lightness only, from the minimum luminance up to 1,
    // so the smallest value in a listing is half-lit and the largest is
    // nearly white. The hue is kept, so it never turns into a
    // different colour on the way.
    assert_eq!(MIN_LUMINANCE, 0.5);
    let low = shade(0x00ff00, 0.0);
    let high = shade(0x00ff00, 1.0);
    assert_ne!(low, high);
    let luma = |rgb: u32| channels(rgb).1;
    assert!(luma(low) < luma(high), "it lightens as the value grows");
    assert!(
        luma(low) > 0.4 && luma(low) < 0.7,
        "the bottom of the ramp is half-lit: {low:06x}"
    );
    assert!(
        luma(high) > 0.95,
        "the top is as light as a colour gets: {high:06x}"
    );
    // The hue is kept rather than the channels, so green is still the
    // strongest channel of a shade of green even once it is lightened.
    let (r, g, b) = channels(low);
    assert!(g >= r && g > b, "green is still the greenest: {low:06x}");
    // Every step stays on the ramp, so the order never goes backwards.
    for base in [0x00ff00, 0x0000ff, 0xff0000] {
        let ramp: Vec<u32> = (0..=10).map(|i| shade(base, i as f64 / 10.0)).collect();
        for pair in ramp.windows(2) {
            assert!(luma(pair[0]) <= luma(pair[1]), "{base:06x}: {pair:06x?}");
        }
    }
    // A range of one value has no scale to it, so it is the top.
    let one = Range { min: 5.0, max: 5.0 };
    assert_eq!(one.ratio(5.0), 1.0);
    let range = Range {
        min: 0.0,
        max: 100.0,
    };
    assert_eq!(range.ratio(-10.0), 0.0);
    assert_eq!(range.ratio(150.0), 1.0);
    assert_eq!(range.ratio(25.0), 0.25);
}

#[test]
fn the_gradient_reads_its_own_sizes_and_dates_back() {
    // What the listing writes has to read back as what it stands for,
    // or the gradient would measure the wrong things.
    let now = UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000);
    let then = now - std::time::Duration::from_secs(86_400);
    let (tz, n) = (env().tz, unix_secs(now));
    // The cell only holds minutes, so that is as close as it can come.
    let to_minute = |t: i64| t - t.rem_euclid(60);
    assert_eq!(
        seconds_in(&format_date(then, now, tz), n, tz),
        Some(to_minute(unix_secs(then)))
    );
    // An older file is written as a bare day, so that is all it carries:
    // it reads back as that day's local midnight.
    let old = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    assert_eq!(
        seconds_in(&format_date(old, now, tz), n, tz),
        Some(days_from_civil(2023, 11, 14) * 86_400 - tz)
    );
    assert_eq!(
        civil(days_from_civil(2024, 2, 29)),
        (2024, 2, 29),
        "a leap day"
    );
    // A cell with no year is read as this year, unless that would put
    // it in the future, in which case it was last year's.
    let (year, month, day) = civil((n + tz).div_euclid(86_400));
    let (month, day) = (month as i64, day);
    let bare = format!("{day:>2} {}", MONTHS[month as usize - 1]);
    assert_eq!(
        seconds_in(&format!("{bare} {year}"), n, tz),
        Some(days_from_civil(year, month, day) * 86_400 - tz)
    );
    let (_, next_month, next_day) = civil((n + tz).div_euclid(86_400) + 1);
    assert_eq!(
        seconds_in(&format!("{next_day:>2} {}", MONTHS[next_month - 1]), n, tz),
        Some(days_from_civil(year - 1, next_month as i64, next_day) * 86_400 - tz),
        "tomorrow hasn't happened yet"
    );
}

#[test]
fn the_gradient_spreads_a_listing_out_by_size_and_age() {
    let dir = std::env::temp_dir().join(format!("forge-dired-scale-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let sized = |name: &str, len: u64, secs: u64| {
        let f = fs::File::create(dir.join(name)).unwrap();
        f.set_len(len).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    };
    sized("small", 10, 1_700_000_000);
    sized("huge", 5_000_000_000, 1_790_000_000);
    let shade_of = |name: &str| {
        let view = View {
            scale: true,
            ..View::default()
        };
        let mut b = Buffer::new("t");
        b.set_contents(&render_section(&dir, &HashMap::new(), view).unwrap());
        decorate(&mut b);
        let line = (FIRST_ENTRY..b.line_count())
            .find(|l| entry_on_line(&b, *l).is_some_and(|e| e.name == name))
            .unwrap();
        b.decorations()
            .iter()
            .filter(|d| {
                d.owner == OWNER
                    && d.start > b.pos_to_offset(Pos::new(line, 0))
                    && d.start < b.pos_to_offset(Pos::new(line + 1, 0))
            })
            .find_map(|d| match d.face {
                DecoFace::Dired(DiredFace::Shade(rgb)) => Some(rgb),
                _ => None,
            })
    };
    let (small, huge) = (
        shade_of("small").expect("a shaded size"),
        shade_of("huge").expect("a shaded size"),
    );
    assert_ne!(small, huge, "the two sizes are told apart");
    // Off, the sizes are the plain tiers instead.
    let mut b = Buffer::new("t");
    b.set_contents(&render_section(&dir, &HashMap::new(), View::default()).unwrap());
    decorate(&mut b);
    assert!(
        !b.decorations()
            .iter()
            .any(|d| matches!(d.face, DecoFace::Dired(DiredFace::Shade(_)))),
        "no gradient until it is asked for"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
#[cfg(unix)]
fn a_symlinks_target_is_coloured_on_its_own() {
    let dir = std::env::temp_dir().join(format!("forge-dired-link-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("target.txt"), "").unwrap();
    std::os::unix::fs::symlink("target.txt", dir.join("link.txt")).unwrap();
    let mut b = Buffer::new("t");
    b.set_contents(&render_section(&dir, &HashMap::new(), View::default()).unwrap());
    decorate(&mut b);
    let line = (FIRST_ENTRY..b.line_count())
        .find(|l| entry_on_line(&b, *l).is_some_and(|e| e.name == "link.txt"))
        .unwrap();
    let from = b.pos_to_offset(Pos::new(line, 0));
    let to = from + b.line_len(line);
    let faces: Vec<DiredFace> = b
        .decorations()
        .iter()
        .filter(|d| d.owner == OWNER && d.start >= from && d.end <= to)
        .filter_map(|d| match d.face {
            DecoFace::Dired(f) => Some(f),
            _ => None,
        })
        .collect();
    assert!(
        faces.contains(&DiredFace::Symlink),
        "the link itself: {faces:?}"
    );
    assert!(
        faces.contains(&DiredFace::SymlinkTarget),
        "the target after the arrow: {faces:?}"
    );
    assert_eq!(
        entry_on_line(&b, line).map(|e| e.path),
        Some(dir.join("link.txt"))
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_inode_column_sits_before_the_permissions() {
    let dir = std::env::temp_dir().join(format!("forge-dired-inode-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("a.txt"), "").unwrap();
    let text = render_section(
        &dir,
        &HashMap::new(),
        View {
            inode: true,
            ..View::default()
        },
    )
    .unwrap();
    let mut b = Buffer::new("t");
    b.set_contents(&text);
    // "Inode" is right-aligned over the numbers it heads, as the size
    // column's "Size" is.
    let head = b.line(1).clone();
    let inode_col = head.find("Inode").unwrap();
    assert_eq!(head[..inode_col].trim(), "", "{head:?}");
    let perms_col = head.find("Permissions").unwrap();
    // "Permissions" is exactly as wide as its column, so one space is all
    // that separates them.
    assert_eq!(perms_col, inode_col + "Inode".len() + 1, "{head:?}");
    let layout = Layout::of(&head).unwrap();
    assert_eq!(layout.inode, Some(inode_col));
    // With the column on, the permissions move along, and everything
    // that reads them finds them there anyway.
    assert!(layout.perms > inode_col);
    let line = FIRST_ENTRY;
    assert_eq!(
        entry_on_line(&b, line).map(|e| e.name),
        Some("a.txt".to_string())
    );
    let inode_cell: String = b.line(line).chars().take(layout.perms).collect();
    assert!(
        inode_cell.trim().parse::<u64>().is_ok(),
        "{inode_cell:?} is an inode"
    );
    // The label ends where the widest number does.
    let numbers: String = b.line(line).chars().take(layout.perms).collect();
    assert_eq!(
        inode_col + "Inode".len(),
        numbers.trim_end().chars().count()
    );
    decorate(&mut b);
    assert!(
        b.decorations()
            .iter()
            .any(|d| d.face == DecoFace::Dired(DiredFace::Inode))
    );
    // Without it, nothing shifts.
    let plain = render_section(&dir, &HashMap::new(), View::default()).unwrap();
    assert!(plain.lines().nth(1).unwrap().starts_with("  Permissions"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_tree_nests_the_names_under_their_directories() {
    let dir = std::env::temp_dir().join(format!("forge-dired-tree-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src/deep")).unwrap();
    fs::write(dir.join("README.md"), "").unwrap();
    fs::write(dir.join("src/lib.rs"), "").unwrap();
    fs::write(dir.join("src/deep/inner.rs"), "").unwrap();
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".git/HEAD"), "").unwrap();
    let view = View {
        tree: true,
        ..View::default()
    };
    let text = render_section(&dir, &HashMap::new(), view).unwrap();
    assert!(
        text.lines().next().unwrap().ends_with(":  [tree]"),
        "{}",
        text.lines().next().unwrap()
    );
    let mut b = Buffer::new("t");
    b.set_contents(&text);
    // The drawing stops where a directory ends. Each directory is in
    // name order, so "deep" comes before "lib.rs".
    let cells = |li: usize| -> String {
        let line: Vec<char> = text.lines().nth(li).unwrap().chars().collect();
        let col = text.lines().nth(1).unwrap().find("Name").unwrap();
        let cells = tree_cells(&line, col) / 4;
        let tree: String = line[col..col + cells * 4].iter().collect();
        let name = entry_on_line(&b, li).unwrap();
        format!("{tree}{}", name.name)
    };
    assert_eq!(
        (2..7).map(cells).collect::<Vec<_>>(),
        [
            "├── README.md",
            "└── src",
            "    ├── deep",
            "    │   └── inner.rs",
            "    └── lib.rs"
        ],
        "{text}"
    );
    let mut b = Buffer::new("t");
    b.set_contents(&text);
    let paths: Vec<PathBuf> = (FIRST_ENTRY..b.line_count())
        .filter_map(|l| entry_on_line(&b, l))
        .map(|e| e.path)
        .collect();
    assert_eq!(
        paths,
        [
            dir.join("README.md"),
            dir.join("src"),
            dir.join("src/deep"),
            dir.join("src/deep/inner.rs"),
            dir.join("src/lib.rs"),
        ]
    );
    assert_eq!(
        entry_on_line(&b, FIRST_ENTRY + 4).map(|e| e.name),
        Some("lib.rs".to_string())
    );
    // The dotfiles stay out of the tree as well.
    assert!(!text.contains("HEAD"));
    // ...and come in with `a`.
    let all = render_section(
        &dir,
        &HashMap::new(),
        View {
            tree: true,
            all: true,
            ..View::default()
        },
    )
    .unwrap();
    assert!(all.contains(".git"), "{all}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn tree_cells_are_told_from_the_names_beside_them() {
    let cells: Vec<String> = ["├── ", "│   ", "└── ", "    "]
        .iter()
        .map(|c| c.to_string())
        .collect();
    let line: Vec<char> = "├── src/deep".chars().collect();
    assert_eq!(tree_cells(&line, 0), 4);
    let deep: Vec<char> = "│   └── inner".chars().collect();
    assert_eq!(tree_cells(&deep, 0), 8);
    // A name that merely starts with the same box-drawing characters is
    // a name, not a cell: only whole four-character cells count.
    let tricky: Vec<char> = "├───file".chars().collect();
    assert_eq!(tree_cells(&tricky, 0), 0);
    let plain: Vec<char> = "readme".chars().collect();
    assert_eq!(tree_cells(&plain, 0), 0);
    assert_eq!(Branch::parse("├── "), Some(Branch::Edge));
    assert_eq!(Branch::parse("read"), None);
    let _ = cells;
}

fn listing_app(tag: &str) -> (App, PathBuf) {
    let dir = std::env::temp_dir().join(format!("forge-dired-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("sub")).unwrap();
    for f in ["a.txt", "b.txt", "c.log", "old.txt~", "run.sh"] {
        fs::write(dir.join(f), format!("content of {f}\n")).unwrap();
    }
    #[cfg(unix)]
    fs::set_permissions(dir.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut app = App::new(forge_core::Editor::default());
    let id = app.editor.add_buffer(Buffer::new("scratch"));
    app.editor.show_buffer(id);
    open(&mut app, &dir);
    let dir = fs::canonicalize(&dir).unwrap();
    (app, dir)
}

fn marked(app: &App, mark: char) -> Vec<String> {
    let mut v: Vec<String> = paths_with_mark(app.buf_ref(), mark)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn marking_by_kind_regexp_and_backups() {
    let (mut a, _dir) = listing_app("mark");
    execute(&mut a, "dired-mark-executables");
    assert_eq!(marked(&a, '*'), ["run.sh"]);
    execute(&mut a, "dired-toggle-marks");
    assert_eq!(
        marked(&a, '*'),
        ["a.txt", "b.txt", "c.log", "old.txt~", "sub"]
    );
    execute(&mut a, "dired-unmark-all-marks");
    execute(&mut a, "dired-mark-directories");
    assert_eq!(marked(&a, '*'), ["sub"]);
    extra::answered(&mut a, Extra::MarkRegexp('*'), r"\.txt$");
    assert_eq!(marked(&a, '*'), ["a.txt", "b.txt", "sub"]);
    extra::answered(&mut a, Extra::ChangeMarks, "* D");
    assert_eq!(marked(&a, 'D'), ["a.txt", "b.txt", "sub"]);
    execute(&mut a, "dired-unmark-all-marks");
    execute(&mut a, "dired-flag-backup-files");
    assert_eq!(marked(&a, 'D'), ["old.txt~"]);
    extra::answered(&mut a, Extra::MarkContaining, "content of c");
    assert_eq!(marked(&a, '*'), ["c.log"]);
    extra::answered(&mut a, Extra::MarkExtension, "sh");
    assert_eq!(marked(&a, '*'), ["c.log", "run.sh"]);
}

#[test]
fn links_regexp_renames_chmod_and_shell() {
    let (mut a, dir) = listing_app("ops");
    extra::answered(&mut a, Extra::MarkRegexp('*'), r"^[ab]\.txt$");
    // Regexp rename.
    let paths = paths_with_mark(a.buf_ref(), '*');
    extra::answered(
        &mut a,
        Extra::RegexpOp('R', paths),
        r"^(.)\.txt => \1-new.md",
    );
    assert!(dir.join("a-new.md").exists() && dir.join("b-new.md").exists());
    assert!(!dir.join("a.txt").exists());
    // Symlinks, absolute and relative.
    #[cfg(unix)]
    {
        extra::answered(
            &mut a,
            Extra::Link('S', vec![dir.join("c.log")]),
            &dir.join("c.sym").display().to_string(),
        );
        assert_eq!(fs::read_link(dir.join("c.sym")).unwrap(), dir.join("c.log"));
        extra::answered(
            &mut a,
            Extra::Link('Y', vec![dir.join("c.log")]),
            &dir.join("sub/c.rel").display().to_string(),
        );
        assert_eq!(
            fs::read_link(dir.join("sub/c.rel")).unwrap(),
            PathBuf::from("../c.log")
        );
    }
    extra::answered(
        &mut a,
        Extra::Link('H', vec![dir.join("c.log")]),
        &dir.join("c.hard").display().to_string(),
    );
    assert_eq!(
        fs::read_to_string(dir.join("c.hard")).unwrap(),
        "content of c.log\n"
    );
    // chmod, octal and symbolic.
    #[cfg(unix)]
    {
        extra::answered(&mut a, Extra::Chmod(vec![dir.join("c.log")]), "600");
        assert_eq!(
            fs::metadata(dir.join("c.log"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        extra::answered(&mut a, Extra::Chmod(vec![dir.join("c.log")]), "u+x,go+r");
        assert_eq!(
            fs::metadata(dir.join("c.log"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o744
        );
    }
    // Upper-casing.
    goto_path(&mut a, &dir.join("c.log"));
    execute(&mut a, "dired-mark");
    execute(&mut a, "dired-upcase");
    assert!(dir.join("C.LOG").exists());
    // Replacement across files.
    extra::answered(
        &mut a,
        Extra::ReplaceApply(vec![dir.join("a-new.md")], "content".into(), "text".into()),
        "y",
    );
    assert_eq!(
        fs::read_to_string(dir.join("a-new.md")).unwrap(),
        "text of a.txt\n"
    );
    // Compression to an archive.
    extra::answered(
        &mut a,
        Extra::CompressTo(vec![dir.join("b-new.md")]),
        &dir.join("pack.tar.gz").display().to_string(),
    );
    assert!(dir.join("pack.tar.gz").exists());
}

#[test]
fn wdired_renames_edited_names() {
    let (mut a, dir) = listing_app("wdired");
    execute(&mut a, "wdired-change-to-wdired-mode");
    assert_eq!(a.buf_ref().major_mode(), WDIRED_MODE);
    let line = entries(a.buf_ref())
        .into_iter()
        .find(|(_, _, e)| e.name == "c.log")
        .unwrap()
        .0;
    let text = a.buf_ref().line(line);
    let at = text.rfind("c.log").unwrap();
    let col = text[..at].chars().count();
    a.buf()
        .replace_range(Pos::new(line, col), Pos::new(line, col + 5), "d.log");
    execute(&mut a, "wdired-finish-edit");
    assert!(dir.join("d.log").exists() && !dir.join("c.log").exists());
    assert_eq!(a.buf_ref().major_mode(), MODE);
}

#[test]
fn motion_and_goto() {
    let (mut a, dir) = listing_app("move");
    extra::answered(&mut a, Extra::Goto, "run.sh");
    let line = a.buf_ref().point().line;
    assert_eq!(entry_on_line(a.buf_ref(), line).unwrap().name, "run.sh");
    execute(&mut a, "dired-next-dirline");
    let line = a.buf_ref().point().line;
    assert_eq!(
        entry_on_line(a.buf_ref(), line).unwrap().path,
        dir.join("sub")
    );
    execute(&mut a, "dired-previous-line");
    let line = a.buf_ref().point().line;
    assert_eq!(entry_on_line(a.buf_ref(), line).unwrap().name, "run.sh");
}

#[test]
fn details_omit_and_hidden_subdirs() {
    let (mut a, dir) = listing_app("hide");
    fs::write(dir.join("sub/inner.txt"), "x").unwrap();
    // Hide the details: names, marks and kinds still parse.
    execute(&mut a, "dired-hide-details-mode");
    assert!(a.buf_ref().line(0).contains("no details"));
    assert!(a.buf_ref().line(1).ends_with("(details hidden)"));
    execute(&mut a, "dired-mark-directories");
    assert_eq!(marked(&a, '*'), ["sub"]);
    execute(&mut a, "dired-hide-details-mode");
    assert!(a.buf_ref().line(1).starts_with("  total ") && !a.buf_ref().line(1).contains("hidden"));
    assert_eq!(marked(&a, '*'), ["sub"]);
    // Omit mode drops the backup file.
    assert!(
        entries(a.buf_ref())
            .iter()
            .any(|(_, _, e)| e.name == "old.txt~")
    );
    execute(&mut a, "dired-omit-mode");
    assert!(
        !entries(a.buf_ref())
            .iter()
            .any(|(_, _, e)| e.name == "old.txt~")
    );
    execute(&mut a, "dired-omit-mode");
    // An inline listing can be hidden and comes back.
    goto_path(&mut a, &dir.join("sub"));
    execute(&mut a, "dired-maybe-insert-subdir");
    assert!(
        entries(a.buf_ref())
            .iter()
            .any(|(_, _, e)| e.name == "inner.txt")
    );
    execute(&mut a, "dired-hide-subdir");
    assert!(
        !entries(a.buf_ref())
            .iter()
            .any(|(_, _, e)| e.name == "inner.txt")
    );
    assert!(a.buf_ref().get_contents().contains("hidden"));
    execute(&mut a, "dired-hide-subdir");
    assert!(
        entries(a.buf_ref())
            .iter()
            .any(|(_, _, e)| e.name == "inner.txt")
    );
    // Hiding all, then showing all.
    execute(&mut a, "dired-hide-all");
    assert!(entries(a.buf_ref()).is_empty());
    execute(&mut a, "dired-hide-all");
    assert!(!entries(a.buf_ref()).is_empty());
}

#[test]
fn image_dired_lists_pictures_and_the_keys_are_the_defaults() {
    let (mut a, dir) = listing_app("img");
    fs::write(dir.join("p.png"), "x").unwrap();
    fs::write(dir.join("q.jpg"), "x").unwrap();
    execute(&mut a, "dired-revert");
    execute(&mut a, "image-dired-display-thumbs");
    let b = a.buf_ref();
    assert_eq!(b.major_mode(), extra::THUMBS_MODE);
    assert_eq!(b.inline_images.len(), 2);
    // keys: T is touch, not --tree.
    let (a2, _) = listing_app("keys");
    let _ = a2;
    for (keys, cmd) in crate::keymap::DIRED_MODE_BINDINGS {
        if *keys == "T" {
            assert_eq!(*cmd, "dired-do-touch");
        }
        if *keys == "C-c C-t" {
            assert_eq!(*cmd, "dired-toggle-tree");
        }
    }
}

#[test]
fn undo_takes_back_a_mark() {
    let (mut a, _dir) = listing_app("undo");
    execute(&mut a, "dired-mark");
    assert_eq!(marked(&a, '*').len(), 1);
    crate::commands::execute(&mut a, "undo");
    assert!(marked(&a, '*').is_empty(), "undo should take the mark back");
}

#[test]
fn a_new_listing_looks_like_ls() {
    let (mut a, dir) = listing_app("ls-style");
    let b = a.buf_ref();
    // "  /dir:  [ls, all]", then ls's own "total N", then one ls -l line each.
    assert!(b.line(0).contains("[ls"), "{:?}", b.line(0));
    assert!(
        b.line(1).trim_start().starts_with("total "),
        "{:?}",
        b.line(1)
    );
    let run = (FIRST_ENTRY..b.line_count())
        .find(|&l| entry_on_line(b, l).is_some_and(|e| e.name == "run.sh"))
        .expect("run.sh is listed");
    let text = b.line(run);
    let fields: Vec<&str> = text.split_whitespace().collect();
    assert_eq!(fields[0], "-rwxr-xr-x", "{text:?}");
    assert_eq!(fields[4], "18", "the size is exact bytes: {text:?}");
    assert_eq!(fields.last().copied(), Some("run.sh"));
    // Names, kinds and paths come back from that text.
    let sub = entries(b)
        .into_iter()
        .find(|(_, _, e)| e.name == "sub")
        .unwrap()
        .2;
    assert_eq!((sub.kind, sub.path), ('d', dir.join("sub")));
    // Colours: the directory's name, the owner and group columns.
    let faces: Vec<DiredFace> = b
        .decorations()
        .iter()
        .filter_map(|d| match d.face {
            DecoFace::Dired(f) => Some(f),
            _ => None,
        })
        .collect();
    for f in [
        DiredFace::Directory,
        DiredFace::User,
        DiredFace::Group,
        DiredFace::Executable,
    ] {
        assert!(faces.contains(&f), "{f:?} missing");
    }
    // The old columns are one key away, and back.
    execute(&mut a, "dired-toggle-ls-style");
    assert!(a.buf_ref().line(1).contains("Permissions"));
    execute(&mut a, "dired-toggle-ls-style");
    assert!(a.buf_ref().line(1).trim_start().starts_with("total "));
    // Marks and commands work on ls lines as they do on detailed columns.
    execute(&mut a, "dired-mark-directories");
    assert_eq!(marked(&a, '*'), ["sub"]);
}

#[test]
fn a_tether_directory_is_a_real_dired_listing() {
    let (_a, dir) = listing_app("remote-src");
    let name = format!("/local::{}", dir.display());
    let mut a = App::new(forge_core::Editor::default());
    let id = a.editor.add_buffer(Buffer::new("scratch"));
    a.editor.show_buffer(id);
    open(&mut a, Path::new(&name));
    let b = a.buf_ref();
    assert_eq!(b.major_mode(), MODE, "{:?}", b.line(0));
    assert!(b.line(0).contains("/local::"), "{:?}", b.line(0));
    assert!(
        b.line(1).trim_start().starts_with("total "),
        "{:?}",
        b.line(1)
    );
    let listed: Vec<String> = entries(b).into_iter().map(|(_, _, e)| e.name).collect();
    assert_eq!(
        listed,
        ["a.txt", "b.txt", "c.log", "old.txt~", "run.sh", "sub"]
    );
    let sub = entries(b)
        .into_iter()
        .find(|(_, _, e)| e.name == "sub")
        .unwrap()
        .2;
    assert_eq!(sub.kind, 'd');
    assert_eq!(sub.path, PathBuf::from(format!("{name}/sub")));
    let run = entries(b)
        .into_iter()
        .find(|(_, _, e)| e.name == "run.sh")
        .unwrap()
        .2;
    assert_eq!(run.kind, '-');

    // Operations on the far side: make a directory, copy, rename, delete.
    let made = PathBuf::from(format!("{name}/new/deep"));
    rfs::mkdir(&made).unwrap();
    assert!(dir.join("new/deep").is_dir());
    let src = PathBuf::from(format!("{name}/a.txt"));
    rfs::copy(&src, &PathBuf::from(format!("{name}/copy.txt"))).unwrap();
    assert!(dir.join("copy.txt").is_file());
    rfs::rename(
        &PathBuf::from(format!("{name}/copy.txt")),
        &PathBuf::from(format!("{name}/moved.txt")),
    )
    .unwrap();
    assert!(!dir.join("copy.txt").exists() && dir.join("moved.txt").exists());
    #[cfg(unix)]
    {
        rfs::chmod(&PathBuf::from(format!("{name}/moved.txt")), "600").unwrap();
        assert_eq!(
            fs::metadata(dir.join("moved.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(rfs::exists(&PathBuf::from(format!("{name}/moved.txt"))));
    assert!(rfs::is_dir(&PathBuf::from(format!("{name}/sub"))));
    delete_confirmed(&mut a, vec![PathBuf::from(format!("{name}/moved.txt"))]);
    assert!(!dir.join("moved.txt").exists());
    // Between a remote and this machine.
    rfs::copy(&src, &dir.join("sub/local-copy.txt")).unwrap();
    assert!(dir.join("sub/local-copy.txt").exists());
    // Climbing out of the listing.
    assert_eq!(
        rfs::parent(&sub.path),
        Some(PathBuf::from(format!("{name}/")))
    );
    assert_eq!(rfs::parent(Path::new("/local::/")), None);
}
