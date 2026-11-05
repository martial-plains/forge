// SPDX-License-Identifier: MIT OR Apache-2.0
//! The expression language Bases files use for filters, formulas and
//! sorting: `status != "done" && file.hasTag("project")`,
//! `(now() - file.mtime).days`, `if(price > 10, "dear", "cheap")`.

use std::cell::Cell;
use std::collections::BTreeMap;

use crate::value::{DAY, Value, format_date, parse_date, parse_duration};
use crate::vault::Index;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Value),
    Ident(String),
    Member(Box<Expr>, String),
    At(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    List(Vec<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Id(String),
    P(&'static str),
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let cs: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < cs.len() {
        let c = cs[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && cs.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                // `1.days` is a member, not a fraction.
                if cs[i] == '.' && !cs.get(i + 1).is_some_and(char::is_ascii_digit) {
                    break;
                }
                i += 1;
            }
            let t: String = cs[start..i].iter().collect();
            out.push(Tok::Num(t.parse().map_err(|_| format!("bad number {t}"))?));
        } else if c == '"' || c == '\'' {
            let q = c;
            i += 1;
            let mut s = String::new();
            loop {
                match cs.get(i) {
                    None => return Err("unterminated string".into()),
                    Some('\\') => {
                        i += 1;
                        match cs.get(i) {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(&o) => s.push(o),
                            None => return Err("unterminated string".into()),
                        }
                    }
                    Some(&ch) if ch == q => break,
                    Some(&ch) => s.push(ch),
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < cs.len()
                && (cs[i].is_alphanumeric()
                    || cs[i] == '_'
                    || cs[i] == '-' && cs.get(i + 1).is_some_and(|n| n.is_alphanumeric()) && false)
            {
                i += 1;
            }
            out.push(Tok::Id(cs[start..i].iter().collect()));
        } else {
            let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
            let p = match two.as_str() {
                "==" => Some("=="),
                "!=" => Some("!="),
                "<=" => Some("<="),
                ">=" => Some(">="),
                "&&" => Some("&&"),
                "||" => Some("||"),
                _ => None,
            };
            if let Some(p) = p {
                out.push(Tok::P(p));
                i += 2;
                continue;
            }
            let p = match c {
                '+' => "+",
                '-' => "-",
                '*' => "*",
                '/' => "/",
                '%' => "%",
                '<' => "<",
                '>' => ">",
                '!' => "!",
                '(' => "(",
                ')' => ")",
                '[' => "[",
                ']' => "]",
                ',' => ",",
                '.' => ".",
                _ => return Err(format!("unexpected {c:?}")),
            };
            out.push(Tok::P(p));
            i += 1;
        }
    }
    Ok(out)
}

struct Parser {
    t: Vec<Tok>,
    i: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn eat(&mut self, p: &str) -> bool {
        if self.peek()
            == Some(&Tok::P(match p {
                "==" => "==",
                "!=" => "!=",
                "<=" => "<=",
                ">=" => ">=",
                "&&" => "&&",
                "||" => "||",
                "+" => "+",
                "-" => "-",
                "*" => "*",
                "/" => "/",
                "%" => "%",
                "<" => "<",
                ">" => ">",
                "!" => "!",
                "(" => "(",
                ")" => ")",
                "[" => "[",
                "]" => "]",
                "," => ",",
                "." => ".",
                _ => return false,
            }))
        {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, p: &str) -> Result<(), String> {
        if self.eat(p) {
            Ok(())
        } else {
            Err(format!("expected {p}"))
        }
    }

    fn binary(&mut self, level: usize) -> Result<Expr, String> {
        const LEVELS: &[&[(&str, Op)]] = &[
            &[("||", Op::Or)],
            &[("&&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        if level == LEVELS.len() {
            return self.unary();
        }
        let mut left = self.binary(level + 1)?;
        'outer: loop {
            for (p, op) in LEVELS[level] {
                if self.eat(p) {
                    let right = self.binary(level + 1)?;
                    left = Expr::Bin(*op, Box::new(left), Box::new(right));
                    continue 'outer;
                }
            }
            return Ok(left);
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("-") {
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, String> {
        let mut e = self.primary()?;
        loop {
            if self.eat(".") {
                match self.peek().cloned() {
                    Some(Tok::Id(name)) => {
                        self.i += 1;
                        e = Expr::Member(Box::new(e), name);
                    }
                    _ => return Err("expected a name after .".into()),
                }
            } else if self.eat("(") {
                let mut args = Vec::new();
                if !self.eat(")") {
                    loop {
                        args.push(self.binary(0)?);
                        if self.eat(")") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                e = Expr::Call(Box::new(e), args);
            } else if self.eat("[") {
                let ix = self.binary(0)?;
                self.expect("]")?;
                e = Expr::At(Box::new(e), Box::new(ix));
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.i += 1;
                Ok(Expr::Lit(Value::Num(n)))
            }
            Some(Tok::Str(s)) => {
                self.i += 1;
                Ok(Expr::Lit(Value::Str(s)))
            }
            Some(Tok::Id(id)) => {
                self.i += 1;
                Ok(match id.as_str() {
                    "true" => Expr::Lit(Value::Bool(true)),
                    "false" => Expr::Lit(Value::Bool(false)),
                    "null" => Expr::Lit(Value::Null),
                    _ => Expr::Ident(id),
                })
            }
            Some(Tok::P("(")) => {
                self.i += 1;
                let e = self.binary(0)?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::P("[")) => {
                self.i += 1;
                let mut items = Vec::new();
                if !self.eat("]") {
                    loop {
                        items.push(self.binary(0)?);
                        if self.eat("]") {
                            break;
                        }
                        self.expect(",")?;
                    }
                }
                Ok(Expr::List(items))
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("the expression ends too soon".into()),
        }
    }
}

pub fn parse(src: &str) -> Result<Expr, String> {
    let mut p = Parser { t: lex(src)?, i: 0 };
    let e = p.binary(0)?;
    if p.i < p.t.len() {
        return Err(format!("unexpected {:?}", p.t[p.i]));
    }
    Ok(e)
}

/// What an expression is evaluated against: one note in an index.
pub struct Ctx<'a> {
    pub index: &'a Index,
    pub note: usize,
    pub formulas: &'a BTreeMap<String, String>,
    /// Milliseconds since 1970 (`now()`).
    pub now: i64,
    depth: Cell<u32>,
}

impl<'a> Ctx<'a> {
    pub fn new(
        index: &'a Index,
        note: usize,
        formulas: &'a BTreeMap<String, String>,
        now: i64,
    ) -> Self {
        Ctx {
            index,
            note,
            formulas,
            now,
            depth: Cell::new(0),
        }
    }
}

pub fn eval_str(src: &str, ctx: &Ctx) -> Result<Value, String> {
    eval(&parse(src)?, ctx)
}

fn loose_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Str(x) | Value::Link(x), Value::Str(y) | Value::Link(y)) => x == y,
        (Value::Date(d), Value::Str(s)) | (Value::Str(s), Value::Date(d)) => {
            parse_date(s) == Some(*d)
        }
        (Value::Num(x), Value::Str(s)) | (Value::Str(s), Value::Num(x)) => {
            s.trim().parse::<f64>() == Ok(*x)
        }
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| loose_eq(p, q))
        }
        _ => a == b,
    }
}

fn as_duration(v: &Value) -> Option<i64> {
    match v {
        Value::Dur(d) => Some(*d),
        Value::Str(s) => parse_duration(s),
        _ => None,
    }
}

fn arith(op: Op, a: &Value, b: &Value) -> Result<Value, String> {
    use Value::*;
    Ok(match (op, a, b) {
        (Op::Add, Str(x), y) => Str(format!("{x}{y}")),
        (Op::Add, x, Str(y)) if !matches!(x, Date(_) | Dur(_)) => Str(format!("{x}{y}")),
        (Op::Add, List(x), List(y)) => List(x.iter().chain(y).cloned().collect()),
        (Op::Sub, Date(x), Date(y)) => Dur(x - y),
        (Op::Add, Date(x), y) => {
            Date(x + as_duration(y).ok_or("can only add a duration to a date")?)
        }
        (Op::Sub, Date(x), y) => {
            Date(x - as_duration(y).ok_or("can only subtract a duration from a date")?)
        }
        (Op::Add, Dur(x), Dur(y)) => Dur(x + y),
        (Op::Sub, Dur(x), Dur(y)) => Dur(x - y),
        (Op::Mul, Dur(x), Num(y)) | (Op::Mul, Num(y), Dur(x)) => Dur((*x as f64 * y) as i64),
        (Op::Div, Dur(x), Num(y)) => Dur((*x as f64 / y) as i64),
        (Op::Div, Dur(x), Dur(y)) => Num(*x as f64 / *y as f64),
        (_, x, y) => {
            let (Some(p), Some(q)) = (x.as_num(), y.as_num()) else {
                return if matches!(x, Null) || matches!(y, Null) {
                    Ok(Null)
                } else {
                    Err(format!(
                        "can't do arithmetic on {} and {}",
                        x.type_name(),
                        y.type_name()
                    ))
                };
            };
            Num(match op {
                Op::Add => p + q,
                Op::Sub => p - q,
                Op::Mul => p * q,
                Op::Div => p / q,
                Op::Rem => p % q,
                _ => unreachable!(),
            })
        }
    })
}

pub fn eval(e: &Expr, ctx: &Ctx) -> Result<Value, String> {
    use Value::*;
    Ok(match e {
        Expr::Lit(v) => v.clone(),
        Expr::List(items) => List(
            items
                .iter()
                .map(|i| eval(i, ctx))
                .collect::<Result<_, _>>()?,
        ),
        Expr::Ident(name) => match name.as_str() {
            "file" => ctx.index.file_value(ctx.note),
            "note" => Obj(ctx.index.notes[ctx.note].props.clone()),
            "formula" => Obj(BTreeMap::new()),
            n => ctx.index.notes[ctx.note]
                .props
                .get(n)
                .cloned()
                .unwrap_or(Null),
        },
        Expr::Not(x) => Bool(!eval(x, ctx)?.truthy()),
        Expr::Neg(x) => match eval(x, ctx)? {
            Dur(d) => Dur(-d),
            v => Num(-v.as_num().ok_or("can't negate that")?),
        },
        Expr::Bin(Op::And, l, r) => {
            let lv = eval(l, ctx)?;
            if !lv.truthy() {
                Bool(false)
            } else {
                Bool(eval(r, ctx)?.truthy())
            }
        }
        Expr::Bin(Op::Or, l, r) => {
            let lv = eval(l, ctx)?;
            if lv.truthy() {
                Bool(true)
            } else {
                Bool(eval(r, ctx)?.truthy())
            }
        }
        Expr::Bin(op, l, r) => {
            let (a, b) = (eval(l, ctx)?, eval(r, ctx)?);
            match op {
                Op::Eq => Bool(loose_eq(&a, &b)),
                Op::Ne => Bool(!loose_eq(&a, &b)),
                Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                    if matches!(a, Null) || matches!(b, Null) {
                        Bool(false)
                    } else {
                        let ord = a.compare(&b);
                        Bool(match op {
                            Op::Lt => ord.is_lt(),
                            Op::Le => ord.is_le(),
                            Op::Gt => ord.is_gt(),
                            _ => ord.is_ge(),
                        })
                    }
                }
                _ => arith(*op, &a, &b)?,
            }
        }
        Expr::At(x, i) => {
            let (x, i) = (eval(x, ctx)?, eval(i, ctx)?);
            match (&x, &i) {
                (List(l), n) => n
                    .as_num()
                    .and_then(|n| l.get(n as usize))
                    .cloned()
                    .unwrap_or(Null),
                (Obj(o), k) => o.get(&k.to_string()).cloned().unwrap_or(Null),
                (Str(s), n) => n
                    .as_num()
                    .and_then(|n| s.chars().nth(n as usize))
                    .map_or(Null, |c| Str(c.to_string())),
                _ => Null,
            }
        }
        Expr::Member(obj, name) => member(obj, name, ctx)?,
        Expr::Call(callee, args) => call(callee, args, ctx)?,
    })
}

fn member(obj: &Expr, name: &str, ctx: &Ctx) -> Result<Value, String> {
    use Value::*;
    if let Expr::Ident(id) = obj {
        match id.as_str() {
            "formula" => return formula(name, ctx),
            "note" => {
                return Ok(ctx.index.notes[ctx.note]
                    .props
                    .get(name)
                    .cloned()
                    .unwrap_or(Null));
            }
            _ => {}
        }
    }
    let v = eval(obj, ctx)?;
    Ok(match (&v, name) {
        (Obj(o), n) => o.get(n).cloned().unwrap_or(Null),
        (Str(s) | Link(s), "length") => Num(s.chars().count() as f64),
        (List(l), "length") => Num(l.len() as f64),
        (Dur(d), "days") => Num((*d as f64 / DAY as f64).trunc()),
        (Dur(d), "hours") => Num((*d as f64 / 3_600_000.0).trunc()),
        (Dur(d), "minutes") => Num((*d as f64 / 60_000.0).trunc()),
        (Dur(d), "seconds") => Num((*d as f64 / 1000.0).trunc()),
        (Dur(d), "milliseconds") => Num(*d as f64),
        (Date(d), "year") => Num(crate::value::civil_from_days(d.div_euclid(DAY)).0 as f64),
        (Date(d), "month") => Num(crate::value::civil_from_days(d.div_euclid(DAY)).1 as f64),
        (Date(d), "day") => Num(crate::value::civil_from_days(d.div_euclid(DAY)).2 as f64),
        (Date(d), "hour") => Num((d.rem_euclid(DAY) / 3_600_000) as f64),
        (Date(d), "minute") => Num((d.rem_euclid(DAY) / 60_000 % 60) as f64),
        (Null, _) => Null,
        _ => Null,
    })
}

fn formula(name: &str, ctx: &Ctx) -> Result<Value, String> {
    let src = ctx
        .formulas
        .get(name)
        .ok_or_else(|| format!("no formula named {name}"))?;
    if ctx.depth.get() > 16 {
        return Err(format!("formula {name} refers to itself"));
    }
    ctx.depth.set(ctx.depth.get() + 1);
    let r = eval_str(src, ctx);
    ctx.depth.set(ctx.depth.get() - 1);
    r.map_err(|e| format!("formula {name}: {e}"))
}

fn want_str(args: &[Value], i: usize, what: &str) -> Result<String, String> {
    args.get(i)
        .map(ToString::to_string)
        .ok_or_else(|| format!("{what} needs an argument"))
}

fn call(callee: &Expr, args: &[Expr], ctx: &Ctx) -> Result<Value, String> {
    use Value::*;
    match callee {
        Expr::Ident(name) => {
            // `if` evaluates only the branch it takes.
            if name == "if" {
                let cond = args
                    .first()
                    .map(|a| eval(a, ctx))
                    .transpose()?
                    .unwrap_or(Null);
                let pick = if cond.truthy() {
                    args.get(1)
                } else {
                    args.get(2)
                };
                return pick.map_or(Ok(Null), |a| eval(a, ctx));
            }
            let a: Vec<Value> = args
                .iter()
                .map(|x| eval(x, ctx))
                .collect::<Result<_, _>>()?;
            Ok(match name.as_str() {
                "now" => Date(ctx.now),
                "today" => Date(ctx.now.div_euclid(DAY) * DAY),
                "date" => match a.first() {
                    Some(Date(d)) => Date(d.div_euclid(DAY) * DAY),
                    Some(v) => parse_date(&v.to_string()).map_or(Null, Date),
                    None => Null,
                },
                "duration" => parse_duration(&want_str(&a, 0, "duration")?).map_or(Null, Dur),
                "number" => a.first().and_then(Value::as_num).map_or(Null, Num),
                "string" => Str(a.first().map(ToString::to_string).unwrap_or_default()),
                "list" => match a.first() {
                    Some(List(l)) => List(l.clone()),
                    Some(Null) | None => List(vec![]),
                    Some(v) => List(vec![v.clone()]),
                },
                "link" => Link(want_str(&a, 0, "link")?),
                "min" | "max" => {
                    let nums: Vec<f64> = a.iter().filter_map(Value::as_num).collect();
                    let pick = if name == "min" {
                        nums.iter().copied().fold(f64::INFINITY, f64::min)
                    } else {
                        nums.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                    };
                    if nums.is_empty() { Null } else { Num(pick) }
                }
                "round" => a.first().and_then(Value::as_num).map_or(Null, |n| {
                    Num(round_to(n, a.get(1).and_then(Value::as_num).unwrap_or(0.0)))
                }),
                "floor" => a
                    .first()
                    .and_then(Value::as_num)
                    .map_or(Null, |n| Num(n.floor())),
                "ceil" => a
                    .first()
                    .and_then(Value::as_num)
                    .map_or(Null, |n| Num(n.ceil())),
                "abs" => a
                    .first()
                    .and_then(Value::as_num)
                    .map_or(Null, |n| Num(n.abs())),
                "contains" => {
                    Bool(want_str(&a, 0, "contains")?.contains(&want_str(&a, 1, "contains")?))
                }
                other => return Err(format!("no function named {other}")),
            })
        }
        Expr::Member(obj, method) => {
            if let Expr::Ident(id) = obj.as_ref()
                && id == "file"
            {
                let a: Vec<Value> = args
                    .iter()
                    .map(|x| eval(x, ctx))
                    .collect::<Result<_, _>>()?;
                return file_method(method, &a, ctx);
            }
            let target = eval(obj, ctx)?;
            let a: Vec<Value> = args
                .iter()
                .map(|x| eval(x, ctx))
                .collect::<Result<_, _>>()?;
            method_on(&target, method, &a)
        }
        _ => Err("that can't be called".into()),
    }
}

fn round_to(n: f64, digits: f64) -> f64 {
    let m = 10f64.powi(digits as i32);
    (n * m).round() / m
}

fn file_method(name: &str, a: &[Value], ctx: &Ctx) -> Result<Value, String> {
    use Value::*;
    let note = &ctx.index.notes[ctx.note];
    Ok(match name {
        "hasTag" => Bool(a.iter().any(|t| note.has_tag(&t.to_string()))),
        "inFolder" => {
            let f = want_str(a, 0, "inFolder")?;
            let f = f.trim_matches('/');
            Bool(note.folder == f || note.folder.starts_with(&format!("{f}/")))
        }
        "hasProperty" => Bool(note.props.contains_key(&want_str(a, 0, "hasProperty")?)),
        "hasLink" => {
            let want = want_str(a, 0, "hasLink")?;
            let target = ctx.index.resolve(&want, ctx.note);
            Bool(
                ctx.index.outgoing(ctx.note).iter().any(|&j| {
                    Some(j) == target || ctx.index.notes[j].name.eq_ignore_ascii_case(&want)
                }),
            )
        }
        "asLink" => Link(note.name.clone()),
        other => return Err(format!("file has no method {other}")),
    })
}

fn method_on(v: &Value, name: &str, a: &[Value]) -> Result<Value, String> {
    use Value::*;
    let arg_s = |i: usize| want_str(a, i, name);
    Ok(match (v, name) {
        (_, "isEmpty") => Bool(v.is_empty()),
        (_, "isTruthy") => Bool(v.truthy()),
        (_, "toString") => Str(v.to_string()),
        (_, "isType") => Bool(v.type_name() == arg_s(0)?),
        (Str(s) | Link(s), "contains") => Bool(s.contains(&arg_s(0)?)),
        (Str(s) | Link(s), "startsWith") => Bool(s.starts_with(&arg_s(0)?)),
        (Str(s) | Link(s), "endsWith") => Bool(s.ends_with(&arg_s(0)?)),
        (Str(s) | Link(s), "lower" | "toLowerCase") => Str(s.to_lowercase()),
        (Str(s) | Link(s), "upper" | "toUpperCase") => Str(s.to_uppercase()),
        (Str(s), "trim") => Str(s.trim().to_string()),
        (Str(s), "replace") => Str(s.replace(&arg_s(0)?, &arg_s(1)?)),
        (Str(s), "split") => List(s.split(&arg_s(0)?).map(|p| Str(p.to_string())).collect()),
        (Str(s), "title") => Str(s
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next().map_or(String::new(), |f| {
                    f.to_uppercase()
                        .chain(c.flat_map(char::to_lowercase))
                        .collect()
                })
            })
            .collect::<Vec<_>>()
            .join(" ")),
        (Str(s), "slice") => {
            let (from, to) = (
                a.first().and_then(Value::as_num).unwrap_or(0.0) as usize,
                a.get(1).and_then(Value::as_num).map(|n| n as usize),
            );
            Str(s
                .chars()
                .skip(from)
                .take(to.map_or(usize::MAX, |t| t.saturating_sub(from)))
                .collect())
        }
        (List(l), "contains") => Bool(l.iter().any(|x| loose_eq(x, &a[0]))),
        (List(l), "containsAny") => Bool(a.iter().any(|w| l.iter().any(|x| loose_eq(x, w)))),
        (List(l), "containsAll") => Bool(a.iter().all(|w| l.iter().any(|x| loose_eq(x, w)))),
        (List(l), "join") => Str(l
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(&arg_s(0).unwrap_or_else(|_| ", ".into()))),
        (List(l), "sort") => {
            let mut l = l.clone();
            l.sort_by(Value::compare);
            List(l)
        }
        (List(l), "reverse") => List(l.iter().rev().cloned().collect()),
        (List(l), "unique") => {
            let mut out: Vec<Value> = Vec::new();
            for x in l {
                if !out.iter().any(|y| loose_eq(x, y)) {
                    out.push(x.clone());
                }
            }
            List(out)
        }
        (List(l), "flat") => List(
            l.iter()
                .flat_map(|x| match x {
                    List(i) => i.clone(),
                    o => vec![o.clone()],
                })
                .collect(),
        ),
        (List(l), "slice") => {
            let (from, to) = (
                a.first().and_then(Value::as_num).unwrap_or(0.0) as usize,
                a.get(1)
                    .and_then(Value::as_num)
                    .map_or(l.len(), |n| n as usize),
            );
            List(
                l.iter()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .cloned()
                    .collect(),
            )
        }
        (Num(n), "round") => Num(round_to(
            *n,
            a.first().and_then(Value::as_num).unwrap_or(0.0),
        )),
        (Num(n), "floor") => Num(n.floor()),
        (Num(n), "ceil") => Num(n.ceil()),
        (Num(n), "abs") => Num(n.abs()),
        (Num(n), "toFixed") => Str(format!(
            "{:.*}",
            a.first().and_then(Value::as_num).unwrap_or(0.0) as usize,
            n
        )),
        (Date(d), "format") => Str(format_date(*d, &arg_s(0)?)),
        (Date(d), "date") => Date(d.div_euclid(DAY) * DAY),
        (Date(d), "time") => Str(format_date(*d, "HH:mm:ss")),
        (Null, _) => Null,
        _ => return Err(format!("{} has no method {name}", v.type_name())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::RawNote;

    fn world() -> (Index, BTreeMap<String, String>) {
        let n = RawNote {
            path: "projects/Alpha.md".into(),
            text: "---\nstatus: active\nprice: 12\ndue: 2024-06-10\ntags: [work]\nauthors: [a, b]\n---\nSee [[Beta]] #idea".into(),
            size: 100,
            ctime: 1_700_000_000_000,
            mtime: 1_700_086_400_000,
        };
        let b = RawNote {
            path: "Beta.md".into(),
            text: "".into(),
            ..Default::default()
        };
        let formulas = BTreeMap::from([
            ("double".to_string(), "price * 2".to_string()),
            ("loop".to_string(), "formula.loop".to_string()),
            (
                "label".to_string(),
                "if(price > 10, \"dear\", \"cheap\")".to_string(),
            ),
        ]);
        (Index::build(&[n, b]), formulas)
    }

    fn run(src: &str) -> Result<Value, String> {
        let (idx, f) = world();
        let ctx = Ctx::new(&idx, 0, &f, 1_717_977_600_000); // 2024-06-10
        eval_str(src, &ctx)
    }

    fn ok(src: &str) -> Value {
        run(src).unwrap_or_else(|e| panic!("{src}: {e}"))
    }

    #[test]
    fn literals_operators_and_precedence() {
        assert_eq!(ok("1 + 2 * 3"), Value::Num(7.0));
        assert_eq!(ok("(1 + 2) * 3"), Value::Num(9.0));
        assert_eq!(ok("10 % 4 - -1"), Value::Num(3.0));
        assert_eq!(ok("\"a\" + 1"), Value::Str("a1".into()));
        assert_eq!(ok("1 < 2 && !(3 < 2) || false"), Value::Bool(true));
        assert_eq!(ok("[1, 2][1]"), Value::Num(2.0));
        assert!(run("1 +").is_err() && run("@").is_err() && run("\"x").is_err());
    }

    #[test]
    fn properties_file_and_formulas() {
        assert_eq!(ok("status"), Value::Str("active".into()));
        assert_eq!(ok("status == \"active\" && price >= 12"), Value::Bool(true));
        assert_eq!(ok("missing"), Value::Null);
        assert_eq!(ok("note.status"), Value::Str("active".into()));
        assert_eq!(ok("file.name"), Value::Str("Alpha".into()));
        assert_eq!(ok("file.hasTag(\"work\")"), Value::Bool(true));
        assert_eq!(ok("file.hasTag(\"nope\", \"idea\")"), Value::Bool(true));
        assert_eq!(ok("file.inFolder(\"projects\")"), Value::Bool(true));
        assert_eq!(ok("file.hasLink(\"Beta\")"), Value::Bool(true));
        assert_eq!(ok("file.links.length"), Value::Num(1.0));
        assert_eq!(ok("formula.double"), Value::Num(24.0));
        assert_eq!(ok("formula.label"), Value::Str("dear".into()));
        assert!(run("formula.loop").unwrap_err().contains("itself"));
        assert!(run("formula.nope").is_err());
    }

    #[test]
    fn dates_and_durations() {
        assert_eq!(
            ok("(now() - file.mtime).days"),
            Value::Num(((1_717_977_600_000i64 - 1_700_086_400_000) / DAY) as f64)
        );
        assert_eq!(ok("due == date(\"2024-06-10\")"), Value::Bool(true));
        assert_eq!(ok("due.year"), Value::Num(2024.0));
        assert_eq!(
            ok("due.format(\"DD MMM YYYY\")"),
            Value::Str("10 Jun 2024".into())
        );
        assert_eq!(ok("due + \"1d\" > due"), Value::Bool(true));
        assert_eq!(ok("(today() - due).days"), Value::Num(0.0));
        assert_eq!(ok("duration(\"2 weeks\").days"), Value::Num(14.0));
    }

    #[test]
    fn strings_and_lists() {
        assert_eq!(ok("status.upper()"), Value::Str("ACTIVE".into()));
        assert_eq!(ok("status.contains(\"tiv\")"), Value::Bool(true));
        assert_eq!(ok("status.length"), Value::Num(6.0));
        assert_eq!(ok("status.slice(1, 3)"), Value::Str("ct".into()));
        assert_eq!(ok("authors.join(\"+\")"), Value::Str("a+b".into()));
        assert_eq!(ok("authors.contains(\"b\")"), Value::Bool(true));
        assert_eq!(ok("authors.containsAll(\"a\", \"c\")"), Value::Bool(false));
        assert_eq!(ok("missing.isEmpty()"), Value::Bool(true));
        assert_eq!(ok("if(true, 1, 1 / 0)"), Value::Num(1.0));
        assert_eq!(ok("round(3.14159, 2)"), Value::Num(314.0 / 100.0));
        assert_eq!(ok("max(1, 9, 3)"), Value::Num(9.0));
        assert!(run("status.nonsense()").is_err());
    }
}
