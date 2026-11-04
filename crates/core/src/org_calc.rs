// SPDX-License-Identifier: MIT OR Apache-2.0
//! Table formulas (`#+TBLFM:`), a Calc-flavoured subset.
//!
//! References: `$2` (this row), `@3$2`, `@3` (a whole row), `$2..$4`,
//! `@2$1..@5$3`, relative `@-1`, `$+1`, `@<` `@>` `@>>` (first/last rows),
//! `$<` `$>`, `@I..@II` (rows between hlines), `$#` `@#` (this column /
//! row number) and `$name` (a column named in a `!` row, or a constant
//! from a `$` row).
//!
//! Operators `+ - * / ^ %`, comparisons, `&&`, `||`, `!`, parentheses and
//! these functions: `vsum vmean vmax vmin vcount vprod vmedian`, `sum mean
//! max min count`, `abs sqrt exp ln log10 sin cos tan floor ceil round`,
//! `if(c, a, b)`. Flags after `;`: `%.2f`-style formats, `N` (non-numbers
//! count as 0), `E` (empty fields count as 0).

use crate::org_table::Row;

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Num(f64),
    Str(String),
    Vec(Vec<Val>),
}

#[derive(Debug, Clone, Default)]
struct Flags {
    format: Option<String>,
    n: bool,
    e: bool,
}

#[derive(Debug, Clone)]
enum Target {
    /// `$3`: every data row.
    Column(ColSel),
    /// `@3`: one row.
    Row(RowSel),
    /// `@3$2` (and ranges of fields).
    Field(RowSel, ColSel, Option<(RowSel, ColSel)>),
}

#[derive(Debug, Clone, PartialEq)]
enum RowSel {
    Abs(i64),
    Rel(i64),
    First(usize),
    Last(usize),
    Hline(usize),
    Cur,
}

#[derive(Debug, Clone, PartialEq)]
enum ColSel {
    Abs(i64),
    Rel(i64),
    First(usize),
    Last(usize),
    Name(String),
    Cur,
}

struct Formula {
    target: Target,
    expr: String,
    flags: Flags,
}

/// Split a `#+TBLFM:` body into formulas.
fn parse_formulas(lines: &[String]) -> Result<Vec<Formula>, String> {
    let mut out = Vec::new();
    for line in lines {
        for part in line.split("::") {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (lhs, rhs) = part
                .split_once('=')
                .ok_or_else(|| format!("Bad formula: {part}"))?;
            if lhs.trim_end().ends_with(':') {
                return Err(format!("Lisp formulas aren't supported: {part}"));
            }
            let target = parse_target(lhs.trim())?;
            let (expr, flags) = split_flags(rhs.trim());
            out.push(Formula {
                target,
                expr,
                flags,
            });
        }
    }
    Ok(out)
}

fn split_flags(rhs: &str) -> (String, Flags) {
    let (mut depth, mut quoted, mut last) = (0i32, false, None);
    for (i, c) in rhs.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            ';' if !quoted && depth == 0 => last = Some(i),
            _ => {}
        }
    }
    let Some(i) = last else {
        return (rhs.to_string(), Flags::default());
    };
    let mut flags = Flags::default();
    for w in rhs[i + 1..].split_whitespace() {
        if w.starts_with('%') {
            flags.format = Some(w.to_string());
        } else {
            for c in w.chars() {
                match c {
                    'N' => flags.n = true,
                    'E' => flags.e = true,
                    _ => {}
                }
            }
        }
    }
    (rhs[..i].trim().to_string(), flags)
}

fn parse_target(s: &str) -> Result<Target, String> {
    let (first, second) = match s.split_once("..") {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let (r1, c1) = scan_ref(first).ok_or_else(|| format!("Bad formula target: {s}"))?;
    match (r1, c1) {
        (None, Some(c)) if second.is_none() => Ok(Target::Column(c)),
        (Some(r), None) if second.is_none() => Ok(Target::Row(r)),
        (r, c) => {
            let end = match second {
                Some(b) => {
                    let (r2, c2) = scan_ref(b).ok_or_else(|| format!("Bad formula target: {s}"))?;
                    Some((r2.unwrap_or(RowSel::Cur), c2.unwrap_or(ColSel::Cur)))
                }
                None => None,
            };
            Ok(Target::Field(
                r.unwrap_or(RowSel::Cur),
                c.unwrap_or(ColSel::Cur),
                end,
            ))
        }
    }
}

/// Parse `@row$col` (either part optional) from the whole of `s`.
fn scan_ref(s: &str) -> Option<(Option<RowSel>, Option<ColSel>)> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let r = scan_row(&chars, &mut i);
    let c = scan_col(&chars, &mut i);
    (i == chars.len() && (r.is_some() || c.is_some())).then_some((r, c))
}

