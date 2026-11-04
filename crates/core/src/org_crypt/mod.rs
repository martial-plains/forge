// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org Crypt entry structure (stage 1): ranges and PGP-block detection.
//!
//! Mirrors the structural half of `org-crypt.el`: which lines of an
//! entry are encryptable, whether an entry is already encrypted, which
//! headings a tag matcher selects, and which key each heading wants. There
//! is deliberately no crypto here, so backends (a pure-Rust OpenPGP crate,
//! GPGME, or a `gpg` subprocess) can build on these ranges without depending
//! on any of them.
//!
//! Reference behavior (`org-crypt.el`):
//! - Only the entry body is encrypted: everything from end-of-metadata
//!   (past the planning line and the property drawer) to end-of-subtree,
//!   trailing blank lines excluded. Headline and properties stay plaintext.
//! - An encrypted entry body is one ASCII-armored block:
//!   `-----BEGIN PGP MESSAGE-----` through `-----END PGP MESSAGE-----`.
//! - Key selection: a nil default key means symmetric encryption
//!   unconditionally (a `CRYPTKEY` property is ignored); otherwise the
//!   heading's `CRYPTKEY` property wins over the default, and an empty key
//!   falls back to symmetric. A `CRYPTKEY` of `"nil"` counts as empty.
//! - Forge excludes the crypt tag from inheritance so an already-encrypted
//!   parent is not encrypted again. Forge has no tag-inheritance exclusion,
//!   so `org_crypt_encryptable_entries` instead returns only the outermost
//!   matching headings, which has the same effect.
//!
//! Simplifications vs Forge, to revisit with the backend:
//!
//! - Drawers other than `:PROPERTIES:` (e.g. `:LOGBOOK:`) count as content
//!   and would be encrypted. end-of-metadata skips them.
//! - `CRYPTKEY` is inherited from the nearest ancestor; Forge's
//!   `'selective` inheritance is not modeled.

pub mod crypto;

use crate::buffer::Buffer;

/// The default value of `org-crypt-tag-matcher`.
pub const DEFAULT_TAG_MATCHER: &str = "crypt";

/// The property naming a per-heading encryption key.
pub const CRYPTKEY_PROPERTY: &str = "CRYPTKEY";

/// How heading `h` wants its body encrypted (see `org_crypt_key_for_heading`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptKey {
    /// Symmetric encryption (passphrase).
    Symmetric,
    /// Encrypt for these recipients (a keyring match string).
    Recipients(String),
}

fn is_begin_pgp(line: &str) -> bool {
    line.trim() == "-----BEGIN PGP MESSAGE-----"
}

fn is_end_pgp(line: &str) -> bool {
    line.trim() == "-----END PGP MESSAGE-----"
}

impl Buffer {
    /// First body line of heading `h`: past the planning line and the
    /// property drawer. May lie beyond the subtree end for an empty entry.
    pub fn org_crypt_content_start(&self, h: usize) -> usize {
        let mut beg = h + 1;
        if self.org_planning_line(h).is_some() {
            beg += 1;
        }
        if let Some((_, z)) = self.org_property_drawer(h) {
            beg = beg.max(z + 1);
        }
        beg
    }

    /// Encryptable body lines of heading `h`, trailing blank lines excluded.
    /// None when the entry has no body. The range covers the whole subtree,
    /// including sub-headings, matching `org-end-of-subtree`.
    pub fn org_crypt_entry_range(&self, h: usize) -> Option<(usize, usize)> {
        self.org_level(h)?;
        let beg = self.org_crypt_content_start(h);
        let mut end = self.org_subtree_end(h);
        // Trailing blank lines are not part of the body: an entry whose
        // body is blank has no body at all.
        while end >= beg && self.line(end).trim().is_empty() {
            end -= 1;
        }
        (beg <= end).then_some((beg, end))
    }

    /// The armored PGP block of heading `h`, as inclusive `(beg, end)` line
    /// numbers. The block must start at the first body line (leading
    /// whitespace tolerated ignores indentation) and its `END`
    /// line must come before the trimmed subtree end; anything after the
    /// `END` line must be blank. None for a plaintext or malformed entry.
    pub fn org_crypt_block_at(&self, h: usize) -> Option<(usize, usize)> {
        let (beg, end) = self.org_crypt_entry_range(h)?;
        if !is_begin_pgp(&self.line(beg)) {
            return None;
        }
        let close = (beg + 1..=end).find(|&l| is_end_pgp(&self.line(l)))?;
        if (close + 1..=end).any(|l| !self.line(l).trim().is_empty()) {
            return None;
        }
        Some((beg, close))
    }

