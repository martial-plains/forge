// SPDX-License-Identifier: MIT OR Apache-2.0
//! OpenDocument Text export: a zip with `content.xml`, `styles.xml`,
//! `meta.xml`, a manifest and any pictures it could read.

use super::ast::*;
use super::cite;
use super::common::*;
use super::entities;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::fmt::Write;

pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            // XML allows only tab, newline and carriage return below
            // space: the others would make the file invalid.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0""#;

struct O<'a> {
    doc: &'a Doc,
    anchors: Anchors,
    o: Options,
    st: &'a Settings,
    pictures: RefCell<Vec<(String, Vec<u8>)>>,
    seen_fn: RefCell<HashSet<usize>>,
    tables: Cell<usize>,
    figs: Cell<usize>,
    frames: Cell<usize>,
}

fn styles_xml() -> String {
    let mut s = String::new();
    let _ = write!(
        s,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-styles {NS} office:version=\"1.2\">\n<office:styles>\n"
    );
    s.push_str(r##"<style:default-style style:family="paragraph"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.2cm"/><style:text-properties style:font-name="Liberation Serif" fo:font-size="11pt"/></style:default-style>
<style:style style:name="Standard" style:family="paragraph" style:class="text"/>
<style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard" style:class="text"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.25cm"/></style:style>
<style:style style:name="Title" style:family="paragraph" style:parent-style-name="Standard" style:class="chapter"><style:paragraph-properties fo:text-align="center" fo:margin-bottom="0.3cm"/><style:text-properties fo:font-size="22pt" fo:font-weight="bold"/></style:style>
<style:style style:name="Subtitle" style:family="paragraph" style:parent-style-name="Standard" style:class="chapter"><style:paragraph-properties fo:text-align="center"/><style:text-properties fo:font-size="14pt" fo:font-style="italic"/></style:style>
<style:style style:name="Heading" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:class="text"><style:paragraph-properties fo:margin-top="0.4cm" fo:margin-bottom="0.2cm" fo:keep-with-next="always"/><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="Heading_20_1" style:display-name="Heading 1" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="1" style:class="text"><style:text-properties fo:font-size="18pt"/></style:style>
<style:style style:name="Heading_20_2" style:display-name="Heading 2" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="2" style:class="text"><style:text-properties fo:font-size="15pt"/></style:style>
<style:style style:name="Heading_20_3" style:display-name="Heading 3" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="3" style:class="text"><style:text-properties fo:font-size="13pt"/></style:style>
<style:style style:name="Heading_20_4" style:display-name="Heading 4" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="4" style:class="text"><style:text-properties fo:font-size="12pt"/></style:style>
<style:style style:name="Heading_20_5" style:display-name="Heading 5" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="5" style:class="text"><style:text-properties fo:font-size="11pt"/></style:style>
<style:style style:name="Heading_20_6" style:display-name="Heading 6" style:family="paragraph" style:parent-style-name="Heading" style:default-outline-level="6" style:class="text"><style:text-properties fo:font-size="11pt"/></style:style>
<style:style style:name="Preformatted_20_Text" style:display-name="Preformatted Text" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.2cm" fo:background-color="#f2f2f2" fo:padding="0.1cm"/><style:text-properties style:font-name="Liberation Mono" fo:font-size="10pt"/></style:style>
<style:style style:name="Quotations" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-left="1cm" fo:margin-right="1cm" fo:margin-top="0cm" fo:margin-bottom="0.25cm"/></style:style>
<style:style style:name="Verse" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-left="1cm"/></style:style>
<style:style style:name="Centered" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="center"/></style:style>
<style:style style:name="Horizontal_20_Line" style:display-name="Horizontal Line" style:family="paragraph" style:parent-style-name="Standard" style:class="html"><style:paragraph-properties fo:margin-top="0cm" fo:margin-bottom="0.25cm" fo:border-bottom="0.5pt solid #000000" fo:padding="0cm"/></style:style>
<style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"/>
<style:style style:name="Table_20_Heading" style:display-name="Table Heading" style:family="paragraph" style:parent-style-name="Table_20_Contents" style:class="extra"><style:paragraph-properties fo:text-align="center"/><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="Caption" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:paragraph-properties fo:margin-top="0.1cm" fo:margin-bottom="0.2cm"/><style:text-properties fo:font-style="italic"/></style:style>
<style:style style:name="Footnote" style:family="paragraph" style:parent-style-name="Standard" style:class="extra"><style:text-properties fo:font-size="10pt"/></style:style>
<style:style style:name="Contents_20_Heading" style:display-name="Contents Heading" style:family="paragraph" style:parent-style-name="Heading"><style:text-properties fo:font-size="16pt"/></style:style>
<style:style style:name="Contents_20_1" style:display-name="Contents 1" style:family="paragraph" style:parent-style-name="Standard"/>
<style:style style:name="Contents_20_2" style:display-name="Contents 2" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-left="0.5cm"/></style:style>
<style:style style:name="Contents_20_3" style:display-name="Contents 3" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-left="1cm"/></style:style>
<style:style style:name="Internet_20_link" style:display-name="Internet link" style:family="text"><style:text-properties fo:color="#000080" style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color"/></style:style>
<text:notes-configuration text:note-class="footnote" style:num-format="1" text:start-value="0" text:footnotes-position="page" text:start-numbering-at="document"/>
"##);
    s.push_str("</office:styles>\n</office:document-styles>\n");
    s
}

fn content_styles() -> &'static str {
    r##"<office:automatic-styles>
<style:style style:name="T_bold" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="T_italic" style:family="text"><style:text-properties fo:font-style="italic"/></style:style>
<style:style style:name="T_underline" style:family="text"><style:text-properties style:text-underline-style="solid" style:text-underline-width="auto" style:text-underline-color="font-color"/></style:style>
<style:style style:name="T_strike" style:family="text"><style:text-properties style:text-line-through-style="solid"/></style:style>
<style:style style:name="T_code" style:family="text"><style:text-properties style:font-name="Liberation Mono" fo:font-size="10pt"/></style:style>
<style:style style:name="T_sub" style:family="text"><style:text-properties style:text-position="sub 58%"/></style:style>
<style:style style:name="T_sup" style:family="text"><style:text-properties style:text-position="super 58%"/></style:style>
<style:style style:name="T_ts" style:family="text"><style:text-properties fo:color="#808080"/></style:style>
<style:style style:name="Tbl" style:family="table"><style:table-properties style:width="17cm" table:align="margins"/></style:style>
<style:style style:name="Cell" style:family="table-cell"><style:table-cell-properties fo:padding="0.1cm" fo:border="0.5pt solid #000000"/></style:style>
<style:style style:name="Fr" style:family="graphic"><style:graphic-properties style:wrap="none" style:vertical-pos="top" style:vertical-rel="baseline" style:horizontal-pos="center" style:horizontal-rel="paragraph"/></style:style>
<text:list-style style:name="LBullet"><text:list-level-style-bullet text:level="1" text:bullet-char="•"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="0.9cm" fo:text-indent="-0.5cm" fo:margin-left="0.9cm"/></style:list-level-properties></text:list-level-style-bullet><text:list-level-style-bullet text:level="2" text:bullet-char="◦"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="1.8cm" fo:text-indent="-0.5cm" fo:margin-left="1.8cm"/></style:list-level-properties></text:list-level-style-bullet><text:list-level-style-bullet text:level="3" text:bullet-char="▪"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="2.7cm" fo:text-indent="-0.5cm" fo:margin-left="2.7cm"/></style:list-level-properties></text:list-level-style-bullet></text:list-style>
<text:list-style style:name="LNumber"><text:list-level-style-number text:level="1" style:num-suffix="." style:num-format="1"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="0.9cm" fo:text-indent="-0.6cm" fo:margin-left="0.9cm"/></style:list-level-properties></text:list-level-style-number><text:list-level-style-number text:level="2" style:num-suffix="." style:num-format="1"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="1.8cm" fo:text-indent="-0.6cm" fo:margin-left="1.8cm"/></style:list-level-properties></text:list-level-style-number><text:list-level-style-number text:level="3" style:num-suffix="." style:num-format="1"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" text:list-tab-stop-position="2.7cm" fo:text-indent="-0.6cm" fo:margin-left="2.7cm"/></style:list-level-properties></text:list-level-style-number></text:list-style>
</office:automatic-styles>
"##
}

