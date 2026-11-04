// SPDX-License-Identifier: MIT OR Apache-2.0
//! Org entities (`\alpha`, `\rarr`, ...): the names and what each becomes
//! in LaTeX, HTML, plain ASCII and UTF-8.

/// (name, LaTeX, HTML, ASCII, UTF-8). LaTeX text starting with `$` is
/// math, written as is.
pub const ENTITIES: &[(&str, &str, &str, &str, &str)] = &[
    ("Alpha", "A", "&Alpha;", "Alpha", "Α"),
    ("alpha", "$\\alpha$", "&alpha;", "alpha", "α"),
    ("Beta", "B", "&Beta;", "Beta", "Β"),
    ("beta", "$\\beta$", "&beta;", "beta", "β"),
    ("Gamma", "$\\Gamma$", "&Gamma;", "Gamma", "Γ"),
    ("gamma", "$\\gamma$", "&gamma;", "gamma", "γ"),
    ("Delta", "$\\Delta$", "&Delta;", "Delta", "Δ"),
    ("delta", "$\\delta$", "&delta;", "delta", "δ"),
    ("epsilon", "$\\epsilon$", "&epsilon;", "epsilon", "ε"),
    ("zeta", "$\\zeta$", "&zeta;", "zeta", "ζ"),
    ("eta", "$\\eta$", "&eta;", "eta", "η"),
    ("Theta", "$\\Theta$", "&Theta;", "Theta", "Θ"),
    ("theta", "$\\theta$", "&theta;", "theta", "θ"),
    ("iota", "$\\iota$", "&iota;", "iota", "ι"),
    ("kappa", "$\\kappa$", "&kappa;", "kappa", "κ"),
    ("Lambda", "$\\Lambda$", "&Lambda;", "Lambda", "Λ"),
    ("lambda", "$\\lambda$", "&lambda;", "lambda", "λ"),
    ("mu", "$\\mu$", "&mu;", "mu", "μ"),
    ("nu", "$\\nu$", "&nu;", "nu", "ν"),
    ("Xi", "$\\Xi$", "&Xi;", "Xi", "Ξ"),
    ("xi", "$\\xi$", "&xi;", "xi", "ξ"),
    ("Pi", "$\\Pi$", "&Pi;", "Pi", "Π"),
    ("pi", "$\\pi$", "&pi;", "pi", "π"),
    ("rho", "$\\rho$", "&rho;", "rho", "ρ"),
    ("Sigma", "$\\Sigma$", "&Sigma;", "Sigma", "Σ"),
    ("sigma", "$\\sigma$", "&sigma;", "sigma", "σ"),
    ("tau", "$\\tau$", "&tau;", "tau", "τ"),
    ("Upsilon", "$\\Upsilon$", "&Upsilon;", "Upsilon", "Υ"),
    ("upsilon", "$\\upsilon$", "&upsilon;", "upsilon", "υ"),
    ("Phi", "$\\Phi$", "&Phi;", "Phi", "Φ"),
    ("phi", "$\\phi$", "&phi;", "phi", "φ"),
    ("chi", "$\\chi$", "&chi;", "chi", "χ"),
    ("Psi", "$\\Psi$", "&Psi;", "Psi", "Ψ"),
    ("psi", "$\\psi$", "&psi;", "psi", "ψ"),
    ("Omega", "$\\Omega$", "&Omega;", "Omega", "Ω"),
    ("omega", "$\\omega$", "&omega;", "omega", "ω"),
    ("rarr", "$\\rightarrow$", "&rarr;", "->", "→"),
    ("to", "$\\to$", "&rarr;", "->", "→"),
    ("rightarrow", "$\\rightarrow$", "&rarr;", "->", "→"),
    ("larr", "$\\leftarrow$", "&larr;", "<-", "←"),
    ("leftarrow", "$\\leftarrow$", "&larr;", "<-", "←"),
    ("uarr", "$\\uparrow$", "&uarr;", "^", "↑"),
    ("darr", "$\\downarrow$", "&darr;", "v", "↓"),
    ("harr", "$\\leftrightarrow$", "&harr;", "<->", "↔"),
    ("leftrightarrow", "$\\leftrightarrow$", "&harr;", "<->", "↔"),
    ("rArr", "$\\Rightarrow$", "&rArr;", "=>", "⇒"),
    ("Rightarrow", "$\\Rightarrow$", "&rArr;", "=>", "⇒"),
    ("lArr", "$\\Leftarrow$", "&lArr;", "<=", "⇐"),
    ("Leftarrow", "$\\Leftarrow$", "&lArr;", "<=", "⇐"),
    ("hArr", "$\\Leftrightarrow$", "&hArr;", "<=>", "⇔"),
    ("Leftrightarrow", "$\\Leftrightarrow$", "&hArr;", "<=>", "⇔"),
    ("mapsto", "$\\mapsto$", "&#x21A6;", "|->", "↦"),
    ("ldots", "\\ldots{}", "&hellip;", "...", "…"),
    ("hellip", "\\ldots{}", "&hellip;", "...", "…"),
    ("dots", "\\dots{}", "&hellip;", "...", "…"),
    ("mdash", "---", "&mdash;", "---", "—"),
    ("ndash", "--", "&ndash;", "--", "–"),
    ("nbsp", "~", "&nbsp;", " ", "\u{a0}"),
    ("shy", "\\-", "&shy;", "", "\u{ad}"),
    ("copy", "\\copyright{}", "&copy;", "(c)", "©"),
    ("reg", "\\textregistered{}", "&reg;", "(r)", "®"),
    ("trade", "\\texttrademark{}", "&trade;", "TM", "™"),
    ("deg", "$^{\\circ}$", "&deg;", "degrees", "°"),
    ("pm", "$\\pm$", "&plusmn;", "+-", "±"),
    ("mp", "$\\mp$", "&#x2213;", "-+", "∓"),
    ("times", "$\\times$", "&times;", "*", "×"),
    ("div", "$\\div$", "&divide;", "/", "÷"),
    ("cdot", "$\\cdot$", "&sdot;", "*", "⋅"),
    ("leq", "$\\leq$", "&le;", "<=", "≤"),
    ("le", "$\\le$", "&le;", "<=", "≤"),
    ("geq", "$\\geq$", "&ge;", ">=", "≥"),
    ("ge", "$\\ge$", "&ge;", ">=", "≥"),
    ("neq", "$\\neq$", "&ne;", "/=", "≠"),
    ("ne", "$\\ne$", "&ne;", "/=", "≠"),
    ("approx", "$\\approx$", "&asymp;", "~", "≈"),
    ("equiv", "$\\equiv$", "&equiv;", "==", "≡"),
    ("sim", "$\\sim$", "&sim;", "~", "∼"),
    ("infty", "$\\infty$", "&infin;", "infinity", "∞"),
    ("sum", "$\\sum$", "&sum;", "sum", "∑"),
    ("prod", "$\\prod$", "&prod;", "prod", "∏"),
    ("int", "$\\int$", "&int;", "integral", "∫"),
    (
        "partial",
        "$\\partial$",
        "&part;",
        "[partial differential]",
        "∂",
    ),
    ("nabla", "$\\nabla$", "&nabla;", "[nabla]", "∇"),
    ("sqrt", "$\\surd$", "&radic;", "[square root]", "√"),
    ("forall", "$\\forall$", "&forall;", "[for all]", "∀"),
    ("exists", "$\\exists$", "&exist;", "[there exists]", "∃"),
    ("in", "$\\in$", "&isin;", "[element of]", "∈"),
    ("notin", "$\\notin$", "&notin;", "[not an element of]", "∉"),
    ("subset", "$\\subset$", "&sub;", "[subset of]", "⊂"),
    ("supset", "$\\supset$", "&sup;", "[superset of]", "⊃"),
    ("cup", "$\\cup$", "&cup;", "[union]", "∪"),
    ("cap", "$\\cap$", "&cap;", "[intersection]", "∩"),
    ("emptyset", "$\\emptyset$", "&empty;", "[empty set]", "∅"),
    ("wedge", "$\\wedge$", "&and;", "[logical and]", "∧"),
    ("vee", "$\\vee$", "&or;", "[logical or]", "∨"),
    ("neg", "$\\neg$", "&not;", "[not]", "¬"),
    ("therefore", "$\\therefore$", "&there4;", "[therefore]", "∴"),
    ("aleph", "$\\aleph$", "&alefsym;", "aleph", "ℵ"),
    ("hbar", "$\\hbar$", "&#x210F;", "hbar", "ℏ"),
    ("ell", "$\\ell$", "&ell;", "ell", "ℓ"),
    ("angle", "$\\angle$", "&ang;", "[angle]", "∠"),
    ("star", "$\\star$", "&star;", "*", "⋆"),
    ("bullet", "\\textbullet{}", "&bull;", "*", "•"),
    ("dagger", "\\textdagger{}", "&dagger;", "[dagger]", "†"),
    (
        "ddagger",
        "\\textdaggerdbl{}",
        "&Dagger;",
        "[doubledagger]",
        "‡",
    ),
    ("sect", "\\S", "&sect;", "section", "§"),
    ("para", "\\P{}", "&para;", "paragraph", "¶"),
    ("pound", "\\pounds{}", "&pound;", "pound", "£"),
    ("euro", "\\texteuro{}", "&euro;", "EUR", "€"),
    ("yen", "\\textyen{}", "&yen;", "yen", "¥"),
    ("cent", "\\textcent{}", "&cent;", "cent", "¢"),
    ("laquo", "\\guillemotleft{}", "&laquo;", "<<", "«"),
    ("raquo", "\\guillemotright{}", "&raquo;", ">>", "»"),
    ("iexcl", "!`", "&iexcl;", "!", "¡"),
    ("iquest", "?`", "&iquest;", "?", "¿"),
    ("check", "\\checkmark", "&checkmark;", "[checkmark]", "✓"),
    (
        "checkmark",
        "\\checkmark",
        "&checkmark;",
        "[checkmark]",
        "✓",
    ),
    ("frac12", "\\textonehalf{}", "&frac12;", "1/2", "½"),
    ("frac14", "\\textonequarter{}", "&frac14;", "1/4", "¼"),
    ("frac34", "\\textthreequarters{}", "&frac34;", "3/4", "¾"),
    ("ae", "\\ae{}", "&aelig;", "ae", "æ"),
    ("AE", "\\AE{}", "&AElig;", "AE", "Æ"),
    ("szlig", "\\ss{}", "&szlig;", "ss", "ß"),
    ("oslash", "\\o{}", "&oslash;", "o", "ø"),
    ("Oslash", "\\O{}", "&Oslash;", "O", "Ø"),
    ("aacute", "\\'{a}", "&aacute;", "a", "á"),
    ("eacute", "\\'{e}", "&eacute;", "e", "é"),
    ("egrave", "\\`{e}", "&egrave;", "e", "è"),
    ("uuml", "\\\"{u}", "&uuml;", "ue", "ü"),
    ("ouml", "\\\"{o}", "&ouml;", "oe", "ö"),
    ("auml", "\\\"{a}", "&auml;", "ae", "ä"),
    ("ntilde", "\\~{n}", "&ntilde;", "n", "ñ"),
    ("ccedil", "\\c{c}", "&ccedil;", "c", "ç"),
    ("amp", "\\&", "&amp;", "&", "&"),
    ("lt", "<", "&lt;", "<", "<"),
    ("gt", ">", "&gt;", ">", ">"),
    ("quot", "\"", "&quot;", "\"", "\""),
    ("lbrace", "\\{", "&#123;", "{", "{"),
    ("rbrace", "\\}", "&#125;", "}", "}"),
    ("vert", "\\vert{}", "&vert;", "|", "|"),
    ("backslash", "\\textbackslash{}", "&bsol;", "\\", "\\"),
];

