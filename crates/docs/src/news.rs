// SPDX-License-Identifier: MIT OR Apache-2.0
//! `NEWS.org`: the release notes, and the data behind `view-news`.
//!
//! The file lives at the repository root and is bundled into the binary the
//! same way the manual is, so `view-news` works without an install step. Each
//! top-level Org heading is one release; the body is parsed with the manual's
//! markup pipeline, so =keys=, `[[file:forge.org][links]]` and =M-x command=
//! references render with live links in the help buffer.

use std::sync::OnceLock;

use crate::ast::Block;
use crate::manual::{parse_body, parse_inline};

/// The bundled `NEWS.org`.
pub const NEWS_SRC: &str = include_str!("../../../NEWS.org");

/// One release's section of `NEWS.org`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The heading text (the version, usually).
    pub title: String,
    /// The parsed body, with `**` sub-headings kept as [`Block::Heading`].
    pub body: Vec<Block>,
}

/// Parse `NEWS.org` into releases, in the order they are written (newest
/// first by convention).
pub fn releases(source: &str) -> Vec<Release> {
    let lines: Vec<&str> = source.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match heading(lines[i]) {
            Some((1, rest)) => {
                let title = rest.to_string();
                i += 1;
                let start = i;
                while i < lines.len() && !matches!(heading(lines[i]), Some((1, _))) {
                    i += 1;
                }
                out.push(Release {
                    title,
                    body: release_body(&lines[start..i]),
                });
            }
            _ => i += 1,
        }
    }
    out
}

/// The newest release (the first one written), if the file has any.
pub fn newest(source: &str) -> Option<Release> {
    releases(source).into_iter().next()
}

/// The bundled releases, parsed once.
pub fn the_news() -> &'static [Release] {
    static NEWS: OnceLock<Vec<Release>> = OnceLock::new();
    NEWS.get_or_init(|| releases(NEWS_SRC))
}

/// The newest bundled release, if any.
pub fn newest_release() -> Option<&'static Release> {
    the_news().first()
}

/// One Org heading: the number of leading `*` and the text after the space.
fn heading(line: &str) -> Option<(usize, &str)> {
    let stars = line.bytes().take_while(|b| *b == b'*').count();
    if stars == 0 {
        return None;
    }
    let rest = &line[stars..];
    let rest = rest.strip_prefix(' ')?;
    Some((stars, rest.trim_end()))
}

/// Parse a release body, turning `**` (and deeper) headings into blocks and
/// running the rest through the manual's block parser.
fn release_body(lines: &[&str]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    for &line in lines {
        if let Some((level, rest)) = heading(line)
            && level >= 2
        {
            flush(&mut para, &mut blocks);
            blocks.push(Block::Heading(level as u8, parse_inline(rest)));
            continue;
        }
        para.push(line);
    }
    flush(&mut para, &mut blocks);
    blocks
}

fn flush(para: &mut Vec<&str>, blocks: &mut Vec<Block>) {
    if para.is_empty() {
        return;
    }
    blocks.extend(parse_body(&para.join("\n")));
    para.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
#+title: NEWS

* 0.2.0
Fixed a thing.

** Commands
- =new-command= does a thing.

* 0.1.0
The first release.
";

    #[test]
    fn releases_are_split_on_top_level_headings() {
        let releases = releases(SAMPLE);
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].title, "0.2.0");
        assert_eq!(releases[1].title, "0.1.0");
        assert_eq!(newest(SAMPLE).unwrap().title, "0.2.0");
    }

    #[test]
    fn sub_headings_become_heading_blocks() {
        let releases = releases(SAMPLE);
        assert!(
            releases[0]
                .body
                .iter()
                .any(|b| matches!(b, Block::Heading(2, _))),
            "expected a level-2 heading, got {:?}",
            releases[0].body
        );
        assert!(
            releases[0]
                .body
                .iter()
                .any(|b| matches!(b, Block::List { .. })),
            "expected the bullet list, got {:?}",
            releases[0].body
        );
    }

    #[test]
    fn the_bundled_news_has_a_release() {
        let release = newest_release().expect("NEWS.org has a release");
        assert_eq!(release.title, "0.1.0");
        assert!(!release.body.is_empty());
    }
}
