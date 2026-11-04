// SPDX-License-Identifier: MIT OR Apache-2.0
//! Citations: reading BibTeX files and formatting `[cite:@key]` the way
//! Org's `basic` export processor does.

/// One BibTeX entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibEntry {
    pub kind: String,
    pub key: String,
    pub fields: Vec<(String, String)>,
}

impl BibEntry {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// (surname, given names) of each author (or editor).
    pub fn authors(&self) -> Vec<(String, String)> {
        let Some(a) = self.get("author").or_else(|| self.get("editor")) else {
            return Vec::new();
        };
        split_and(a)
            .into_iter()
            .map(|p| match p.split_once(',') {
                Some((l, f)) => (l.trim().to_string(), f.trim().to_string()),
                None => {
                    let mut w: Vec<&str> = p.split_whitespace().collect();
                    let last = w.pop().unwrap_or("").to_string();
                    (last, w.join(" "))
                }
            })
            .collect()
    }

    pub fn year(&self) -> String {
        if let Some(y) = self.get("year") {
            return y.trim().to_string();
        }
        self.get("date")
            .map(|d| d.chars().take(4).collect())
            .unwrap_or_default()
    }

    /// "Smith", "Smith and Jones", "Smith et al.".
    pub fn author_short(&self) -> String {
        let a = self.authors();
        match a.len() {
            0 => self.get("title").unwrap_or(&self.key).to_string(),
            1 => a[0].0.clone(),
            2 => format!("{} and {}", a[0].0, a[1].0),
            _ => format!("{} et al.", a[0].0),
        }
    }

    /// "Smith, J., Jones, K."
    fn author_full(&self) -> String {
        let a = self.authors();
        let fmt: Vec<String> = a
            .iter()
            .map(|(l, f)| {
                let init: Vec<String> = f
                    .split(|c: char| c.is_whitespace() || c == '-')
                    .filter(|w| !w.is_empty())
                    .filter_map(|w| w.chars().next())
                    .map(|c| format!("{c}."))
                    .collect();
                if init.is_empty() {
                    l.clone()
                } else {
                    format!("{l}, {}", init.join(" "))
                }
            })
            .collect();
        match fmt.len() {
            0 => String::new(),
            1 => fmt[0].clone(),
            _ => format!(
                "{}, & {}",
                fmt[..fmt.len() - 1].join(", "),
                fmt[fmt.len() - 1]
            ),
        }
    }

    /// A one-line description: `Smith (2020) Title`.
    pub fn describe(&self) -> String {
        format!(
            "{} ({}) {}",
            self.author_short(),
            self.year(),
            self.get("title").unwrap_or("")
        )
        .trim()
        .to_string()
    }

    /// The reference-list line for this entry.
    pub fn reference(&self) -> String {
        let mut s = String::new();
        let au = self.author_full();
        if !au.is_empty() {
            s.push_str(&au);
            s.push(' ');
        }
        let y = self.year();
        if !y.is_empty() {
            s.push_str(&format!("({y}). "));
        }
        if let Some(t) = self.get("title") {
            s.push_str(t.trim_end_matches('.'));
            s.push_str(". ");
        }
        if let Some(j) = self
            .get("journal")
            .or_else(|| self.get("booktitle"))
            .or_else(|| self.get("journaltitle"))
        {
            s.push_str(j);
            if let Some(v) = self.get("volume") {
                s.push_str(&format!(", {v}"));
                if let Some(n) = self.get("number") {
                    s.push_str(&format!("({n})"));
                }
            }
            if let Some(p) = self.get("pages") {
                s.push_str(&format!(", {}", p.replace("--", "–")));
            }
            s.push_str(". ");
        } else if let Some(p) = self.get("publisher") {
            s.push_str(p);
            s.push_str(". ");
        }
        if let Some(d) = self.get("doi") {
            s.push_str(&format!("https://doi.org/{d}"));
        } else if let Some(u) = self.get("url") {
            s.push_str(u);
        }
        s.trim().to_string()
    }
}

fn split_and(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in s.split_whitespace() {
        if w == "and" {
            out.push(std::mem::take(&mut cur));
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn clean(v: &str) -> String {
    let mut s = v.replace(['{', '}'], "");
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.replace("\\&", "&")
        .replace("\\_", "_")
        .replace("\\'e", "é")
        .replace("\\\"o", "ö")
        .replace("\\\"u", "ü")
}

/// Entries of BibTeX source. `@comment`, `@string` and `@preamble` are
/// skipped.
pub fn parse_bib(src: &str) -> Vec<BibEntry> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] != '@' {
            i += 1;
            continue;
        }
        i += 1;
        let s = i;
        while i < c.len() && c[i].is_ascii_alphabetic() {
            i += 1;
        }
        let kind: String = c[s..i].iter().collect::<String>().to_lowercase();
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        let Some(&open) = c.get(i) else { break };
        if open != '{' && open != '(' {
            continue;
        }
        let close = if open == '{' { '}' } else { ')' };
        // The entry's extent, with balanced braces.
        let start = i + 1;
        let mut depth = 1;
        i += 1;
        while i < c.len() && depth > 0 {
            if c[i] == open {
                depth += 1;
            } else if c[i] == close {
                depth -= 1;
            }
            i += 1;
        }
        let body: String = c[start..i.saturating_sub(1).max(start)].iter().collect();
        if matches!(kind.as_str(), "comment" | "string" | "preamble") {
            continue;
        }
        let Some((key, rest)) = body.split_once(',') else {
            continue;
        };
        let fields = parse_fields(rest);
        out.push(BibEntry {
            kind,
            key: key.trim().to_string(),
            fields,
        });
    }
    out
}