/// Text with spaces, tabs and newlines as ODF wants them.
fn spaced(t: &str) -> String {
    let mut s = String::new();
    let mut run = 0usize;
    for c in t.chars() {
        if c == ' ' {
            run += 1;
            continue;
        }
        flush_spaces(&mut s, &mut run, false);
        match c {
            '\n' => s.push_str("<text:line-break/>"),
            '\t' => s.push_str("<text:tab/>"),
            c => s.push_str(&esc(&c.to_string())),
        }
    }
    flush_spaces(&mut s, &mut run, true);
    s
}

fn flush_spaces(s: &mut String, run: &mut usize, _end: bool) {
    match *run {
        0 => {}
        1 => s.push(' '),
        n => {
            s.push(' ');
            let _ = write!(s, "<text:s text:c=\"{}\"/>", n - 1);
        }
    }
    *run = 0;
}

pub fn export(doc: &Doc, st: &Settings) -> Vec<u8> {
    let root = sections(doc);
    let mut anchors = Anchors::new(&root);
    anchors.add_nodes(&doc.nodes);
    let o = doc.meta.options.clone();
    let c = O {
        doc,
        anchors,
        o: o.clone(),
        st,
        pictures: RefCell::new(Vec::new()),
        seen_fn: RefCell::new(HashSet::new()),
        tables: Cell::new(0),
        figs: Cell::new(0),
        frames: Cell::new(0),
    };
    let mut body = String::new();
    if o.title && !doc.meta.title.is_empty() {
        let _ = writeln!(
            body,
            "<text:p text:style-name=\"Title\">{}</text:p>",
            c.inline(&super::parse::inline_of(&doc.meta.title, &o))
        );
        if !doc.meta.subtitle.is_empty() {
            let _ = writeln!(
                body,
                "<text:p text:style-name=\"Subtitle\">{}</text:p>",
                esc(&doc.meta.subtitle)
            );
        }
        let mut by = Vec::new();
        if o.author && !doc.meta.author.is_empty() {
            by.push(esc(&doc.meta.author));
        }
        let d = date_of(doc);
        if o.date && !d.is_empty() {
            by.push(esc(&d));
        }
        if !by.is_empty() {
            let _ = writeln!(
                body,
                "<text:p text:style-name=\"Centered\">{}</text:p>",
                by.join(" — ")
            );
        }
    }
    let toc = match (&o.toc, toc_keyword(doc)) {
        (_, Some((d, _))) => Some(d),
        (Some(d), None) => Some(*d),
        _ => None,
    };
    if let Some(d) = toc
        && !root.children.is_empty()
    {
        let mut all = Vec::new();
        root.all(&mut all);
        body.push_str("<text:table-of-content text:style-name=\"Standard\" text:protected=\"true\" text:name=\"Table of Contents\"><text:table-of-content-source text:outline-level=\"10\"><text:index-title-template text:style-name=\"Contents_20_Heading\">Table of Contents</text:index-title-template></text:table-of-content-source><text:index-body><text:index-title text:style-name=\"Standard\" text:name=\"Table of Contents_Head\"><text:p text:style-name=\"Contents_20_Heading\">Table of Contents</text:p></text:index-title>\n");
        for s in all.iter().filter(|s| s.depth <= d.min(o.headline_levels)) {
            let num = if s.numbered(&o) {
                format!("{} ", s.number_string())
            } else {
                String::new()
            };
            let _ = writeln!(
                body,
                "<text:p text:style-name=\"Contents_20_{}\"><text:a xlink:type=\"simple\" xlink:href=\"#{}\">{}{}</text:a></text:p>",
                s.depth.min(3),
                esc(&s.id),
                num,
                esc(&s.title_plain())
            );
        }
        body.push_str("</text:index-body></text:table-of-content>\n");
    }
    body.push_str(&c.nodes(&root.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
    for s in &root.children {
        body.push_str(&c.section(s));
    }
    let mut content = String::new();
    let _ = write!(
        content,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-content {NS} office:version=\"1.2\">\n<office:font-face-decls><style:font-face style:name=\"Liberation Serif\" svg:font-family=\"'Liberation Serif'\"/><style:font-face style:name=\"Liberation Mono\" svg:font-family=\"'Liberation Mono'\"/></office:font-face-decls>\n"
    );
    content.push_str(content_styles());
    content.push_str("<office:body><office:text>\n");
    content.push_str(&body);
    content.push_str("</office:text></office:body></office:document-content>\n");
    let title = plain(&super::parse::inline_of(&doc.meta.title, &o));
    let meta = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document-meta {NS} office:version=\"1.2\"><office:meta><meta:generator>Forge Org Mode</meta:generator><dc:title>{}</dc:title><dc:description>{}</dc:description><meta:initial-creator>{}</meta:initial-creator><dc:creator>{}</dc:creator><meta:keyword>{}</meta:keyword><dc:language>{}</dc:language></office:meta></office:document-meta>\n",
        esc(&title),
        esc(&doc.meta.description),
        esc(&doc.meta.author),
        esc(&doc.meta.author),
        esc(&doc.meta.keywords),
        esc(if doc.meta.language.is_empty() {
            "en"
        } else {
            &doc.meta.language
        })
    );
    let pics = c.pictures.into_inner();
    let mut manifest = String::new();
    let _ = write!(
        manifest,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<manifest:manifest {NS} manifest:version=\"1.2\">\n<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"1.2\" manifest:media-type=\"application/vnd.oasis.opendocument.text\"/>\n<manifest:file-entry manifest:full-path=\"content.xml\" manifest:media-type=\"text/xml\"/>\n<manifest:file-entry manifest:full-path=\"styles.xml\" manifest:media-type=\"text/xml\"/>\n<manifest:file-entry manifest:full-path=\"meta.xml\" manifest:media-type=\"text/xml\"/>\n"
    );
    for (name, _) in &pics {
        let mt = match name.rsplit('.').next().unwrap_or("") {
            "png" => "image/png",
            "gif" => "image/gif",
            _ => "image/jpeg",
        };
        let _ = writeln!(
            manifest,
            "<manifest:file-entry manifest:full-path=\"{name}\" manifest:media-type=\"{mt}\"/>"
        );
    }
    manifest.push_str("</manifest:manifest>\n");
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        (
            "mimetype".into(),
            b"application/vnd.oasis.opendocument.text".to_vec(),
        ),
        ("content.xml".into(), content.into_bytes()),
        ("styles.xml".into(), styles_xml().into_bytes()),
        ("meta.xml".into(), meta.into_bytes()),
        ("META-INF/manifest.xml".into(), manifest.into_bytes()),
    ];
    entries.extend(pics);
    zip(&entries)
}