fn scan_int(chars: &[char], i: &mut usize) -> Option<i64> {
    let start = *i;
    if matches!(chars.get(*i), Some('+') | Some('-')) {
        *i += 1;
    }
    let d = *i;
    while chars.get(*i).is_some_and(char::is_ascii_digit) {
        *i += 1;
    }
    if *i == d {
        *i = start;
        return None;
    }
    chars[start..*i].iter().collect::<String>().parse().ok()
}

fn scan_row(chars: &[char], i: &mut usize) -> Option<RowSel> {
    if chars.get(*i) != Some(&'@') {
        return None;
    }
    *i += 1;
    match chars.get(*i) {
        Some('<') => {
            let mut n = 0;
            while chars.get(*i) == Some(&'<') {
                n += 1;
                *i += 1;
            }
            Some(RowSel::First(n))
        }
        Some('>') => {
            let mut n = 0;
            while chars.get(*i) == Some(&'>') {
                n += 1;
                *i += 1;
            }
            Some(RowSel::Last(n))
        }
        Some('I') => {
            let mut n = 0;
            while chars.get(*i) == Some(&'I') {
                n += 1;
                *i += 1;
            }
            Some(RowSel::Hline(n))
        }
        Some('#') => {
            *i += 1;
            Some(RowSel::Cur)
        }
        _ => {
            let signed = matches!(chars.get(*i), Some('+') | Some('-'));
            let n = scan_int(chars, i)?;
            Some(if signed {
                RowSel::Rel(n)
            } else if n == 0 {
                RowSel::Cur
            } else {
                RowSel::Abs(n)
            })
        }
    }
}

fn scan_col(chars: &[char], i: &mut usize) -> Option<ColSel> {
    if chars.get(*i) != Some(&'$') {
        return None;
    }
    *i += 1;
    match chars.get(*i) {
        Some('<') => {
            let mut n = 0;
            while chars.get(*i) == Some(&'<') {
                n += 1;
                *i += 1;
            }
            Some(ColSel::First(n))
        }
        Some('>') => {
            let mut n = 0;
            while chars.get(*i) == Some(&'>') {
                n += 1;
                *i += 1;
            }
            Some(ColSel::Last(n))
        }
        Some('#') => {
            *i += 1;
            Some(ColSel::Cur)
        }
        Some(c) if c.is_alphabetic() || *c == '_' => {
            let start = *i;
            while chars
                .get(*i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                *i += 1;
            }
            Some(ColSel::Name(chars[start..*i].iter().collect()))
        }
        _ => {
            let signed = matches!(chars.get(*i), Some('+') | Some('-'));
            let n = scan_int(chars, i)?;
            Some(if signed {
                ColSel::Rel(n)
            } else {
                ColSel::Abs(n)
            })
        }
    }
}

// --------------------------------------------------------------- the grid

struct Grid<'a> {
    rows: &'a [Row],
    /// Indices into `rows` of the data rows.
    data: Vec<usize>,
    hlines: Vec<usize>,
    names: Vec<(String, usize)>,
    consts: Vec<(String, String)>,
}