fn parse_fields(s: &str) -> Vec<(String, String)> {
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < c.len() {
        while i < c.len() && (c[i].is_whitespace() || c[i] == ',') {
            i += 1;
        }
        let s0 = i;
        while i < c.len() && (c[i].is_alphanumeric() || matches!(c[i], '-' | '_' | ':')) {
            i += 1;
        }
        if i == s0 {
            i += 1;
            continue;
        }
        let name: String = c[s0..i].iter().collect::<String>().to_lowercase();
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        if c.get(i) != Some(&'=') {
            continue;
        }
        i += 1;
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        let mut val = String::new();
        // A value may be several pieces joined by `#`.
        loop {
            match c.get(i) {
                Some('{') => {
                    let mut depth = 0;
                    while i < c.len() {
                        if c[i] == '{' {
                            depth += 1;
                            if depth == 1 {
                                i += 1;
                                continue;
                            }
                        } else if c[i] == '}' {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        val.push(c[i]);
                        i += 1;
                    }
                }
                Some('"') => {
                    i += 1;
                    while i < c.len() && c[i] != '"' {
                        val.push(c[i]);
                        i += 1;
                    }
                    i += 1;
                }
                _ => {
                    while i < c.len() && !matches!(c[i], ',' | '#') && !c[i].is_whitespace() {
                        val.push(c[i]);
                        i += 1;
                    }
                }
            }
            while i < c.len() && c[i].is_whitespace() {
                i += 1;
            }
            if c.get(i) == Some(&'#') {
                i += 1;
                while i < c.len() && c[i].is_whitespace() {
                    i += 1;
                }
                continue;
            }
            break;
        }
        out.push((name, clean(&val)));
    }
    out
}

/// One `[cite:...]` reference.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiteRef {
    pub prefix: String,
    pub key: String,
    pub suffix: String,
}

/// A parsed citation object.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cite {
    pub style: String,
    pub variant: String,
    pub prefix: String,
    pub suffix: String,
    pub refs: Vec<CiteRef>,
}

fn is_key_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '_' | '-' | ':' | '.' | '?' | '$' | '%' | '&' | '+' | '<' | '>' | '~' | '/' | '!' | '*'
        )
}

/// Parse the text of `[cite...]`, from just after `[cite` to before `]`:
/// `/style/variant:global prefix;pre @key suf;...;global suffix`.
pub fn parse_cite(inner: &str) -> Option<Cite> {
    let (head, body) = inner.split_once(':')?;
    let mut cite = Cite::default();
    let mut it = head.trim_start_matches('/').split('/');
    cite.style = it.next().unwrap_or("").to_string();
    cite.variant = it.next().unwrap_or("").to_string();
    let parts: Vec<&str> = body.split(';').collect();
    for (n, p) in parts.iter().enumerate() {
        let chars: Vec<(usize, char)> = p.char_indices().collect();
        let at = chars.iter().position(|&(b, c)| {
            c == '@' && (b == 0 || !is_key_char(p[..b].chars().last().unwrap_or(' ')))
        });
        match at {
            Some(a) => {
                let b0 = chars[a].0;
                let mut end = p.len();
                for &(b, c) in &chars[a + 1..] {
                    if !is_key_char(c) {
                        end = b;
                        break;
                    }
                }
                let mut key = &p[b0 + 1..end];
                // A trailing punctuation mark isn't part of the key.
                while key.ends_with(['.', ':', '?', '!', '*', '/']) && key.len() > 1 {
                    key = &key[..key.len() - 1];
                    end -= 1;
                }
                if key.is_empty() {
                    return None;
                }
                cite.refs.push(CiteRef {
                    prefix: p[..b0].trim().to_string(),
                    key: key.to_string(),
                    suffix: p[end..].trim().trim_start_matches(',').trim().to_string(),
                });
            }
            None => {
                let t = p.trim().to_string();
                if cite.refs.is_empty() && n == 0 {
                    cite.prefix = t;
                } else if n == parts.len() - 1 {
                    cite.suffix = t;
                }
            }
        }
    }
    (!cite.refs.is_empty()).then_some(cite)
}

