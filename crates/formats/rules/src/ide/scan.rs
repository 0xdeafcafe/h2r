//! A lossless scan of the source: every token with its byte span,
//! comments included, and never an error. Where the source lexes, its
//! tokens other than comments are the lexer's, one for one.

use crate::lang::lex::PUNCT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum K {
    Ident,
    Num,
    Str,
    Punct,
    Comment,
    /// A character the lexer refuses.
    Bad,
}

/// A token: its kind and byte span `s..e`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tk {
    pub k: K,
    pub s: usize,
    pub e: usize,
}

pub(crate) fn scan(src: &str) -> Vec<Tk> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let s = i;
        let k = if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            K::Comment
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            K::Ident
        } else if c.is_ascii_digit() {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if b.get(i) == Some(&b'.') && b.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            K::Num
        } else if c == b'"' {
            i += 1;
            loop {
                match b.get(i) {
                    None | Some(b'\n') => break,
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') => {
                        let ascii = b.get(i + 1).is_some_and(|e| e.is_ascii() && *e != b'\n');
                        i += if ascii { 2 } else { 1 };
                    }
                    Some(_) => i += 1,
                }
            }
            K::Str
        } else if let Some(p) = PUNCT.iter().find(|p| src[i..].starts_with(**p)) {
            i += p.len();
            K::Punct
        } else {
            let ch = src[i..].chars().next().map_or(1, char::len_utf8);
            i += ch;
            K::Bad
        };
        out.push(Tk { k, s, e: i });
    }
    out
}

/// Whether a string token ends with its closing quote.
pub(crate) fn closed(src: &str, t: &Tk) -> bool {
    let text = &src[t.s..t.e];
    if text.len() < 2 || !text.ends_with('"') {
        return false;
    }
    // A final `\"` is an escaped quote, unless the backslash is escaped.
    let body = &text[1..text.len() - 1];
    let backslashes = body.bytes().rev().take_while(|&c| c == b'\\').count();
    backslashes % 2 == 0
}