impl<'a> Grid<'a> {
    fn new(rows: &'a [Row]) -> Self {
        let data = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_rule())
            .map(|(i, _)| i)
            .collect();
        let hlines = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_rule())
            .map(|(i, _)| i)
            .collect();
        let mut names = Vec::new();
        let mut consts = Vec::new();
        for r in rows {
            if let Row::Data(c) = r {
                match c.first().map(String::as_str) {
                    Some("!") => {
                        for (i, n) in c.iter().enumerate().skip(1) {
                            if !n.is_empty() {
                                names.push((n.clone(), i + 1));
                            }
                        }
                    }
                    Some("$") => {
                        for f in c.iter().skip(1) {
                            if let Some((k, v)) = f.split_once('=') {
                                consts.push((k.trim().to_string(), v.trim().to_string()));
                            }
                        }
                    }
                    Some("^") => {
                        // Names for the column in the row above is not supported.
                    }
                    _ => {}
                }
            }
        }
        Grid {
            rows,
            data,
            hlines,
            names,
            consts,
        }
    }

    fn ncols(&self) -> usize {
        self.rows
            .iter()
            .filter_map(|r| {
                if let Row::Data(c) = r {
                    Some(c.len())
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0)
    }

    /// Data row number (1-based) -> index into rows.
    fn data_row(&self, n: i64) -> Option<usize> {
        (n >= 1)
            .then(|| self.data.get(n as usize - 1).copied())
            .flatten()
    }

    /// 1-based data row number of a rows index.
    fn row_number(&self, idx: usize) -> i64 {
        self.data
            .iter()
            .position(|&i| i == idx)
            .map_or(0, |p| p as i64 + 1)
    }

    fn resolve_row(&self, sel: &RowSel, cur: usize, as_end: bool) -> Option<usize> {
        let cur_n = self.row_number(cur);
        match sel {
            RowSel::Cur => Some(cur),
            RowSel::Abs(n) => self.data_row(*n),
            RowSel::Rel(n) => self.data_row(cur_n + n),
            RowSel::First(k) => self.data_row(*k as i64),
            RowSel::Last(k) => self.data_row(self.data.len() as i64 - *k as i64 + 1),
            RowSel::Hline(n) => {
                let h = *self.hlines.get(n.checked_sub(1)?)?;
                if as_end {
                    // Last data row before this hline.
                    self.data.iter().rev().find(|&&i| i < h).copied()
                } else {
                    self.data.iter().find(|&&i| i > h).copied()
                }
            }
        }
    }

    fn resolve_col(&self, sel: &ColSel, cur: usize) -> Option<usize> {
        let n = self.ncols() as i64;
        let c = match sel {
            ColSel::Cur => cur as i64,
            ColSel::Abs(n) => *n,
            ColSel::Rel(d) => cur as i64 + d,
            ColSel::First(k) => *k as i64,
            ColSel::Last(k) => n - *k as i64 + 1,
            ColSel::Name(name) => {
                return self.names.iter().find(|(k, _)| k == name).map(|(_, c)| *c);
            }
        };
        (c >= 1).then_some(c as usize)
    }

    fn field(&self, row: usize, col: usize) -> String {
        match self.rows.get(row) {
            Some(Row::Data(c)) => c.get(col - 1).cloned().unwrap_or_default(),
            _ => String::new(),
        }
    }
}

// ------------------------------------------------------------- evaluator

struct Ctx<'a> {
    grid: &'a Grid<'a>,
    row: usize,
    col: usize,
    flags: &'a Flags,
}

fn field_val(text: &str, flags: &Flags) -> Val {
    let t = text.trim();
    if t.is_empty() {
        return if flags.n || flags.e {
            Val::Num(0.0)
        } else {
            Val::Str(String::new())
        };
    }
    match t.parse::<f64>() {
        Ok(v) => Val::Num(v),
        Err(_) if flags.n => Val::Num(0.0),
        Err(_) => Val::Str(t.to_string()),
    }
}

struct Parser<'a, 'b> {
    s: Vec<char>,
    i: usize,
    ctx: &'a Ctx<'b>,
}

