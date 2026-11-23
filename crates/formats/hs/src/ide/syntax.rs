//! The working copy's text as tokens, a tree of lists, and LSP positions.
//! Tolerant: an unclosed list ends at the end of the text, a stray `)` is
//! skipped, and each is a [`SyntaxError`] with its place.

use super::{Position, Range};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tok {
    Open,
    Close,
    Str,
    Atom,
    Comment,
}

/// A token: its kind and byte span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Lexeme {
    pub kind: Tok,
    pub start: usize,
    pub end: usize,
}

/// A place the text does not read, with its code and sentence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub start: usize,
    pub end: usize,
    pub code: &'static str,
    pub message: String,
}

/// Bytes that end an atom.
fn delimiter(b: u8) -> bool {
    b.is_ascii_whitespace() || matches!(b, b'(' | b')' | b'"' | b';')
}

/// Every token of `src`, comments included, and what did not read: a
/// string or block comment with no end. A string ends at its line's end
/// at the latest.
pub(crate) fn lex(src: &str) -> (Vec<Lexeme>, Vec<SyntaxError>) {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut errors = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        match b[i] {
            c if c.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            b';' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                loop {
                    if i >= b.len() {
                        errors.push(SyntaxError {
                            start,
                            end: start + 2,
                            code: "syntax.comment",
                            message: "this comment has no closing *;".into(),
                        });
                        break;
                    }
                    if b[i] == b'*' && b.get(i + 1) == Some(&b';') {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                out.push(Lexeme { kind: Tok::Comment, start, end: i });
            }
            b';' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                out.push(Lexeme { kind: Tok::Comment, start, end: i });
            }
            b'(' => {
                i += 1;
                out.push(Lexeme { kind: Tok::Open, start, end: i });
            }
            b')' => {
                i += 1;
                out.push(Lexeme { kind: Tok::Close, start, end: i });
            }
            b'"' => {
                i += 1;
                let mut closed = false;
                while i < b.len() && b[i] != b'\n' {
                    match b[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            closed = true;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                i = i.min(b.len());
                // A backslash at a line's end steps past it.
                while !src.is_char_boundary(i) {
                    i -= 1;
                }
                if !closed {
                    errors.push(SyntaxError {
                        start,
                        end: i,
                        code: "syntax.string",
                        message: "this string has no closing quote on its line".into(),
                    });
                }
                out.push(Lexeme { kind: Tok::Str, start, end: i });
            }
            _ => {
                while i < b.len() && !delimiter(b[i]) {
                    i += 1;
                }
                out.push(Lexeme { kind: Tok::Atom, start, end: i });
            }
        }
    }
    (out, errors)
}

/// A node of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Node {
    List(List),
    Atom { start: usize, end: usize },
    Str { start: usize, end: usize },
}

impl Node {
    pub fn span(&self) -> (usize, usize) {
        match self {
            Node::List(l) => (l.open, l.end),
            Node::Atom { start, end } | Node::Str { start, end } => (*start, *end),
        }
    }

    pub fn atom<'a>(&self, src: &'a str) -> Option<&'a str> {
        match self {
            Node::Atom { start, end } => Some(&src[*start..*end]),
            _ => None,
        }
    }

    pub fn list(&self) -> Option<&List> {
        match self {
            Node::List(l) => Some(l),
            _ => None,
        }
    }
}

/// A parenthesised list: where it opens, where it closes (if it does),
/// and its items. `end` is past the `)`, or the end of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct List {
    pub open: usize,
    pub close: Option<usize>,
    pub end: usize,
    pub items: Vec<Node>,
}

/// The whole text: its top-level nodes, its comments, and what did not
/// read.
#[derive(Clone, Debug, Default)]
pub(crate) struct Tree {
    pub items: Vec<Node>,
    pub lexemes: Vec<Lexeme>,
    pub errors: Vec<SyntaxError>,
}

pub(crate) fn parse(src: &str) -> Tree {
    let (lexemes, mut errors) = lex(src);
    let mut top: Vec<Node> = Vec::new();
    let mut stack: Vec<List> = Vec::new();
    let push = |stack: &mut Vec<List>, top: &mut Vec<Node>, n: Node| match stack.last_mut() {
        Some(l) => l.items.push(n),
        None => top.push(n),
    };
    for lx in &lexemes {
        match lx.kind {
            Tok::Comment => {}
            Tok::Open => stack.push(List {
                open: lx.start,
                close: None,
                end: src.len(),
                items: Vec::new(),
            }),
            Tok::Close => match stack.pop() {
                Some(mut l) => {
                    l.close = Some(lx.start);
                    l.end = lx.end;
                    push(&mut stack, &mut top, Node::List(l));
                }
                None => errors.push(SyntaxError {
                    start: lx.start,
                    end: lx.end,
                    code: "syntax.close",
                    message: "this ) closes nothing".into(),
                }),
            },
            Tok::Atom => push(&mut stack, &mut top, Node::Atom { start: lx.start, end: lx.end }),
            Tok::Str => push(&mut stack, &mut top, Node::Str { start: lx.start, end: lx.end }),
        }
    }
    while let Some(l) = stack.pop() {
        errors.push(SyntaxError {
            start: l.open,
            end: l.open + 1,
            code: "syntax.open",
            message: "this ( is never closed".into(),
        });
        push(&mut stack, &mut top, Node::List(l));
    }
    errors.sort_by_key(|e| e.start);
    Tree { items: top, lexemes, errors }
}

/// Line starts, for LSP positions (UTF-16 columns).
pub(crate) struct Lines<'a> {
    src: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub fn new(src: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(src.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1));
        Lines { src, starts }
    }

    pub fn position(&self, byte: usize) -> Position {
        let byte = byte.min(self.src.len());
        let line = self.starts.partition_point(|&s| s <= byte) - 1;
        let mut from = self.starts[line];
        let mut at = byte;
        while !self.src.is_char_boundary(at) {
            at -= 1;
        }
        from = from.min(at);
        let character = self.src[from..at].encode_utf16().count() as u32;
        Position::new(line as u32, character)
    }

    pub fn offset(&self, pos: Position) -> usize {
        let Some(&start) = self.starts.get(pos.line as usize) else {
            return self.src.len();
        };
        let end = self.starts.get(pos.line as usize + 1).map_or(self.src.len(), |&e| e);
        let mut units = 0u32;
        for (i, c) in self.src[start..end].char_indices() {
            if units >= pos.character || c == '\n' {
                return start + i;
            }
            units += c.len_utf16() as u32;
        }
        end
    }

    pub fn range(&self, start: usize, end: usize) -> Range {
        Range {
            start: self.position(start),
            end: self.position(end),
        }
    }

    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.src.len())
    }
}