/// An entity as (name, LaTeX, HTML, ASCII, UTF-8), borrowed from
/// `'static` storage.
pub type Entity = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

/// The `org-entities-user` table: entities added at runtime, searched
/// before [`ENTITIES`]. Names are leaked, so the table is `'static`.
static USER: std::sync::LazyLock<std::sync::RwLock<Vec<Entity>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(Vec::new()));

/// Add (or replace) a user entity, Org's `org-entities-user`. Only the
/// name, UTF-8 form and the per-backend renderings are kept; a form left
/// empty falls back to the UTF-8 text.
pub fn add_user(name: &str, latex: &str, html: &str, ascii: &str, utf8: &str) {
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let e: Entity = (leak(name), leak(latex), leak(html), leak(ascii), leak(utf8));
    if let Ok(mut t) = USER.write() {
        if let Some(slot) = t.iter_mut().find(|x| x.0 == e.0) {
            *slot = e;
        } else {
            t.push(e);
        }
    }
}

/// Forget every user entity (the startup default).
pub fn clear_user() {
    if let Ok(mut t) = USER.write() {
        t.clear();
    }
}

/// The entity called `name`: a user entity first, then the built-in table.
pub fn find(name: &str) -> Option<Entity> {
    if let Ok(t) = USER.read()
        && let Some(e) = t.iter().find(|e| e.0 == name)
    {
        return Some(*e);
    }
    ENTITIES.iter().find(|e| e.0 == name).copied()
}