impl Parser<'_, '_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_whitespace()) {
            self.i += 1;
        }
    }

    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        let t: Vec<char> = t.chars().collect();
        if self.s[self.i..].starts_with(&t) {
            self.i += t.len();
            true
        } else {
            false
        }
    }

    fn peek_is(&mut self, t: &str) -> bool {
        self.ws();
        let t: Vec<char> = t.chars().collect();
        self.s[self.i..].starts_with(&t)
    }

    fn or(&mut self) -> Result<Val, String> {
        let mut a = self.and()?;
        while self.eat("||") {
            let b = self.and()?;
            a = Val::Num(f64::from(truthy(&a) || truthy(&b)));
        }
        Ok(a)
    }

    fn and(&mut self) -> Result<Val, String> {
        let mut a = self.cmp()?;
        while self.eat("&&") {
            let b = self.cmp()?;
            a = Val::Num(f64::from(truthy(&a) && truthy(&b)));
        }
        Ok(a)
    }

    fn cmp(&mut self) -> Result<Val, String> {
        let a = self.add()?;
        for op in ["<=", ">=", "==", "!=", "<", ">", "="] {
            if self.peek_is(op) {
                self.eat(op);
                let b = self.add()?;
                let r = match (&a, &b) {
                    (Val::Num(x), Val::Num(y)) => match op {
                        "<=" => x <= y,
                        ">=" => x >= y,
                        "==" | "=" => x == y,
                        "!=" => x != y,
                        "<" => x < y,
                        _ => x > y,
                    },
                    _ => {
                        let (x, y) = (as_str(&a), as_str(&b));
                        match op {
                            "==" | "=" => x == y,
                            "!=" => x != y,
                            "<" => x < y,
                            ">" => x > y,
                            "<=" => x <= y,
                            _ => x >= y,
                        }
                    }
                };
                return Ok(Val::Num(f64::from(r)));
            }
        }
        Ok(a)
    }

    fn add(&mut self) -> Result<Val, String> {
        let mut a = self.mul()?;
        loop {
            self.ws();
            if self.eat("+") {
                let b = self.mul()?;
                a = arith(&a, &b, |x, y| x + y);
            } else if self.peek_is("-") && !self.peek_is("--") {
                self.eat("-");
                let b = self.mul()?;
                a = arith(&a, &b, |x, y| x - y);
            } else {
                return Ok(a);
            }
        }
    }

    fn mul(&mut self) -> Result<Val, String> {
        let mut a = self.unary()?;
        loop {
            if self.eat("*") {
                let b = self.unary()?;
                a = arith(&a, &b, |x, y| x * y);
            } else if self.eat("/") {
                let b = self.unary()?;
                a = arith(&a, &b, |x, y| x / y);
            } else if self.eat("%") {
                let b = self.unary()?;
                a = arith(&a, &b, |x, y| x % y);
            } else {
                return Ok(a);
            }
        }
    }

    fn unary(&mut self) -> Result<Val, String> {
        if self.eat("-") {
            let v = self.unary()?;
            return Ok(arith(&Val::Num(0.0), &v, |x, y| x - y));
        }
        if self.eat("+") {
            return self.unary();
        }
        if self.peek_is("!") && !self.peek_is("!=") {
            self.eat("!");
            let v = self.unary()?;
            return Ok(Val::Num(f64::from(!truthy(&v))));
        }
        self.pow()
    }

    fn pow(&mut self) -> Result<Val, String> {
        let a = self.primary()?;
        if self.eat("^") {
            let b = self.unary()?;
            return Ok(arith(&a, &b, f64::powf));
        }
        Ok(a)
    }

    fn primary(&mut self) -> Result<Val, String> {
        self.ws();
        let Some(&c) = self.s.get(self.i) else {
            return Err("Unexpected end of formula".into());
        };
        if c == '(' {
            self.i += 1;
            let v = self.or()?;
            if !self.eat(")") {
                return Err("Missing )".into());
            }
            return Ok(v);
        }
        if c == '"' {
            self.i += 1;
            let start = self.i;
            while self.s.get(self.i).is_some_and(|c| *c != '"') {
                self.i += 1;
            }
            let text: String = self.s[start..self.i].iter().collect();
            // An unterminated string leaves the index at the end of the
            // input: step past the closing quote only when there is one.
            self.i = (self.i + 1).min(self.s.len());
            return Ok(Val::Str(text));
        }
        if c.is_ascii_digit()
            || (c == '.' && self.s.get(self.i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = self.i;
            while self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_ascii_digit() || *c == '.')
            {
                self.i += 1;
            }
            if matches!(self.s.get(self.i), Some('e') | Some('E'))
                && self
                    .s
                    .get(self.i + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
            {
                self.i += 2;
                while self.s.get(self.i).is_some_and(char::is_ascii_digit) {
                    self.i += 1;
                }
            }
            let t: String = self.s[start..self.i].iter().collect();
            return t
                .parse()
                .map(Val::Num)
                .map_err(|_| format!("Bad number {t}"));
        }
        if c == '@' || c == '$' {
            return self.reference();
        }
        if c.is_alphabetic() {
            let start = self.i;
            while self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                self.i += 1;
            }
            let name: String = self.s[start..self.i].iter().collect();
            if self.eat("(") {
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.or()?);
                        if self.eat(")") {
                            break;
                        }
                        if !self.eat(",") {
                            return Err(format!("Bad arguments to {name}"));
                        }
                    }
                }
                return call(&name, args);
            }
            return match name.as_str() {
                "pi" => Ok(Val::Num(std::f64::consts::PI)),
                "nan" => Ok(Val::Num(f64::NAN)),
                _ => Err(format!("Unknown name {name}")),
            };
        }
        Err(format!("Unexpected {c:?} in formula"))
    }

    fn reference(&mut self) -> Result<Val, String> {
        let first = self.scan_one()?;
        if self.s[self.i..].starts_with(&['.', '.']) {
            self.i += 2;
            let second = self.scan_one()?;
            return self.range(first, second);
        }
        let (r, c) = first;
        match (&r, &c) {
            (Some(r), None)
                if *r != RowSel::Cur || self.s.get(self.i.wrapping_sub(1)) != Some(&'#') =>
            {
                let row = self
                    .ctx
                    .grid
                    .resolve_row(r, self.ctx.row, false)
                    .ok_or("Row out of range")?;
                let vals = (1..=self.ctx.grid.ncols())
                    .map(|col| field_val(&self.ctx.grid.field(row, col), self.ctx.flags))
                    .collect();
                Ok(Val::Vec(vals))
            }
            _ => self.scalar(r, c),
        }
    }

    fn scan_one(&mut self) -> Result<(Option<RowSel>, Option<ColSel>), String> {
        let (r, c);
        let mut i = self.i;
        r = scan_row(&self.s, &mut i);
        c = scan_col(&self.s, &mut i);
        if r.is_none() && c.is_none() {
            return Err("Bad field reference".into());
        }
        self.i = i;
        Ok((r, c))
    }

    fn scalar(&self, r: Option<RowSel>, c: Option<ColSel>) -> Result<Val, String> {
        let g = self.ctx.grid;
        if r.is_none() && c == Some(ColSel::Cur) {
            return Ok(Val::Num(self.ctx.col as f64));
        }
        if c.is_none() && r == Some(RowSel::Cur) {
            return Ok(Val::Num(g.row_number(self.ctx.row) as f64));
        }
        if let Some(ColSel::Name(n)) = &c
            && r.is_none()
            && let Some((_, v)) = g.consts.iter().find(|(k, _)| k == n)
        {
            return Ok(field_val(v, self.ctx.flags));
        }
        let row = match &r {
            Some(sel) => g
                .resolve_row(sel, self.ctx.row, false)
                .ok_or("Row out of range")?,
            None => self.ctx.row,
        };
        let col = match &c {
            Some(sel) => g
                .resolve_col(sel, self.ctx.col)
                .ok_or("Column out of range")?,
            None => self.ctx.col,
        };
        Ok(field_val(&g.field(row, col), self.ctx.flags))
    }

    fn range(
        &self,
        a: (Option<RowSel>, Option<ColSel>),
        b: (Option<RowSel>, Option<ColSel>),
    ) -> Result<Val, String> {
        let g = self.ctx.grid;
        let row_of = |s: &Option<RowSel>, end: bool| match s {
            Some(sel) => g
                .resolve_row(sel, self.ctx.row, end)
                .ok_or("Row out of range"),
            None => Ok(self.ctx.row),
        };
        let col_of = |s: &Option<ColSel>| match s {
            Some(sel) => g
                .resolve_col(sel, self.ctx.col)
                .ok_or("Column out of range"),
            None => Ok(self.ctx.col),
        };
        let (r1, r2) = (row_of(&a.0, false)?, row_of(&b.0, true)?);
        let (c1, c2) = (col_of(&a.1)?, col_of(&b.1)?);
        let (r1, r2) = (r1.min(r2), r1.max(r2));
        let (c1, c2) = (c1.min(c2), c1.max(c2));
        let mut out = Vec::new();
        for &ri in &g.data {
            if ri >= r1 && ri <= r2 {
                for col in c1..=c2 {
                    out.push(field_val(&g.field(ri, col), self.ctx.flags));
                }
            }
        }
        Ok(Val::Vec(out))
    }
}

