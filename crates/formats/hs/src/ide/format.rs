//! The formatter: the decompiler's layout, kept to what a person wrote.
//! Each line is indented two spaces for each list open at its start
//! (less the `)`s it starts with); trailing spaces go, runs of blank lines
//! become one, and the file ends with one line break. Line breaks, the
//! order of tokens and comments stay as written; a block comment's inner
//! lines are left alone. Text that does not read is refused.

use super::syntax::{self, Tok};

pub const INDENT: &str = "  ";

pub fn format(src: &str) -> Result<String, String> {
    let tree = syntax::parse(src);
    if let Some(e) = tree.errors.first() {
        return Err(format!("not formatted: {}", e.message));
    }
    let lexemes = &tree.lexemes;
    // Block comments over more than one line.
    let blocks: Vec<(usize, usize)> = lexemes
        .iter()
        .filter(|lx| lx.kind == Tok::Comment && src[lx.start..lx.end].contains('\n'))
        .map(|lx| (lx.start, lx.end))
        .collect();
    let mut out = String::with_capacity(src.len());
    let mut depth = 0usize;
    let mut next = 0;
    let mut at = 0;
    let mut blank = false;
    let mut started = false;
    for line in src.split_inclusive('\n') {
        let end = at + line.len();
        // Inside a block comment that began on an earlier line: as it is.
        let in_comment = blocks.iter().any(|&(s, e)| s < at && at < e);
        let body = line.trim_end_matches(['\n', '\r']);
        if in_comment {
            out.push_str(body.trim_end());
            out.push('\n');
            blank = false;
        } else if body.trim().is_empty() {
            blank = started;
        } else {
            if blank {
                out.push('\n');
                blank = false;
            }
            let trimmed = body.trim();
            let closes = trimmed.bytes().take_while(|&b| b == b')').count();
            for _ in 0..depth.saturating_sub(closes) {
                out.push_str(INDENT);
            }
            out.push_str(trimmed);
            out.push('\n');
            started = true;
        }
        // The depth after this line.
        while next < lexemes.len() && lexemes[next].start < end {
            match lexemes[next].kind {
                Tok::Open => depth += 1,
                Tok::Close => depth = depth.saturating_sub(1),
                _ => {}
            }
            next += 1;
        }
        at = end;
    }
    Ok(out)
}
