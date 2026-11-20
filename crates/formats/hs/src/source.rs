use crate::{Catalog, Datum, Expression, Literal, Result, Scenario, invalid};
use std::collections::BTreeSet;
use std::fmt::Write;

pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Decompiled {
    pub source: String,
    /// Wire-node occurrences represented (call + name = two).
    pub emitted_nodes: usize,
    /// Unique wire slots represented; sharing can make this smaller.
    pub unique_nodes: usize,
}

impl Scenario {
    pub fn decompile(&self, catalog: &Catalog, script: Option<&str>) -> Result<Decompiled> {
        self.decompile_with_budget(catalog, script, MAX_SOURCE_BYTES)
    }

    /// Set a smaller output byte budget. Every append, including headers,
    /// escaping and closing delimiters, is checked before allocating it.
    /// This limits output bytes, not all working memory used by validation.
    pub fn decompile_with_budget(
        &self,
        catalog: &Catalog,
        script: Option<&str>,
        bytes: usize,
    ) -> Result<Decompiled> {
        self.validate(catalog)?;
        let selected = if let Some(name) = script {
            vec![self.script(name)?]
        } else {
            self.scripts.iter().collect()
        };
        let mut out = Renderer {
            scenario: self,
            source: Output::new(bytes.min(MAX_SOURCE_BYTES)),
            emitted: 0,
            seen: BTreeSet::new(),
            budget: self.nodes.len().saturating_mul(16).max(1024),
        };
        if script.is_none() {
            for g in &self.globals {
                out.source.push(&format!(
                    "(global {} {}\n",
                    symbol(catalog.type_name(g.value_type)?),
                    symbol(g.name()?)
                ))?;
                out.expression(g.root)?;
                out.source.push(")\n\n")?;
            }
        }
        for s in selected {
            out.source.push(&format!(
                "(script {}",
                symbol(&catalog.script_kinds[s.kind as usize])
            ))?;
            if matches!(s.kind, 3 | 4) {
                out.source
                    .push(&format!(" {}", symbol(catalog.type_name(s.return_type)?)))?;
            }
            out.source.push(&format!(" {}\n", symbol(s.name()?)))?;
            out.expression(s.root)?;
            out.source.push(")\n\n")?;
        }
        Ok(Decompiled {
            source: out.source.text,
            emitted_nodes: out.emitted,
            unique_nodes: out.seen.len(),
        })
    }
}
struct Output {
    text: String,
    limit: usize,
}
impl Output {
    fn new(limit: usize) -> Self {
        Self {
            text: String::new(),
            limit,
        }
    }
    fn push(&mut self, value: &str) -> Result<()> {
        if value.len() > self.limit.saturating_sub(self.text.len()) {
            return Err(invalid("source byte budget exceeded"));
        }
        self.text.push_str(value);
        Ok(())
    }
}
struct Renderer<'a> {
    scenario: &'a Scenario,
    source: Output,
    emitted: usize,
    seen: BTreeSet<Datum>,
    budget: usize,
}
enum Event {
    Expression(Datum, usize),
    Arguments(Datum, usize),
    Close,
}
impl Renderer<'_> {
    fn count(&mut self, datum: Datum) -> Result<()> {
        self.emitted += 1;
        if self.emitted > self.budget {
            return Err(invalid("source node expansion budget exceeded"));
        }
        self.seen.insert(datum);
        Ok(())
    }
    fn expression(&mut self, root: Datum) -> Result<()> {
        self.source.push("  ")?;
        let mut stack = vec![Event::Expression(root, 1)];
        while let Some(event) = stack.pop() {
            match event {
                Event::Close => self.source.push(")")?,
                Event::Arguments(datum, depth) => {
                    if datum == Datum::NULL {
                        continue;
                    }
                    let n = *self.scenario.node(datum)?;
                    if n.atom() {
                        self.source.push(" ")?;
                    } else {
                        self.source.push("\n")?;
                        for _ in 0..depth.min(32) {
                            self.source.push("  ")?;
                        }
                    }
                    stack.push(Event::Arguments(n.next, depth));
                    stack.push(Event::Expression(datum, depth));
                }
                Event::Expression(datum, depth) => {
                    self.count(datum)?;
                    let n = *self.scenario.node(datum)?;
                    if n.atom() {
                        self.source.push(&self.atom(datum)?)?;
                    } else {
                        let head_datum = Datum(n.data);
                        let head = *self.scenario.node(head_datum)?;
                        self.count(head_datum)?;
                        self.source.push("(")?;
                        self.source
                            .push(&symbol(self.scenario.string(head.string_offset)?))?;
                        stack.push(Event::Close);
                        stack.push(Event::Arguments(head.next, depth + 1));
                    }
                }
            }
        }
        Ok(())
    }
    fn atom(&self, datum: Datum) -> Result<String> {
        let literal = match self.scenario.expression(datum)? {
            Expression::Global { name, .. } => return Ok(symbol(name)),
            Expression::Atom { literal, .. } => literal,
            _ => return Err(invalid("expected atom")),
        };
        Ok(match literal {
            Literal::Void => "none".into(),
            Literal::Boolean(value) => value.to_string(),
            Literal::RealBits(bits) => {
                let value = f32::from_bits(bits);
                if value.is_finite() {
                    format!("{value:?}")
                } else {
                    format!("#real-bits:{bits:08x}")
                }
            }
            Literal::Short(value) => value.to_string(),
            Literal::Long(value) => value.to_string(),
            Literal::String(bytes) => quoted_bytes(bytes),
            Literal::Symbol { spelling, .. } | Literal::FunctionName(spelling) => symbol(spelling),
        })
    }
}
fn symbol(s: &str) -> String {
    if s.is_empty()
        || s.chars()
            .any(|c| c.is_whitespace() || c.is_control() || "()\";\\".contains(c))
    {
        quoted(s)
    } else {
        s.to_owned()
    }
}
fn quoted_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            32..=126 => out.push(b as char),
            _ => write!(out, "\\x{b:02x}").unwrap(),
        }
    }
    out.push('"');
    out
}
fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => write!(out, "\\u{{{:x}}}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Independently tokenize and count the rendered Lisp-like definitions.
/// This is a node-count check, not compilation back to identical
/// datums, inferred types, resource IDs or salt/allocation history.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceCounts {
    pub scripts: usize,
    pub globals: usize,
    pub nodes: usize,
}
pub fn source_counts(source: &str) -> Result<SourceCounts> {
    #[derive(PartialEq)]
    enum Token<'a> {
        Open,
        Close,
        Atom(&'a str),
    }
    let mut tokens = Vec::new();
    let b = source.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            c if c.is_ascii_whitespace() => i += 1,
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            b')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                loop {
                    if i >= b.len() {
                        return Err(invalid("unterminated source string"));
                    }
                    match b[i] {
                        b'\\' => {
                            i += 2;
                        }
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                tokens.push(Token::Atom(&source[start..i]));
            }
            _ => {
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !b"();\"".contains(&b[i]) {
                    i += 1;
                }
                tokens.push(Token::Atom(&source[start..i]));
            }
        }
    }
    let mut result = SourceCounts::default();
    let mut at = 0;
    while at < tokens.len() {
        if tokens.get(at) != Some(&Token::Open) {
            return Err(invalid("expected source definition"));
        }
        at += 1;
        let header = match tokens.get(at) {
            Some(Token::Atom("global")) => {
                result.globals += 1;
                3
            }
            Some(Token::Atom("script")) => {
                result.scripts += 1;
                if matches!(tokens.get(at + 1), Some(Token::Atom("static" | "stub"))) {
                    4
                } else {
                    3
                }
            }
            _ => return Err(invalid("expected script/global definition")),
        };
        if !(at..at + header).all(|i| matches!(tokens.get(i), Some(Token::Atom(_)))) {
            return Err(invalid("malformed definition header"));
        }
        at += header;
        // Nonrecursive expression parser. Each call list requires a head atom;
        // its opening list and head atom together represent two wire nodes.
        let mut depth = 0usize;
        loop {
            match tokens.get(at) {
                Some(Token::Open) => {
                    result.nodes += 2;
                    if !matches!(tokens.get(at + 1), Some(Token::Atom(_))) {
                        return Err(invalid("call has no function atom"));
                    }
                    depth += 1;
                    at += 2;
                }
                Some(Token::Atom(_)) => {
                    result.nodes += 1;
                    at += 1;
                    if depth == 0 {
                        break;
                    }
                }
                Some(Token::Close) if depth != 0 => {
                    depth -= 1;
                    at += 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => return Err(invalid("malformed source expression")),
            }
        }
        if tokens.get(at) != Some(&Token::Close) {
            return Err(invalid("definition must have one root expression"));
        }
        at += 1;
    }
    Ok(result)
}

#[cfg(test)]
mod budget_tests {
    #[test]
    fn failed_append_never_overshoots_or_changes_output() {
        let mut out = super::Output::new(4);
        out.push("abc").unwrap();
        assert!(out.push("expanded atom").is_err());
        assert_eq!(out.text, "abc");
        out.push(")").unwrap();
        assert_eq!(out.text.len(), 4);
        assert!(out.push("\n").is_err());
    }
}