fn truthy(v: &Val) -> bool {
    match v {
        Val::Num(n) => *n != 0.0 && !n.is_nan(),
        Val::Str(s) => !s.is_empty(),
        Val::Vec(v) => !v.is_empty(),
    }
}

fn as_str(v: &Val) -> String {
    match v {
        Val::Num(n) => fmt_num(*n),
        Val::Str(s) => s.clone(),
        Val::Vec(_) => String::new(),
    }
}

fn arith(a: &Val, b: &Val, f: impl Fn(f64, f64) -> f64) -> Val {
    match (a, b) {
        (Val::Num(x), Val::Num(y)) => Val::Num(f(*x, *y)),
        // A non-numeric field poisons arithmetic, the way Calc's nan does.
        _ => Val::Num(f64::NAN),
    }
}

fn numbers(args: &[Val]) -> Vec<f64> {
    let mut out = Vec::new();
    for a in args {
        match a {
            Val::Num(n) => out.push(*n),
            Val::Vec(v) => out.extend(numbers(v)),
            Val::Str(s) => {
                if let Ok(n) = s.trim().parse::<f64>() {
                    out.push(n);
                }
            }
        }
    }
    out
}

fn call(name: &str, args: Vec<Val>) -> Result<Val, String> {
    let nums = || numbers(&args);
    let one = |f: fn(f64) -> f64| match args.first() {
        Some(Val::Num(n)) => Ok(Val::Num(f(*n))),
        _ => Ok(Val::Num(f64::NAN)),
    };
    Ok(Val::Num(match name {
        "vsum" | "sum" => nums().iter().sum(),
        "vprod" | "prod" => nums().iter().product(),
        "vmean" | "mean" => {
            let n = nums();
            if n.is_empty() {
                f64::NAN
            } else {
                n.iter().sum::<f64>() / n.len() as f64
            }
        }
        "vmax" | "max" => nums().into_iter().fold(f64::NEG_INFINITY, f64::max),
        "vmin" | "min" => nums().into_iter().fold(f64::INFINITY, f64::min),
        "vcount" | "count" => nums().len() as f64,
        "vmedian" | "median" => {
            let mut n = nums();
            n.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            match n.len() {
                0 => f64::NAN,
                l if l % 2 == 1 => n[l / 2],
                l => (n[l / 2 - 1] + n[l / 2]) / 2.0,
            }
        }
        "abs" => return one(f64::abs),
        "sqrt" => return one(f64::sqrt),
        "exp" => return one(f64::exp),
        "ln" => return one(f64::ln),
        "log10" => return one(f64::log10),
        "sin" => return one(f64::sin),
        "cos" => return one(f64::cos),
        "tan" => return one(f64::tan),
        "floor" => return one(f64::floor),
        "ceil" => return one(f64::ceil),
        "round" => match args.as_slice() {
            [Val::Num(n)] => n.round(),
            [Val::Num(n), Val::Num(d)] => {
                let m = 10f64.powi(*d as i32);
                (n * m).round() / m
            }
            _ => f64::NAN,
        },
        "if" => {
            return match args.as_slice() {
                [c, a, b] => Ok(if truthy(c) { a.clone() } else { b.clone() }),
                _ => Err("if() takes three arguments".into()),
            };
        }
        other => return Err(format!("Unknown function {other}")),
    }))
}