/// Replace `\name` entities by their UTF-8 text (for display).
pub fn pretty(s: &str) -> String {
    let mut out = String::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\\' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j > i + 1 {
                let name: String = b[i + 1..j].iter().collect();
                // `\alpha{}` and `\alpha ` both end the name.
                if let Some(e) = find(&name) {
                    out.push_str(e.4);
                    i = j;
                    if b.get(i) == Some(&'{') && b.get(i + 1) == Some(&'}') {
                        i += 2;
                    }
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// Plain-text (UTF-8) rendering of simple LaTeX math: Greek and symbol
/// commands, `^`/`_` as super/subscript digits and letters where there
/// are such characters, `\frac{a}{b}` as `a⁄b`, `\sqrt{x}` as `√x`.
pub fn latex_to_unicode(src: &str) -> String {
    let s = src
        .trim()
        .trim_start_matches("\\(")
        .trim_end_matches("\\)")
        .trim_start_matches("\\[")
        .trim_end_matches("\\]")
        .trim_matches('$');
    let mut out = String::new();
    let c: Vec<char> = s.chars().collect();
    render_math(&c, &mut 0, &mut out, false);
    out.trim().to_string()
}

fn sup_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        'x' => 'ˣ',
        'a' => 'ᵃ',
        'b' => 'ᵇ',
        'c' => 'ᶜ',
        'd' => 'ᵈ',
        'e' => 'ᵉ',
        'k' => 'ᵏ',
        'm' => 'ᵐ',
        'p' => 'ᵖ',
        't' => 'ᵗ',
        _ => return None,
    })
}