impl<'a> O<'a> {
    fn section(&self, s: &Section) -> String {
        let h = s.heading.unwrap();
        let mut out = String::new();
        let mut title = String::new();
        if s.numbered(&self.o) {
            let _ = write!(title, "{} ", s.number_string());
        }
        if let Some(k) = &h.todo
            && self.o.todo
        {
            let _ = write!(title, "{} ", esc(k));
        }
        title.push_str(&self.inline(&h.title));
        if self.o.tags && !h.tags.is_empty() {
            let _ = write!(title, " :{}:", esc(&h.tags.join(":")));
        }
        if s.depth > self.o.headline_levels {
            let _ = write!(
                out,
                "<text:list text:style-name=\"LBullet\"><text:list-item><text:p text:style-name=\"Text_20_body\">{title}</text:p>"
            );
            out.push_str(&self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
            for c in &s.children {
                if c.depth > MAX_SECTION_DEPTH {
                    continue;
                }
                out.push_str(&self.section(c));
            }
            out.push_str("</text:list-item></text:list>\n");
            return out;
        }
        let lvl = s.depth.min(6);
        let _ = writeln!(
            out,
            "<text:h text:style-name=\"Heading_20_{lvl}\" text:outline-level=\"{lvl}\"><text:bookmark-start text:name=\"{0}\"/>{title}<text:bookmark-end text:name=\"{0}\"/></text:h>",
            esc(&s.id)
        );
        if self.o.timestamps && !h.planning.is_empty() {
            let p: Vec<String> = h.planning.iter().map(|(k, t)| format!("<text:span text:style-name=\"T_bold\">{k}:</text:span> <text:span text:style-name=\"T_ts\">{}</text:span>", esc(t))).collect();
            let _ = writeln!(
                out,
                "<text:p text:style-name=\"Text_20_body\">{}</text:p>",
                p.join(" ")
            );
        }
        out.push_str(&self.nodes(&s.body.iter().map(|n| (*n).clone()).collect::<Vec<_>>()));
        for c in &s.children {
            if c.depth > MAX_SECTION_DEPTH {
                continue;
            }
            out.push_str(&self.section(c));
        }
        out
    }

    fn nodes(&self, ns: &[Node]) -> String {
        ns.iter().map(|n| self.node(n)).collect()
    }

    fn caption(&self, n: &Node) -> Option<String> {
        n.aff
            .caption
            .as_ref()
            .map(|c| self.inline(&super::parse::inline_of(c, &self.o)))
    }

    fn pre(&self, text: &str) -> String {
        format!(
            "<text:p text:style-name=\"Preformatted_20_Text\">{}</text:p>\n",
            spaced(text)
        )
    }

    fn node(&self, n: &Node) -> String {
        match &n.block {
            Block::Heading(_) => String::new(),
            Block::Para(p) => self.para(n, p),
            Block::List(l) => self.list(l),
            Block::Table(t) => self.table(n, t),
            Block::Src { params, text, .. } => {
                let ex = exports_of(params);
                if ex == "results" || ex == "none" {
                    return String::new();
                }
                self.pre(text)
            }
            Block::Example(t) => self.pre(t),
            Block::Verse(t) => {
                format!("<text:p text:style-name=\"Verse\">{}</text:p>\n", spaced(t))
            }
            Block::Quote(c) => self.nodes(c).replace(
                "text:style-name=\"Text_20_body\"",
                "text:style-name=\"Quotations\"",
            ),
            Block::Center(c) => self.nodes(c).replace(
                "text:style-name=\"Text_20_body\"",
                "text:style-name=\"Centered\"",
            ),
            Block::Special { content, .. } | Block::Drawer { content, .. } => self.nodes(content),
            Block::Export { backend, text } => {
                if backend == "odt" {
                    format!("{text}\n")
                } else {
                    String::new()
                }
            }
            Block::Rule => "<text:p text:style-name=\"Horizontal_20_Line\"/>\n".into(),
            Block::LatexEnv(t) => self.pre(&entities::latex_to_unicode(t)),
            Block::Keyword { key, .. } => {
                if key == "PRINT_BIBLIOGRAPHY" {
                    let list = cite::bibliography(&self.doc.cited, &self.doc.bib);
                    if list.is_empty() {
                        return String::new();
                    }
                    let mut s = String::from(
                        "<text:h text:style-name=\"Heading_20_1\" text:outline-level=\"1\">Bibliography</text:h>\n",
                    );
                    for (_, r) in list {
                        let _ = writeln!(
                            s,
                            "<text:p text:style-name=\"Text_20_body\">{}</text:p>",
                            esc(&r)
                        );
                    }
                    s
                } else {
                    String::new()
                }
            }
        }
    }

    fn picture(&self, path: &str) -> Option<String> {
        let read = self.st.images.as_ref()?;
        let bytes = read(path)?;
        let (w, h) = image_size(&bytes)?;
        let ext = path.rsplit('.').next().unwrap_or("png").to_lowercase();
        let ext = if ext == "jpeg" {
            "jpg".to_string()
        } else {
            ext
        };
        if !matches!(ext.as_str(), "png" | "jpg" | "gif") {
            return None;
        }
        let k = self.frames.get() + 1;
        self.frames.set(k);
        let name = format!("Pictures/img{k}.{ext}");
        self.pictures.borrow_mut().push((name.clone(), bytes));
        let mut wcm = w as f64 * 2.54 / 96.0;
        let mut hcm = h as f64 * 2.54 / 96.0;
        if wcm > 16.0 {
            hcm *= 16.0 / wcm;
            wcm = 16.0;
        }
        Some(format!(
            "<draw:frame draw:style-name=\"Fr\" draw:name=\"img{k}\" text:anchor-type=\"as-char\" svg:width=\"{wcm:.2}cm\" svg:height=\"{hcm:.2}cm\"><draw:image xlink:href=\"{name}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\"/></draw:frame>"
        ))
    }

    fn para(&self, n: &Node, p: &[Inline]) -> String {
        let only: Vec<&Inline> = p
            .iter()
            .filter(|i| !matches!(i, Inline::Text(t) if t.trim().is_empty()))
            .collect();
        if only.len() == 1
            && let Inline::Link { target, .. } = only[0]
            && let LinkKind::Image(path) = self.anchors.classify(target)
            && let Some(frame) = self.picture(&path)
        {
            let mut s = format!("<text:p text:style-name=\"Centered\">{frame}</text:p>\n");
            if let Some(c) = self.caption(n) {
                self.figs.set(self.figs.get() + 1);
                let _ = writeln!(
                    s,
                    "<text:p text:style-name=\"Caption\">Figure {}: {c}</text:p>",
                    self.figs.get()
                );
            }
            return s;
        }
        format!(
            "<text:p text:style-name=\"Text_20_body\">{}</text:p>\n",
            self.inline(p).trim_end()
        )
    }

    fn list(&self, l: &List) -> String {
        let style = if l.ordered { "LNumber" } else { "LBullet" };
        let mut s = format!("<text:list text:style-name=\"{style}\">\n");
        for it in &l.items {
            let cb = match it.checkbox {
                Some('X') => "☑ ",
                Some('-') => "▣ ",
                Some(_) => "☐ ",
                None => "",
            };
            let tag = it
                .tag
                .as_ref()
                .map(|t| {
                    format!(
                        "<text:span text:style-name=\"T_bold\">{}</text:span> – ",
                        self.inline(t)
                    )
                })
                .unwrap_or_default();
            s.push_str("<text:list-item>");
            let mut first = true;
            for n in &it.content {
                if first && let Block::Para(p) = &n.block {
                    let _ = write!(
                        s,
                        "<text:p text:style-name=\"Text_20_body\">{cb}{tag}{}</text:p>",
                        self.inline(p).trim_end()
                    );
                } else {
                    if first {
                        let _ = write!(
                            s,
                            "<text:p text:style-name=\"Text_20_body\">{cb}{tag}</text:p>"
                        );
                    }
                    s.push_str(&self.node(n));
                }
                first = false;
            }
            if it.content.is_empty() {
                let _ = write!(
                    s,
                    "<text:p text:style-name=\"Text_20_body\">{cb}{tag}</text:p>"
                );
            }
            s.push_str("</text:list-item>\n");
        }
        s.push_str("</text:list>\n");
        s
    }

    fn table(&self, n: &Node, t: &Table) -> String {
        let cols = t.align.len().max(1);
        let k = self.tables.get() + 1;
        self.tables.set(k);
        let mut s = String::new();
        if let Some(c) = self.caption(n) {
            let _ = writeln!(
                s,
                "<text:p text:style-name=\"Caption\">Table {k}: {c}</text:p>"
            );
        }
        let _ = writeln!(
            s,
            "<table:table table:name=\"Table{k}\" table:style-name=\"Tbl\">\n<table:table-column table:number-columns-repeated=\"{cols}\"/>"
        );
        let mut head = t.has_header;
        let mut opened_head = false;
        for r in &t.rows {
            match r {
                Row::Rule => {
                    if opened_head {
                        s.push_str("</table:table-header-rows>\n");
                        opened_head = false;
                    }
                    head = false;
                }
                Row::Cells(c) => {
                    if head && !opened_head {
                        s.push_str("<table:table-header-rows>\n");
                        opened_head = true;
                    }
                    s.push_str("<table:table-row>");
                    for j in 0..cols.max(c.len()) {
                        let style = if head {
                            "Table_20_Heading"
                        } else {
                            "Table_20_Contents"
                        };
                        let cell = c.get(j).map(|x| self.inline(x)).unwrap_or_default();
                        let _ = write!(
                            s,
                            "<table:table-cell table:style-name=\"Cell\" office:value-type=\"string\"><text:p text:style-name=\"{style}\">{cell}</text:p></table:table-cell>"
                        );
                    }
                    s.push_str("</table:table-row>\n");
                }
            }
        }
        if opened_head {
            s.push_str("</table:table-header-rows>\n");
        }
        s.push_str("</table:table>\n");
        s
    }

    fn inline(&self, v: &[Inline]) -> String {
        let mut s = String::new();
        let span = |style: &str, inner: String| {
            format!("<text:span text:style-name=\"{style}\">{inner}</text:span>")
        };
        for i in v {
            match i {
                Inline::Text(t) => s.push_str(&spaced(&if self.o.special_strings {
                    special_strings(t)
                } else {
                    t.clone()
                })),
                Inline::Bold(x) => s.push_str(&span("T_bold", self.inline(x))),
                Inline::Italic(x) => s.push_str(&span("T_italic", self.inline(x))),
                Inline::Underline(x) => s.push_str(&span("T_underline", self.inline(x))),
                Inline::Strike(x) => s.push_str(&span("T_strike", self.inline(x))),
                Inline::Code(t) | Inline::Verbatim(t) => s.push_str(&span("T_code", esc(t))),
                Inline::Link { target, desc } => s.push_str(&self.link(target, desc.as_deref())),
                Inline::FnRef { number, .. } => {
                    if !self.o.footnotes {
                        continue;
                    }
                    if self.seen_fn.borrow_mut().insert(*number) {
                        let content: String = self
                            .doc
                            .footnotes
                            .iter()
                            .find(|f| f.number == *number)
                            .map(|f| {
                                f.content
                                    .iter()
                                    .map(|n| match &n.block {
                                        Block::Para(p) => format!(
                                            "<text:p text:style-name=\"Footnote\">{}</text:p>",
                                            self.inline(p).trim_end()
                                        ),
                                        _ => self.node(n),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let _ = write!(
                            s,
                            "<text:note text:id=\"ftn{number}\" text:note-class=\"footnote\"><text:note-citation>{number}</text:note-citation><text:note-body>{content}</text:note-body></text:note>"
                        );
                    } else {
                        let _ = write!(
                            s,
                            "<text:note-ref text:note-class=\"footnote\" text:reference-format=\"value\" text:ref-name=\"ftn{number}\">{number}</text:note-ref>"
                        );
                    }
                }
                Inline::Cite(c) => s.push_str(&esc(&cite::render(c, &self.doc.bib))),
                Inline::Latex { text, .. } => {
                    s.push_str(&span("T_italic", esc(&entities::latex_to_unicode(text))))
                }
                Inline::Entity(n) => {
                    if let Some(e) = entities::find(n) {
                        s.push_str(&esc(e.4));
                    }
                }
                Inline::Timestamp { text, .. } => {
                    if self.o.timestamps {
                        s.push_str(&span("T_ts", esc(text)));
                    }
                }
                Inline::Sub(x) => s.push_str(&span("T_sub", self.inline(x))),
                Inline::Sup(x) => s.push_str(&span("T_sup", self.inline(x))),
                Inline::Break => s.push_str("<text:line-break/>"),
                Inline::Target(t) => {
                    let _ = write!(s, "<text:bookmark text:name=\"org-target-{}\"/>", slug(t));
                }
                Inline::Cookie(c) => s.push_str(&span("T_code", esc(c))),
            }
        }
        s
    }

    fn link(&self, target: &str, desc: Option<&[Inline]>) -> String {
        let a = |href: &str, inner: String| {
            format!(
                "<text:a xlink:type=\"simple\" xlink:href=\"{}\" text:style-name=\"Internet_20_link\">{inner}</text:a>",
                esc(href)
            )
        };
        match self.anchors.classify(target) {
            LinkKind::External(u) => {
                let d = desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&u));
                a(&u, d)
            }
            LinkKind::Internal(id) => {
                let d = desc
                    .map(|d| self.inline(d))
                    .unwrap_or_else(|| esc(target.trim_start_matches(['*', '#'])));
                a(&format!("#{id}"), d)
            }
            LinkKind::File { path, .. } => {
                let d = desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&path));
                a(&path, d)
            }
            LinkKind::Image(p) => match desc {
                None => self.picture(&p).unwrap_or_else(|| a(&p, esc(&p))),
                Some(d) => a(&p, self.inline(d)),
            },
            LinkKind::Unresolved(t) => desc.map(|d| self.inline(d)).unwrap_or_else(|| esc(&t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::parse::parse;
    use std::rc::Rc;

    fn files(src: &str, st: &Settings) -> Vec<(String, String)> {
        let z = export(&parse(src, &|_| None), st);
        unzip(&z)
            .into_iter()
            .map(|(n, d)| (n, String::from_utf8_lossy(&d).to_string()))
            .collect()
    }

    fn content(src: &str) -> String {
        files(src, &Settings::default())
            .into_iter()
            .find(|(n, _)| n == "content.xml")
            .unwrap()
            .1
    }

    #[test]
    fn archive_layout_has_mimetype_first() {
        let f = files("#+TITLE: T\n* A\nx\n", &Settings::default());
        assert_eq!(f[0].0, "mimetype");
        assert_eq!(f[0].1, "application/vnd.oasis.opendocument.text");
        let names: Vec<&str> = f.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            names.contains(&"content.xml")
                && names.contains(&"styles.xml")
                && names.contains(&"meta.xml")
                && names.contains(&"META-INF/manifest.xml")
        );
    }

    #[test]
    fn xml_is_well_formed_enough() {
        for (n, x) in files(
            "#+TITLE: A & B <c>\n* H :t:\ntext *b* /i/ [[https://x.org][l]] & <\n- a\n  - b\n| x | y |\n|---+---|\n| 1 | 2 |\n#+BEGIN_SRC sh\n  ls  -l\n#+END_SRC\nfoot[fn:1]\n\n[fn:1] N.\n",
            &Settings::default(),
        ) {
            if !n.ends_with(".xml") {
                continue;
            }
            let mut stack: Vec<String> = Vec::new();
            let mut rest = x.as_str();
            while let Some(i) = rest.find('<') {
                let j = rest[i..].find('>').unwrap() + i;
                let tag = &rest[i + 1..j];
                rest = &rest[j + 1..];
                if tag.starts_with('?') || tag.ends_with('/') || tag.starts_with('!') {
                    continue;
                }
                if let Some(name) = tag.strip_prefix('/') {
                    assert_eq!(stack.pop().as_deref(), Some(name), "{n}");
                } else {
                    stack.push(tag.split_whitespace().next().unwrap().to_string());
                }
            }
            assert!(stack.is_empty(), "{n}: {stack:?}");
        }
    }

    #[test]
    fn content_covers_the_elements() {
        let c = content(
            "#+TITLE: T\n#+OPTIONS: toc:nil\n* One\ntext *b* x^2 [[https://x.org][l]]\n- a\n1. b\n| H |\n|---|\n| 1 |\n#+BEGIN_SRC sh\nls\n  -l\n#+END_SRC\nn[fn:1]\n\n[fn:1] Note.\n",
        );
        assert!(c.contains("<text:p text:style-name=\"Title\">T</text:p>"));
        assert!(
            c.contains("text:style-name=\"Heading_20_1\" text:outline-level=\"1\"")
                && c.contains("1 One")
        );
        assert!(c.contains("<text:span text:style-name=\"T_bold\">b</text:span>"));
        assert!(c.contains("<text:span text:style-name=\"T_sup\">2</text:span>"));
        assert!(c.contains("xlink:href=\"https://x.org\""));
        assert!(
            c.contains("<text:list text:style-name=\"LBullet\">")
                && c.contains("<text:list text:style-name=\"LNumber\">")
        );
        assert!(c.contains("<table:table-header-rows>") && c.contains("<table:table-cell"));
        assert!(
            c.contains("ls<text:line-break/> <text:s text:c=\"1\"/>-l"),
            "{c}"
        );
        assert!(
            c.contains("<text:note text:id=\"ftn1\"")
                && c.contains(
                    "<text:note-body><text:p text:style-name=\"Footnote\">Note.</text:p>"
                )
        );
    }

    #[test]
    fn pictures_are_embedded_when_readable() {
        // A 2x3 PNG header is enough for the size.
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&2u32.to_be_bytes());
        png.extend_from_slice(&3u32.to_be_bytes());
        png.extend_from_slice(&[8, 2, 0, 0, 0]);
        let st = Settings {
            images: Some(Rc::new(move |p: &str| (p == "a.png").then(|| png.clone()))),
            ..Default::default()
        };
        let f = files("[[file:a.png]]\n", &st);
        assert!(f.iter().any(|(n, _)| n == "Pictures/img1.png"));
        let c = &f.iter().find(|(n, _)| n == "content.xml").unwrap().1;
        assert!(c.contains("<draw:image xlink:href=\"Pictures/img1.png\""));
        let m = &f
            .iter()
            .find(|(n, _)| n == "META-INF/manifest.xml")
            .unwrap()
            .1;
        assert!(m.contains("Pictures/img1.png"));
    }
}