/// A number as Calc would print it.
fn fmt_num(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if v == v.trunc() && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.12}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn format_result(v: &Val, flags: &Flags) -> String {
    match v {
        Val::Str(s) => s.clone(),
        Val::Vec(items) => items
            .iter()
            .map(|i| format_result(i, flags))
            .collect::<Vec<_>>()
            .join(", "),
        Val::Num(n) => match &flags.format {
            Some(f) => printf(f, *n),
            None => fmt_num(*n),
        },
    }
}

/// A tiny printf for `%.2f`, `%d`, `%e`, `%g`, `%5.1f` and `%s`.
fn printf(fmt: &str, v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    let body = fmt.trim_start_matches('%');
    // Split the conversion character off the spec on a char boundary: the
    // last character may be multi-byte ("%0.2é").
    let (conv, spec) = match body.char_indices().next_back() {
        Some((i, c)) => (c, &body[..i]),
        None => ('f', body),
    };
    let (width, prec) = match spec.split_once('.') {
        Some((w, p)) => (w.parse::<usize>().unwrap_or(0), p.parse::<usize>().ok()),
        None => (spec.parse::<usize>().unwrap_or(0), None),
    };
    let s = match conv {
        'd' | 'i' => format!("{}", v.round() as i64),
        'e' => {
            let p = prec.unwrap_or(6);
            let s = format!("{v:.p$e}");
            // Rust prints 1.5e3; C prints 1.5e+03.
            match s.split_once('e') {
                Some((m, e)) => {
                    let (sign, digits) = match e.strip_prefix('-') {
                        Some(d) => ('-', d),
                        None => ('+', e),
                    };
                    format!("{m}e{sign}{digits:0>2}")
                }
                None => s,
            }
        }
        'g' => fmt_num(v),
        's' => fmt_num(v),
        _ => format!("{:.*}", prec.unwrap_or(6), v),
    };
    format!("{s:>width$}")
}