fn sub_char(ch: char) -> Option<char> {
    Some(match ch {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'x' => 'ₓ',
        'm' => 'ₘ',
        'p' => 'ₚ',
        't' => 'ₜ',
        _ => return None,
    })
}

/// One argument: a `{group}` or a single character or command.
fn arg(c: &[char], i: &mut usize) -> String {
    while c.get(*i) == Some(&' ') {
        *i += 1;
    }
    let mut out = String::new();
    match c.get(*i) {
        Some('{') => {
            *i += 1;
            let mut inner = String::new();
            let mut depth = 1;
            while let Some(&ch) = c.get(*i) {
                *i += 1;
                if ch == '{' {
                    depth += 1;
                } else if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                inner.push(ch);
            }
            let ic: Vec<char> = inner.chars().collect();
            render_math(&ic, &mut 0, &mut out, false);
        }
        Some('\\') => {
            let start = *i;
            *i += 1;
            while c.get(*i).is_some_and(|x| x.is_ascii_alphabetic()) {
                *i += 1;
            }
            let t: Vec<char> = c[start..*i].to_vec();
            render_math(&t, &mut 0, &mut out, false);
        }
        Some(&ch) => {
            *i += 1;
            out.push(ch);
        }
        None => {}
    }
    out
}

fn script(text: &str, up: bool) -> String {
    let conv: Option<String> = text
        .chars()
        .map(|ch| if up { sup_char(ch) } else { sub_char(ch) })
        .collect();
    match conv {
        Some(s) => s,
        None => format!(
            "{}{}",
            if up { "^" } else { "_" },
            if text.chars().count() > 1 {
                format!("({text})")
            } else {
                text.to_string()
            }
        ),
    }
}