    /// Whether heading `h`'s body is already an encrypted PGP block.
    pub fn org_crypt_is_encrypted(&self, h: usize) -> bool {
        self.org_crypt_block_at(h).is_some()
    }

    /// Headings selected by a tags matcher such as `"crypt"`, reusing the
    /// tags/properties matcher (`org_match_entries`, tags only).
    pub fn org_crypt_match_entries(&self, matcher: &str) -> Result<Vec<usize>, String> {
        self.org_match_entries(matcher, false)
    }

    /// Match headings that still need encryption: selected by `matcher`,
    /// with a non-empty body, and not already encrypted. Nested matches are
    /// reduced to the outermost heading, since encrypting the parent covers
    /// the subtree (the structural equivalent of excluding the crypt tag
    /// from inheritance).
    pub fn org_crypt_encryptable_entries(&self, matcher: &str) -> Result<Vec<usize>, String> {
        let hits = self.org_crypt_match_entries(matcher)?;
        let in_subtree = |outer: usize, inner: usize| {
            inner != outer && inner > outer && inner <= self.org_subtree_end(outer)
        };
        Ok(hits
            .iter()
            .filter(|&&h| {
                self.org_crypt_entry_range(h).is_some_and(|(beg, end)| {
                    // Already-encrypted entries only need work when edited;
                    // without the decrypt cache (stage 2) there is nothing
                    // to compare, so leave them alone.
                    !self.org_crypt_is_encrypted(h) && beg <= end
                })
            })
            .filter(|&&h| !hits.iter().any(|&o| in_subtree(o, h)))
            .copied()
            .collect())
    }

    /// Which key encrypts heading `h`, given the default key:
    /// - `None` default: always symmetric (a `CRYPTKEY` property is ignored).
    /// - otherwise the heading's `CRYPTKEY` property (inherited) wins over
    ///   the default; `"nil"` counts as empty, and an empty key means
    ///   symmetric.
    pub fn org_crypt_key_for_heading(&self, h: usize, default_key: Option<&str>) -> CryptKey {
        let Some(default) = default_key else {
            return CryptKey::Symmetric;
        };
        let prop = self
            .org_get_property(h, CRYPTKEY_PROPERTY, true)
            .map(|v| {
                if v.trim().eq_ignore_ascii_case("nil") {
                    String::new()
                } else {
                    v.trim().to_string()
                }
            })
            .unwrap_or_else(|| default.trim().to_string());
        if prop.is_empty() {
            CryptKey::Symmetric
        } else {
            CryptKey::Recipients(prop)
        }
    }