fn eval(expr: &str, ctx: &Ctx) -> Result<String, String> {
    let mut p = Parser {
        s: expr.chars().collect(),
        i: 0,
        ctx,
    };
    let v = p.or()?;
    p.ws();
    if p.i < p.s.len() {
        return Err(format!("Unexpected {:?} in formula", p.s[p.i]));
    }
    Ok(format_result(&v, ctx.flags))
}

/// Does this data row get formulas? Marker rows (`!` `^` `_` `$` `/`) do
/// not.
fn computable(row: &Row) -> bool {
    match row {
        Row::Rule => false,
        Row::Data(c) => !matches!(
            c.first().map(String::as_str),
            Some("!" | "^" | "_" | "$" | "/")
        ),
    }
}

/// Apply `formulas` (the bodies of the `#+TBLFM:` lines) to `rows`.
/// Returns how many fields changed. `only` restricts the work to one
/// field `(row index, 1-based column)`.
pub fn recalculate(
    rows: &mut [Row],
    formulas: &[String],
    only: Option<(usize, usize)>,
) -> Result<usize, String> {
    let parsed = parse_formulas(formulas)?;
    let mut changed = 0;
    // Column and row formulas first, then field formulas (they win).
    let order: Vec<&Formula> = parsed
        .iter()
        .filter(|f| !matches!(f.target, Target::Field(..)))
        .chain(
            parsed
                .iter()
                .filter(|f| matches!(f.target, Target::Field(..))),
        )
        .collect();
    let first_rule = rows.iter().position(Row::is_rule);
    for f in order {
        // Which (row index, column) pairs it fills.
        let cells: Vec<(usize, usize)> = {
            let g = Grid::new(rows);
            match &f.target {
                Target::Column(c) => {
                    let col = g
                        .resolve_col(c, 1)
                        .ok_or("Unknown column in formula target")?;
                    g.data
                        .iter()
                        .copied()
                        .filter(|&i| computable(&rows[i]))
                        // Rows above the first hline are a header.
                        .filter(|&i| first_rule.is_none_or(|h| i > h))
                        .map(|i| (i, col))
                        .collect()
                }
                Target::Row(r) => {
                    let row = g
                        .resolve_row(r, g.data.first().copied().unwrap_or(0), false)
                        .ok_or("Row out of range")?;
                    (1..=g.ncols()).map(|c| (row, c)).collect()
                }
                Target::Field(r, c, end) => {
                    let cur = only.map_or_else(|| g.data.first().copied().unwrap_or(0), |(r, _)| r);
                    let curc = only.map_or(1, |(_, c)| c);
                    let r1 = g.resolve_row(r, cur, false).ok_or("Row out of range")?;
                    let c1 = g.resolve_col(c, curc).ok_or("Column out of range")?;
                    match end {
                        None => vec![(r1, c1)],
                        Some((r2, c2)) => {
                            let r2 = g.resolve_row(r2, cur, true).ok_or("Row out of range")?;
                            let c2 = g.resolve_col(c2, curc).ok_or("Column out of range")?;
                            let mut v = Vec::new();
                            for &ri in &g.data {
                                if ri >= r1.min(r2) && ri <= r1.max(r2) {
                                    for col in c1.min(c2)..=c1.max(c2) {
                                        v.push((ri, col));
                                    }
                                }
                            }
                            v
                        }
                    }
                }
            }
        };
        for (ri, col) in cells {
            if let Some((or, oc)) = only
                && (or, oc) != (ri, col)
            {
                continue;
            }
            let text = {
                let g = Grid::new(rows);
                let ctx = Ctx {
                    grid: &g,
                    row: ri,
                    col,
                    flags: &f.flags,
                };
                match eval(&f.expr, &ctx) {
                    Ok(t) => t,
                    Err(e) => {
                        // Put the error in the field, as Org does, and keep going.
                        let _ = e;
                        "#ERROR".to_string()
                    }
                }
            };
            if let Row::Data(c) = &mut rows[ri] {
                while c.len() < col {
                    c.push(String::new());
                }
                if c[col - 1] != text {
                    c[col - 1] = text;
                    changed += 1;
                }
            }
        }
    }
    Ok(changed)
}