fn render_math(c: &[char], i: &mut usize, out: &mut String, _nested: bool) {
    while *i < c.len() {
        let ch = c[*i];
        match ch {
            '\\' => {
                let start = *i + 1;
                let mut j = start;
                while c.get(j).is_some_and(|x| x.is_ascii_alphabetic()) {
                    j += 1;
                }
                if j == start {
                    match c.get(start) {
                        Some(',') | Some(';') | Some(':') | Some('!') | Some(' ') => out.push(' '),
                        Some('\\') => out.push('\n'),
                        Some(&o) => out.push(o),
                        None => {}
                    }
                    *i = (start + 1).min(c.len());
                    continue;
                }
                let name: String = c[start..j].iter().collect();
                *i = j;
                match name.as_str() {
                    "frac" | "dfrac" | "tfrac" => {
                        let a = arg(c, i);
                        let b = arg(c, i);
                        let wrap = |s: String| {
                            if s.chars().count() > 1 {
                                format!("({s})")
                            } else {
                                s
                            }
                        };
                        out.push_str(&format!("{}⁄{}", wrap(a), wrap(b)));
                    }
                    "sqrt" => {
                        let a = arg(c, i);
                        out.push('√');
                        out.push_str(&if a.chars().count() > 1 {
                            format!("({a})")
                        } else {
                            a
                        });
                    }
                    "text" | "mathrm" | "mathbf" | "mathit" | "mathcal" | "mathbb"
                    | "operatorname" | "mbox" => {
                        let a = arg(c, i);
                        out.push_str(&a);
                    }
                    "left" | "right" => {
                        if c.get(*i) == Some(&'.') {
                            *i += 1;
                        }
                    }
                    "cdots" | "ldots" | "dots" => out.push('…'),
                    "quad" | "qquad" => out.push_str("  "),
                    "le" | "leq" => out.push('≤'),
                    "ge" | "geq" => out.push('≥'),
                    "ne" | "neq" => out.push('≠'),
                    "sin" | "cos" | "tan" | "log" | "ln" | "exp" | "lim" | "max" | "min"
                    | "det" | "sec" | "csc" | "cot" => {
                        out.push_str(&name);
                    }
                    "pm" => out.push('±'),
                    "otimes" => out.push('⊗'),
                    "oplus" => out.push('⊕'),
                    "circ" => out.push('∘'),
                    "langle" => out.push('⟨'),
                    "rangle" => out.push('⟩'),
                    "mid" => out.push('∣'),
                    "lbrace" => out.push('{'),
                    "rbrace" => out.push('}'),
                    "setminus" => out.push('∖'),
                    "subseteq" => out.push('⊆'),
                    "supseteq" => out.push('⊇'),
                    "land" => out.push('∧'),
                    "lor" => out.push('∨'),
                    "lnot" => out.push('¬'),
                    "Re" => out.push('ℜ'),
                    "Im" => out.push('ℑ'),
                    "varepsilon" => out.push('ε'),
                    "vartheta" => out.push('ϑ'),
                    "varphi" => out.push('φ'),
                    "begin" | "end" => {
                        let _ = arg(c, i);
                    }
                    other => match find(other) {
                        Some(e) => out.push_str(e.4),
                        None => {
                            out.push('\\');
                            out.push_str(other);
                        }
                    },
                }
            }
            '^' | '_' => {
                *i += 1;
                let a = arg(c, i);
                out.push_str(&script(&a, ch == '^'));
            }
            '{' | '}' => *i += 1,
            '&' => {
                out.push_str("  ");
                *i += 1;
            }
            '~' => {
                out.push(' ');
                *i += 1;
            }
            _ => {
                out.push(ch);
                *i += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_prettify() {
        assert_eq!(pretty(r"\alpha + \beta\to x"), "α + β→ x");
        assert_eq!(pretty(r"\ldots{} ok"), "… ok");
        assert_eq!(pretty(r"\notanentity"), r"\notanentity");
    }

    #[test]
    fn user_entities_are_found_first_and_replaceable() {
        clear_user();
        assert!(find("forge-logo").is_none());
        add_user("forge-logo", r"\Forge", "&Forge;", "Forge", "🔨");
        assert_eq!(find("forge-logo").unwrap().4, "🔨");
        assert_eq!(find("forge-logo").unwrap().1, r"\Forge");
        // A new definition replaces the first.
        add_user("forge-logo", "F", "F", "F", "F");
        assert_eq!(find("forge-logo").unwrap().4, "F");
        clear_user();
        assert!(find("forge-logo").is_none());
    }

    #[test]
    fn math_to_unicode() {
        assert_eq!(latex_to_unicode(r"$x^2 + y_1$"), "x² + y₁");
        assert_eq!(latex_to_unicode(r"\(\alpha \leq \frac{1}{2}\)"), "α ≤ 1⁄2");
        assert_eq!(latex_to_unicode(r"\sqrt{x}"), "√x");
        assert_eq!(latex_to_unicode(r"e^{i\pi}"), "e^(iπ)");
    }
}