/// The text of a citation by the basic processor: "(Smith 2020; Lee
/// 2019, p. 3)", or for the `t` style "Smith (2020)".
pub fn render(c: &Cite, bib: &[BibEntry]) -> String {
    if c.style == "n" || c.style == "nocite" {
        return String::new();
    }
    let find = |k: &str| bib.iter().find(|e| e.key == k);
    let bare = c.variant.starts_with('b');
    let mut items = Vec::new();
    for r in &c.refs {
        let (a, y) = match find(&r.key) {
            Some(e) => (e.author_short(), e.year()),
            None => (r.key.clone(), String::new()),
        };
        let text = match c.style.as_str() {
            "a" | "author" => a,
            "na" | "noauthor" => y,
            "t" | "text" => {
                let inner = [y, r.suffix.clone()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let pre = if r.prefix.is_empty() {
                    String::new()
                } else {
                    format!("{} ", r.prefix)
                };
                if inner.is_empty() {
                    format!("{pre}{a}")
                } else {
                    format!("{pre}{a} ({inner})")
                }
            }
            "l" | "locators" => r.suffix.clone(),
            _ => {
                let mut s = String::new();
                if !r.prefix.is_empty() {
                    s.push_str(&r.prefix);
                    s.push(' ');
                }
                s.push_str(&a);
                if !y.is_empty() {
                    s.push(' ');
                    s.push_str(&y);
                }
                if !r.suffix.is_empty() {
                    s.push_str(", ");
                    s.push_str(&r.suffix);
                }
                s
            }
        };
        items.push(text);
    }
    let sep = "; ";
    let mut body = items.join(sep);
    if matches!(c.style.as_str(), "t" | "text") {
        // Already carries its own parentheses.
    } else if !bare {
        if !c.prefix.is_empty() {
            body = format!("{} {body}", c.prefix);
        }
        if !c.suffix.is_empty() {
            body = format!("{body} {}", c.suffix);
        }
        body = format!("({body})");
    }
    body
}

/// The reference list for the cited keys, sorted by author and year.
pub fn bibliography(cited: &[String], bib: &[BibEntry]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String, String)> = cited
        .iter()
        .filter_map(|k| bib.iter().find(|e| &e.key == k))
        .map(|e| {
            (
                format!("{}{}", e.author_short().to_lowercase(), e.year()),
                e.key.clone(),
                e.reference(),
            )
        })
        .collect();
    v.sort();
    v.dedup_by(|a, b| a.1 == b.1);
    v.into_iter().map(|(_, k, r)| (k, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = r#"
@comment{ignored}
@article{smith2020,
  author = {Smith, John and Lee, Kim},
  title = {A {Great} Result},
  journal = "Journal of Things",
  year = 2020, volume = {3}, number = {2}, pages = {1--9},
}
@book{jones19, author = {Ann Jones}, title = {Books}, publisher = {Pub}, year = {2019}}
"#;

    #[test]
    fn parses_bibtex() {
        let b = parse_bib(BIB);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].key, "smith2020");
        assert_eq!(b[0].get("title"), Some("A Great Result"));
        assert_eq!(b[0].year(), "2020");
        assert_eq!(b[0].author_short(), "Smith and Lee");
        assert_eq!(b[1].authors(), vec![("Jones".into(), "Ann".into())]);
        assert_eq!(b[1].get("publisher"), Some("Pub"));
    }

    #[test]
    fn parses_citations() {
        let c = parse_cite("/t:see @a p. 3;@b-2").unwrap();
        assert_eq!(c.style, "t");
        assert_eq!(c.refs.len(), 2);
        assert_eq!(c.refs[0].prefix, "see");
        assert_eq!(c.refs[0].key, "a");
        assert_eq!(c.refs[0].suffix, "p. 3");
        assert_eq!(c.refs[1].key, "b-2");
        let c = parse_cite(":Global;@x;end").unwrap();
        assert_eq!((c.prefix.as_str(), c.suffix.as_str()), ("Global", "end"));
        assert!(parse_cite(":nothing here").is_none());
        // An address isn't a key.
        assert_eq!(parse_cite(":me@x.org;@k").unwrap().refs.len(), 1);
    }

    #[test]
    fn renders_basic_style() {
        let b = parse_bib(BIB);
        let c = parse_cite(":@smith2020;@jones19, p. 4").unwrap();
        assert_eq!(render(&c, &b), "(Smith and Lee 2020; Jones 2019, p. 4)");
        let c = parse_cite("/t:@smith2020").unwrap();
        assert_eq!(render(&c, &b), "Smith and Lee (2020)");
        let c = parse_cite("/a:@jones19").unwrap();
        assert_eq!(render(&c, &b), "(Jones)");
        let c = parse_cite("/na:@jones19").unwrap();
        assert_eq!(render(&c, &b), "(2019)");
        assert_eq!(render(&parse_cite("/n:@jones19").unwrap(), &b), "");
        assert_eq!(render(&parse_cite(":@nokey").unwrap(), &b), "(nokey)");
    }

    #[test]
    fn builds_a_reference_list() {
        let b = parse_bib(BIB);
        let l = bibliography(
            &["smith2020".into(), "jones19".into(), "smith2020".into()],
            &b,
        );
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].0, "jones19");
        assert_eq!(
            l[1].1,
            "Smith, J., & Lee, K. (2020). A Great Result. Journal of Things, 3(2), 1–9."
        );
    }
}
