// SPDX-License-Identifier: MIT OR Apache-2.0
//! Generate a reference for the plugin API from `wit/forge-editor.wit`.
//!
//! The WIT file is the source of truth for the plugin API, and its `///`
//! comments are the documentation (Part 4 of the documentation design). This is a
//! line-based reader rather than a full WIT parser: it tracks the current
//! `interface` and attaches each item's preceding `///` block to it. That is
//! enough to render a reference and to check that every item is documented.

/// One documented item: a record, enum, resource or function.
pub struct Item {
    /// `record`, `enum`, `flags`, `variant`, `resource` or `func`.
    pub kind: &'static str,
    /// The item's name.
    pub name: String,
    /// Any `@since x.y.z` tag found in its doc block.
    pub since: Option<String>,
    /// The item's doc lines, `@since` removed.
    pub doc: Vec<String>,
}

/// One `interface`, with its doc and items.
pub struct Interface {
    /// The interface name.
    pub name: String,
    /// The interface's doc lines.
    pub doc: Vec<String>,
    /// The interface's items, in source order.
    pub items: Vec<Item>,
}

/// A parsed WIT file.
pub struct Wit {
    /// The `package name@version;` line.
    pub package: String,
    /// The interfaces, in source order.
    pub interfaces: Vec<Interface>,
}

impl Wit {
    /// Parse the WIT subset the file uses.
    pub fn parse(source: &str) -> Wit {
        let mut package = String::new();
        let mut interfaces: Vec<Interface> = Vec::new();
        let mut doc: Vec<String> = Vec::new();

        for raw in source.lines() {
            let line = raw.trim();
            if let Some(rest) = line.strip_prefix("///") {
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                doc.push(rest.trim_end().to_string());
                continue;
            }
            if line.starts_with("//") {
                // A plain comment does not document the next item.
                doc.clear();
                continue;
            }
            if line.is_empty() {
                doc.clear();
                continue;
            }
            if package.is_empty() && line.starts_with("package ") {
                package = line.trim_end_matches(';').to_string();
                doc.clear();
                continue;
            }
            if let Some(name) = decl_name(line, "interface") {
                interfaces.push(Interface {
                    name: name.to_string(),
                    doc: take_doc(&mut doc),
                    items: Vec::new(),
                });
                continue;
            }
            if line == "}" {
                doc.clear();
                continue;
            }
            let Some((kind, name)) = item_head(line) else {
                doc.clear();
                continue;
            };
            if let Some(iface) = interfaces.last_mut() {
                iface.items.push(Item {
                    kind,
                    name: name.to_string(),
                    since: None,
                    doc: take_doc(&mut doc),
                });
            }
        }

        let mut wit = Wit {
            package,
            interfaces,
        };
        for iface in &mut wit.interfaces {
            for item in &mut iface.items {
                item.since = extract_since(&mut item.doc);
            }
        }
        wit
    }

    /// Render a Markdown reference for a whole WIT file.
    pub fn to_markdown(&self) -> String {
        let mut out = format!("# {}\n\n", self.package);
        out.push_str(
            "Generated from `wit/forge-editor.wit`. Each interface lists its \
             records, enums and functions; `@since` names the Forge version an \
             item arrived in.\n\n",
        );
        for iface in &self.interfaces {
            out.push_str(&format!("## interface `{}`\n\n", iface.name));
            if !iface.doc.is_empty() {
                out.push_str(&paragraph(&iface.doc));
                out.push('\n');
            }
            for item in &iface.items {
                let label = match item.kind {
                    "func" => format!("`{}`", item.name),
                    k => format!("{k} `{}`", item.name),
                };
                out.push_str(&format!("### {label}\n\n"));
                if let Some(since) = &item.since {
                    out.push_str(&format!("_Since {since}._\n\n"));
                }
                if item.doc.is_empty() {
                    out.push_str("_No documentation._\n\n");
                } else {
                    out.push_str(&paragraph(&item.doc));
                    out.push('\n');
                }
            }
        }
        out
    }

