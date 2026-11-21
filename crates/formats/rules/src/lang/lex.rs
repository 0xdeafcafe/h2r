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

static PUNCT: [&str; 23] = [
    "..", "==", "!=", "<=", ">=", "{", "}", "(", ")", "[", "]", ",", ":", ";", ".", "=", "<", ">",
    "+", "-", "*", "/", "%",
];

/// The longest string literal, in bytes.
const MAX_STR: usize = 256;

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
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
            let whole: i64 = src[start..i]
                .parse()
                .map_err(|_| CompileError::at(pos, "number too large"))?;
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
                    return Err(CompileError::at(pos, "too many fraction digits"));
                }
                let frac: i64 = digits.parse().unwrap_or(0);
                let scale = 10i64.pow(digits.len() as u32);
                let value = whole
                    .checked_mul(65536)
                    .and_then(|w| w.checked_add((frac * 65536 + scale / 2) / scale))
                    .filter(|v| *v <= i32::MAX as i64)
                    .ok_or_else(|| CompileError::at(pos, "fixed value too large"))?;
                Tok::Fixed(value as i32)
            } else {
                if whole > u32::MAX as i64 {
                    return Err(CompileError::at(pos, "number too large"));
                }
                Tok::Int(whole)
            }
        } else if c == b'"' {
            i += 1;
            let mut s = String::new();
            loop {
                let Some(&b) = bytes.get(i) else {
                    return Err(CompileError::at(pos, "unterminated string"));
                };
                match b {
                    b'"' => {
                        i += 1;
                        break;
                    }
                    b'\n' => return Err(CompileError::at(pos, "unterminated string")),
                    b'\\' => {
                        let esc = bytes.get(i + 1).copied();
                        match esc {
                            Some(b'"') => s.push('"'),
                            Some(b'\\') => s.push('\\'),
                            Some(b'n') => s.push('\n'),
                            _ => return Err(CompileError::at(pos, "unknown escape in string")),
                        }
                        i += 2;
                    }
                    _ => {
                        // Copy one UTF-8 character.
                        let ch_len = utf8_len(b);
                        let end = (i + ch_len).min(bytes.len());
                        match src.get(i..end) {
                            Some(ch) => s.push_str(ch),
                            None => return Err(CompileError::at(pos, "invalid text in string")),
                        }
                        i = end;
                    }
                }
                if s.len() > MAX_STR {
                    return Err(CompileError::at(pos, "string longer than 256 bytes"));
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
                    return Err(CompileError::at(pos, format!("unexpected character {ch:?}")));
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
}