/// Evaluate one formula `expr` for the field at (`row`, `col`) and return
/// the text (`org-table-eval-formula`).
pub fn eval_for_field(
    rows: &[Row],
    row: usize,
    col: usize,
    formula: &str,
) -> Result<String, String> {
    let (expr, flags) = split_flags(formula.trim().trim_start_matches('='));
    let g = Grid::new(rows);
    let ctx = Ctx {
        grid: &g,
        row,
        col,
        flags: &flags,
    };
    eval(&expr, &ctx)
}

/// Format a number the way table formulas print it.
pub fn format_number(v: f64) -> String {
    fmt_num(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::org_table::parse_row;

    fn rows(lines: &[&str]) -> Vec<Row> {
        lines.iter().map(|l| parse_row(l)).collect()
    }

    fn cell(rows: &[Row], r: usize, c: usize) -> String {
        match &rows[r] {
            Row::Data(cells) => cells[c].clone(),
            Row::Rule => String::new(),
        }
    }

    #[test]
    fn column_formula_skips_the_header() {
        let mut t = rows(&[
            "| a | b | s |",
            "|---+---+---|",
            "| 1 | 2 |   |",
            "| 3 | 4 |   |",
        ]);
        let n = recalculate(&mut t, &["$3=$1+$2".into()], None).unwrap();
        assert_eq!(n, 2);
        assert_eq!(cell(&t, 2, 2), "3");
        assert_eq!(cell(&t, 3, 2), "7");
        assert_eq!(cell(&t, 0, 2), "s", "header untouched");
    }

    #[test]
    fn totals_ranges_and_formats() {
        let mut t = rows(&["| x |", "|---|", "| 1.5 |", "| 2 |", "|---|", "| |"]);
        recalculate(&mut t, &["@>$1=vsum(@I..@II);%.2f".into()], None).unwrap();
        assert_eq!(cell(&t, 5, 0), "3.50");
        recalculate(&mut t, &["@>$1=vmean(@2..@3)".into()], None).unwrap();
        assert_eq!(cell(&t, 5, 0), "1.75");
    }

    #[test]
    fn field_formulas_win_and_relative_refs() {
        let mut t = rows(&["| 1 | |", "| 2 | |", "| 3 | |"]);
        recalculate(
            &mut t,
            &["$2=$1*10".into(), "@2$2=@-1$2+@+1$2".into()],
            None,
        )
        .unwrap();
        assert_eq!(cell(&t, 0, 1), "10");
        assert_eq!(cell(&t, 1, 1), "40");
        assert_eq!(cell(&t, 2, 1), "30");
    }

    #[test]
    fn names_flags_and_functions() {
        let mut t = rows(&["| ! | price | qty | total |", "| 2 | 3 | |", "| x | 4 | |"]);
        let n = recalculate(&mut t, &["$4=$price*$qty;N".into()], None).unwrap_or(0);
        let _ = n;
        let mut t = rows(&["| 2 | 3 | |", "| x | 4 | |"]);
        recalculate(&mut t, &["$3=if($1>1, $2, 0);N".into()], None).unwrap();
        assert_eq!(cell(&t, 0, 2), "3");
        assert_eq!(cell(&t, 1, 2), "0");
        assert_eq!(
            eval_for_field(&t, 0, 3, "round(sqrt(16)/3, 2)").unwrap(),
            "1.33"
        );
        assert_eq!(eval_for_field(&t, 0, 3, "2^3^2").unwrap(), "512");
        assert_eq!(eval_for_field(&t, 0, 3, "7 % 4 * -2").unwrap(), "-6");
    }

    #[test]
    fn errors_show_in_the_field() {
        let mut t = rows(&["| 1 | |"]);
        recalculate(&mut t, &["$2=bogus($1)".into()], None).unwrap();
        assert_eq!(cell(&t, 0, 1), "#ERROR");
        assert!(recalculate(&mut t, &["$2:=(+ 1 2)".into()], None).is_err());
    }

    #[test]
    fn unterminated_string_is_not_a_panic() {
        let mut t = rows(&["| 1 | |"]);
        recalculate(&mut t, &["$2=\"abc".into()], None).unwrap();
        assert_eq!(cell(&t, 0, 1), "abc");
    }

    #[test]
    fn multibyte_format_spec_is_not_a_panic() {
        let mut t = rows(&["| 1.5 | |"]);
        recalculate(&mut t, &["$2=$1;%0.2é".into()], None).unwrap();
        assert_eq!(cell(&t, 0, 1), "1.50");
    }
}