    /// The items with no doc comment, as `interface: item` strings.
    pub fn undocumented(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for iface in &self.interfaces {
            if iface.doc.is_empty() {
                problems.push(format!("{}: interface", iface.name));
            }
            for item in &iface.items {
                if item.doc.is_empty() {
                    problems.push(format!("{}: {} {}", iface.name, item.kind, item.name));
                }
            }
        }
        problems
    }
}

fn take_doc(doc: &mut Vec<String>) -> Vec<String> {
    std::mem::take(doc)
}

/// Join wrapped doc lines into Markdown: lines within a paragraph become one
/// wrapped line, blank lines separate paragraphs, and the result ends with a
/// newline.
fn paragraph(lines: &[String]) -> String {
    let mut paragraphs: Vec<Vec<&str>> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in lines {
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line.as_str());
        }
    }
    if !current.is_empty() {
        paragraphs.push(current);
    }
    let mut out = paragraphs
        .iter()
        .map(|p| p.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    out.push('\n');
    out
}

fn extract_since(doc: &mut Vec<String>) -> Option<String> {
    let mut since = None;
    doc.retain(|line| {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("@since") {
            since = Some(rest.trim().to_string());
            false
        } else {
            true
        }
    });
    while doc.last().is_some_and(|l| l.is_empty()) {
        doc.pop();
    }
    since
}

/// If `line` declares `kind name ... {`, return the kind and name.
fn decl_name<'a>(line: &'a str, kind: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kind)?.trim_start();
    if rest.starts_with('{') {
        return None;
    }
    let name = rest.split(|c: char| c.is_whitespace() || c == '{').next()?;
    (!name.is_empty()).then_some(name)
}

/// Classify a line as an item: a named record/enum/... or a function.
fn item_head(line: &str) -> Option<(&'static str, &str)> {
    for kind in ["record", "enum", "flags", "variant", "resource"] {
        if let Some(name) = decl_name(line, kind) {
            return Some((kind, name));
        }
    }
    // A function: `name: func(...)` or `name: static func(...)` or
    // `name: async func(...)`.
    let (name, rest) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest
        .strip_prefix("static ")
        .or_else(|| rest.strip_prefix("async "))
        .unwrap_or(rest);
    if rest.starts_with("func") {
        Some(("func", name))
    } else {
        None
    }
}

/// The WIT source, baked in at build time like the manual.
pub const WIT_SRC: &str = include_str!("../../../wit/forge-editor.wit");

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
/// A package.
package forge:test@0.1.0;

/// Values shared by the interface.
interface types {
    /// A place in a buffer.
    record position {
        line: u32,
    }

    /// No doc here.
    enum color {
        red,
        green,
    }
}

interface log {
    /// Write a message.
    /// @since 0.1.4
    log: func(message: string);

    undoc: func();
}
"#;

    #[test]
    fn parses_interfaces_items_and_since() {
        let wit = Wit::parse(SRC);
        assert_eq!(wit.package, "package forge:test@0.1.0");
        assert_eq!(wit.interfaces.len(), 2);
        assert_eq!(wit.interfaces[0].name, "types");
        assert_eq!(wit.interfaces[0].items.len(), 2);
        assert_eq!(wit.interfaces[0].items[0].kind, "record");
        assert_eq!(wit.interfaces[0].items[0].name, "position");
        let log = &wit.interfaces[1];
        assert_eq!(log.items[0].name, "log");
        assert_eq!(log.items[0].since.as_deref(), Some("0.1.4"));
    }

    #[test]
    fn finds_undocumented_items() {
        let wit = Wit::parse(SRC);
        let problems = wit.undocumented();
        assert!(problems.iter().any(|p| p.contains("func undoc")));
        assert!(!problems.iter().any(|p| p.contains("record position")));
        assert!(!problems.iter().any(|p| p.contains("func log")));
    }

    #[test]
    fn markdown_has_sections_and_since() {
        let md = Wit::parse(SRC).to_markdown();
        assert!(md.contains("## interface `types`"));
        assert!(md.contains("### record `position`"));
        assert!(md.contains("### `log`"));
        assert!(md.contains("_Since 0.1.4._"));
    }
}