    /// The encryptable body of heading `h` as text: its entry-range lines
    /// joined with `\n`, no trailing newline. None when there is no body.
    pub fn org_crypt_body_text(&self, h: usize) -> Option<String> {
        let (beg, end) = self.org_crypt_entry_range(h)?;
        Some(
            (beg..=end)
                .map(|l| self.line(l))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Replace heading `h`'s body with `text`'s lines (no trailing newline
    /// in `text`). False when the entry has no body.
    pub fn org_crypt_set_body(&mut self, h: usize, text: &str) -> bool {
        let Some((beg, end)) = self.org_crypt_entry_range(h) else {
            return false;
        };
        let a = self.line_start_offset(beg);
        let (z, nl) = if end + 1 < self.line_count() {
            (self.line_start_offset(end + 1), true)
        } else {
            (self.line_end_offset(end), false)
        };
        let mut new = text.to_string();
        if nl {
            new.push('\n');
        }
        if self.text_offsets(a, z) != new {
            self.replace_offsets(a, z, &new);
        }
        true
    }

    /// Full buffer text with each heading's body replaced by its armor.
    /// `arms` maps heading lines to armored blocks (LF-separated, no
    /// trailing newline). Ranges are offset-spliced, so the result keeps
    /// the buffer's exact bytes (including a missing final newline) outside
    /// the replaced ranges. For saving while the buffer shows plaintext.
    pub fn org_crypt_save_text(&self, arms: &std::collections::HashMap<usize, String>) -> String {
        let mut reps: Vec<(usize, usize, &str, bool)> = arms
            .iter()
            .filter_map(|(&h, armor)| {
                let (beg, end) = self.org_crypt_entry_range(h)?;
                let a = self.line_start_offset(beg);
                let (z, nl) = if end + 1 < self.line_count() {
                    (self.line_start_offset(end + 1), true)
                } else {
                    (self.line_end_offset(end), false)
                };
                Some((a, z, armor.as_str(), nl))
            })
            .collect();
        reps.sort();
        let text = self.text.to_string();
        let byte = |c: usize| self.char_to_byte(c);
        let mut out = String::with_capacity(text.len());
        let mut cur = 0;
        for (a, z, armor, nl) in reps {
            let (ba, bz) = (byte(a), byte(z));
            out.push_str(&text[cur..ba]);
            out.push_str(armor);
            if nl {
                out.push('\n');
            }
            cur = bz;
        }
        out.push_str(&text[cur..]);
        out
    }
}

/// Maps the `org_crypt_key` setting to `org_crypt_key_for_heading`'s
/// default: unset behaves like default `""` (symmetric unless a
/// heading sets `CRYPTKEY`); the `"nil"` sentinel, stored for an explicit
/// Lua nil, behaves like nil (symmetric, always).
pub fn configured_default_key(setting: Option<&str>) -> Option<String> {
    match setting {
        None | Some("") => Some(String::new()),
        Some("nil") => None,
        Some(s) => Some(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(text: &str) -> Buffer {
        let mut b = Buffer::new("t.org");
        b.set_contents(text);
        b
    }

    const ARMOR: &str = "-----BEGIN PGP MESSAGE-----\nabc\n-----END PGP MESSAGE-----";

    #[test]
    fn content_start_skips_metadata() {
        let b = org(
            "* H :crypt:\nSCHEDULED: <2026-10-14 Wed>\n:PROPERTIES:\n:CRYPTKEY: k\n:END:\nbody\n",
        );
        assert_eq!(b.org_crypt_content_start(0), 5);
        assert_eq!(b.org_crypt_entry_range(0), Some((5, 5)));
    }

    #[test]
    fn empty_entry_has_no_range() {
        let b = org("* H :crypt:\n* Next\n");
        assert_eq!(b.org_crypt_entry_range(0), None);
        let b = org("* H :crypt:\n:PROPERTIES:\n:END:\n");
        assert_eq!(b.org_crypt_entry_range(0), None);
    }

    #[test]
    fn trailing_blanks_trimmed_and_subtree_covered() {
        let b = org("* H :crypt:\nbody\n\n** Child\nchild body\n\n\n* Next\n");
        assert_eq!(b.org_crypt_entry_range(0), Some((1, 4)));
    }

    #[test]
    fn a_blank_body_has_no_range() {
        let b = org("* H :crypt:\n\n\n* Next\n");
        assert_eq!(b.org_crypt_entry_range(0), None);
        assert_eq!(b.org_crypt_body_text(0), None);
    }

    #[test]
    fn detects_armor_block() {
        let b = org(&format!("* H :crypt:\n{ARMOR}\n* Next\n"));
        assert!(b.org_crypt_is_encrypted(0));
        assert_eq!(b.org_crypt_block_at(0), Some((1, 3)));
        assert!(!b.org_crypt_is_encrypted(1));
    }

    #[test]
    fn armor_tolerates_indentation() {
        let b =
            org("* H :crypt:\n  -----BEGIN PGP MESSAGE-----\n  abc\n  -----END PGP MESSAGE-----\n");
        assert!(b.org_crypt_is_encrypted(0));
    }

    #[test]
    fn rejects_malformed_blocks() {
        let b = org("* H :crypt:\n-----BEGIN PGP MESSAGE-----\nabc\n");
        assert!(!b.org_crypt_is_encrypted(0));
        let b = org("* H :crypt:\nhello\n");
        assert!(!b.org_crypt_is_encrypted(0));
        let b = org(&format!("* H :crypt:\n{ARMOR}\ntrailing\n"));
        assert!(!b.org_crypt_is_encrypted(0));
        // BEGIN not first (headline/properties stay plaintext, but body
        // must start with the block).
        let b = org(&format!("* H :crypt:\nnote\n{ARMOR}\n"));
        assert!(!b.org_crypt_is_encrypted(0));
    }

    #[test]
    fn selects_outermost_matches_only() {
        let b = org("* Top :crypt:\nbody\n** Child :crypt:\nchild\n* Other\ntext\n");
        assert_eq!(b.org_crypt_match_entries("crypt").unwrap(), [0, 2]);
        assert_eq!(b.org_crypt_encryptable_entries("crypt").unwrap(), [0]);
    }

    #[test]
    fn skips_encrypted_and_empty_entries() {
        let b = org(&format!(
            "* Done :crypt:\n{ARMOR}\n* Empty :crypt:\n* Plain :crypt:\ntext\n"
        ));
        assert_eq!(b.org_crypt_encryptable_entries("crypt").unwrap(), [5]);
    }

    #[test]
    fn key_selection() {
        let b =
            org("* A :crypt:\n:PROPERTIES:\n:CRYPTKEY: 0xABC\n:END:\nbody\n* B :crypt:\nbody\n");
        // Nil default: symmetric unconditionally, CRYPTKEY ignored.
        assert_eq!(b.org_crypt_key_for_heading(0, None), CryptKey::Symmetric);
        assert_eq!(b.org_crypt_key_for_heading(1, None), CryptKey::Symmetric);
        // Empty default: CRYPTKEY wins, otherwise symmetric.
        assert_eq!(
            b.org_crypt_key_for_heading(0, Some("")),
            CryptKey::Recipients("0xABC".into())
        );
        assert_eq!(
            b.org_crypt_key_for_heading(1, Some("")),
            CryptKey::Symmetric
        );
        // Default key used when no property.
        assert_eq!(
            b.org_crypt_key_for_heading(1, Some("0xDEF")),
            CryptKey::Recipients("0xDEF".into())
        );
        // CRYPTKEY "nil" counts as empty.
        let b = org("* A :crypt:\n:PROPERTIES:\n:CRYPTKEY: nil\n:END:\nbody\n");
        assert_eq!(
            b.org_crypt_key_for_heading(0, Some("0xDEF")),
            CryptKey::Symmetric
        );
    }

    #[test]
    fn cryptkey_inherited_from_parent() {
        let b = org("* P\n:PROPERTIES:\n:CRYPTKEY: 0xABC\n:END:\n** C :crypt:\nbody\n");
        assert_eq!(
            b.org_crypt_key_for_heading(4, Some("")),
            CryptKey::Recipients("0xABC".into())
        );
    }

    #[test]
    fn configured_default_key_mapping() {
        assert_eq!(configured_default_key(None), Some(String::new()));
        assert_eq!(configured_default_key(Some("")), Some(String::new()));
        assert_eq!(configured_default_key(Some("nil")), None);
        assert_eq!(configured_default_key(Some("0xA")), Some("0xA".into()));
    }

    #[test]
    fn body_roundtrip() {
        let mut b = org("* H :crypt:\nbody\nmore\n* Next\n");
        assert_eq!(b.org_crypt_body_text(0).as_deref(), Some("body\nmore"));
        assert!(b.org_crypt_set_body(0, "new\ntext"));
        assert_eq!(b.get_contents(), "* H :crypt:\nnew\ntext\n* Next\n");
        assert_eq!(b.org_crypt_body_text(3), None);
    }

    #[test]
    fn save_text_substitutes_armor() {
        let b = org("* H :crypt:\nbody\n* Next\ntail\n");
        let arms = std::collections::HashMap::from([(0, ARMOR.to_string())]);
        assert_eq!(
            b.org_crypt_save_text(&arms),
            format!("* H :crypt:\n{ARMOR}\n* Next\ntail\n")
        );
        // No trailing newline in the buffer: none in the output either.
        let b = org("* H :crypt:\nbody");
        let arms = std::collections::HashMap::from([(0, ARMOR.to_string())]);
        assert_eq!(
            b.org_crypt_save_text(&arms),
            format!("* H :crypt:\n{ARMOR}")
        );
    }
}
