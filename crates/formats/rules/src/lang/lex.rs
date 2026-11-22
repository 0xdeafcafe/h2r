//! The lexer: source text to tokens with line and column.

use super::{CompileError, Pos};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok {
    /// A name or a keyword; the parser tells them apart.
    Ident(String),
    Int(i64),
    /// A 16.16 fixed literal (`1.5`).
    Fixed(i32),
    Str(String),
    Punct(&'static str),
    Eof,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub tok: Tok,
    pub pos: Pos,
    /// A line break comes before the token; it ends a statement.
    pub nl: bool,
}

pub(crate) static PUNCT: [&str; 24] = [
    "..", "==", "!=", "<=", ">=", "{", "}", "(", ")", "[", "]", ",", ":", ";", ".", "=", "<", ">",
    "+", "-", "*", "/", "%", "?",
];

/// The longest string literal, in bytes.
const MAX_STR: usize = 256;

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    lex_with(src, None)
}

/// Lexes on past errors, for the editor tooling (`crate::ide`): a bad
/// character is skipped, an unterminated string ends at its line and a
/// number out of range reads as 0. Every error is returned, in order.
pub fn lex_tolerant(src: &str) -> (Vec<Token>, Vec<CompileError>) {
    let mut errors = Vec::new();
    let tokens = lex_with(src, Some(&mut errors)).unwrap_or_default();
    (tokens, errors)
}

/// The lexer. With `errors`, an error is recorded there and lexing goes
/// on; without, the first error is returned.
fn lex_with(src: &str, mut errors: Option<&mut Vec<CompileError>>) -> Result<Vec<Token>, CompileError> {
    macro_rules! fail {
        ($e:expr) => {
            match errors.as_mut() {
                Some(list) => list.push($e),
                None => return Err($e),
            }
        };
    }
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1u32;
    let mut line_start = 0usize;
    let mut nl = true;
    while i < bytes.len() {
        let c = bytes[i];
        let pos = Pos {
            line,
            col: (i - line_start) as u32 + 1,
        };
        if c == b'\n' {
            i += 1;
            line += 1;
            line_start = i;
            nl = true;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let tok = if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            Tok::Ident(src[start..i].to_string())
        } else if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let whole: i64 = match src[start..i].parse() {
                Ok(n) => n,
                Err(_) => {
                    fail!(CompileError::at(pos, "number too large"));
                    0
                }
            };
            let fraction = bytes.get(i) == Some(&b'.')
                && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit());
            if fraction {
                i += 1;
                let fs = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let digits = &src[fs..i];
                if digits.len() > 9 {
                    fail!(CompileError::at(pos, "too many fraction digits"));
                    Tok::Fixed(0)
                } else {
                    let frac: i64 = digits.parse().unwrap_or(0);
                    let scale = 10i64.pow(digits.len() as u32);
                    let value = whole
                        .checked_mul(65536)
                        .and_then(|w| w.checked_add((frac * 65536 + scale / 2) / scale))
                        .filter(|v| *v <= i32::MAX as i64);
                    match value {
                        Some(v) => Tok::Fixed(v as i32),
                        None => {
                            fail!(CompileError::at(pos, "fixed value too large"));
                            Tok::Fixed(0)
                        }
                    }
                }
            } else if whole > u32::MAX as i64 {
                fail!(CompileError::at(pos, "number too large"));
                Tok::Int(0)
            } else {
                Tok::Int(whole)
            }
        } else if c == b'"' {
            i += 1;
            let mut s = String::new();
            let mut long = false;
            loop {
                let Some(&b) = bytes.get(i) else {
                    fail!(CompileError::at(pos, "unterminated string"));
                    break;
                };
                match b {
                    b'"' => {
                        i += 1;
                        break;
                    }
                    b'\n' => {
                        fail!(CompileError::at(pos, "unterminated string"));
                        break;
                    }
                    b'\\' => {
                        let esc = bytes.get(i + 1).copied();
                        match esc {
                            Some(b'"') => s.push('"'),
                            Some(b'\\') => s.push('\\'),
                            Some(b'n') => s.push('\n'),
                            _ => fail!(CompileError::at(pos, "unknown escape in string")),
                        }
                        // An escape of a line break or a multi-byte
                        // character skips the backslash alone.
                        i += if esc.is_some_and(|e| e.is_ascii() && e != b'\n') { 2 } else { 1 };
                    }
                    _ => {
                        // Copy one UTF-8 character.
                        let ch_len = utf8_len(b);
                        let end = (i + ch_len).min(bytes.len());
                        match src.get(i..end) {
                            Some(ch) => s.push_str(ch),
                            None => fail!(CompileError::at(pos, "invalid text in string")),
                        }
                        i = end;
                    }
                }
                if s.len() > MAX_STR && !long {
                    long = true;
                    fail!(CompileError::at(pos, "string longer than 256 bytes"));
                }
            }
            Tok::Str(s)
        } else {
            let rest = &src[i..];
            match PUNCT.iter().find(|p| rest.starts_with(**p)) {
                Some(p) => {
                    i += p.len();
                    Tok::Punct(*p)
                }
                None => {
                    let ch = rest.chars().next().unwrap_or('?');
                    fail!(CompileError::at(pos, format!("unexpected character {ch:?}")));
                    i += ch.len_utf8().max(1);
                    continue;
                }
            }
        };
        out.push(Token { tok, pos, nl });
        nl = false;
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: Pos {
            line,
            col: (bytes.len() - line_start) as u32 + 1,
        },
        nl: true,
    });
    Ok(out)
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Tok> {
        lex(src).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn numbers_ranges_and_fixed() {
        assert_eq!(
            toks("1..5 1.5"),
            vec![
                Tok::Int(1),
                Tok::Punct(".."),
                Tok::Int(5),
                Tok::Fixed(98304),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn positions_and_line_breaks() {
        let t = lex("a\n  b // c\n\"x\"").unwrap();
        assert_eq!(t[1].pos, Pos { line: 2, col: 3 });
        assert!(t[1].nl);
        assert_eq!(t[2].tok, Tok::Str("x".into()));
        assert_eq!(t[2].pos.line, 3);
    }

    #[test]
    fn errors_carry_line_and_column() {
        let e = lex("a\n  @").unwrap_err();
        assert_eq!((e.line, e.col), (2, 3));
        assert!(lex("\"open").is_err());
    }

    #[test]
    fn tolerant_lexing_skips_errors_and_reports_each() {
        let (toks, errors) = lex_tolerant("a @ b\n\"open\nc # 99999999999");
        let kinds: Vec<Tok> = toks.into_iter().map(|t| t.tok).collect();
        assert_eq!(
            kinds,
            vec![
                Tok::Ident("a".into()),
                Tok::Ident("b".into()),
                Tok::Str("open".into()),
                Tok::Ident("c".into()),
                Tok::Int(0),
                Tok::Eof
            ]
        );
        let places: Vec<(u32, u32)> = errors.iter().map(|e| (e.line, e.col)).collect();
        assert_eq!(places, vec![(1, 3), (2, 1), (3, 3), (3, 5)]);
        // A clean source lexes the same both ways.
        let src = "rules \"T\" 1\non kill(v, k) { score(k, 1.5) }";
        assert_eq!(lex_tolerant(src), (lex(src).unwrap(), Vec::new()));
    }
}
