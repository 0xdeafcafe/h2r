//! The canonical layout: four-space indents by bracket depth, one
//! statement per line, one space around operators and after commas and
//! colons, at most one blank line in a row, comments kept where they are.
//!
//! The formatter only moves line breaks where the grammar ignores them
//! (after `{`, around a block's `}`, at `;`) and only changes spaces
//! within a line, so the token sequence is the same but for `;`. It
//! checks that: a script that compiled compiles to the same blob after.

use super::scan::{self, K, Tk};
use crate::lang::parse::KEYWORDS;
use crate::lang::{CompileError, Pos, lex, parse};

/// Formats a script that parses. A source that does not parse is
/// returned as its first error.
pub fn format(src: &str) -> Result<String, CompileError> {
    let tokens = lex::lex(src)?;
    parse::parse(&tokens)?;
    let out = reflow(src);
    let changed = || {
        CompileError::at(
            Pos { line: 1, col: 1 },
            "the formatter would change what the script means; the source is left as it is",
        )
    };
    match crate::lang::compile(src) {
        Ok(before) => match crate::lang::compile(&out) {
            Ok(after) if after.encode() == before.encode() => {}
            _ => return Err(changed()),
        },
        Err(_) => {
            let again = lex::lex(&out).map_err(|_| changed())?;
            parse::parse(&again).map_err(|_| changed())?;
        }
    }
    Ok(out)
}

fn text_of<'a>(src: &'a str, t: &Tk) -> &'a str {
    &src[t.s..t.e]
}

fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w)
}

/// Lays the tokens out again.
fn reflow(src: &str) -> String {
    let toks = scan::scan(src);
    let text = |t: &Tk| text_of(src, t);
    let is_p = |t: &Tk, p: &str| t.k == K::Punct && text_of(src, t) == p;

    // An enum's braces stay inline; every other brace holds a block.
    let mut enum_brace = vec![false; toks.len()];
    {
        let code: Vec<usize> = (0..toks.len()).filter(|&i| toks[i].k != K::Comment).collect();
        let mut stack: Vec<bool> = Vec::new();
        for (c, &ti) in code.iter().enumerate() {
            if is_p(&toks[ti], "{") {
                let is_enum = c >= 2
                    && toks[code[c - 2]].k == K::Ident
                    && text(&toks[code[c - 2]]) == "enum"
                    && toks[code[c - 1]].k == K::Ident;
                stack.push(is_enum);
                enum_brace[ti] = is_enum;
            } else if is_p(&toks[ti], "}") {
                enum_brace[ti] = stack.pop().unwrap_or(false);
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_empty = true;
    let mut depth = 0usize;
    let mut need_break = false;
    let mut last_end = 0usize;
    // The last two code tokens emitted, and whether the last was a block's
    // `}` or a unary minus.
    let mut pc: Option<Tk> = None;
    let mut ppc: Option<Tk> = None;
    let mut after_block_close = false;
    let mut after_unary = false;

    let flush = |out: &mut Vec<String>, cur: &mut String, cur_empty: &mut bool| {
        if !*cur_empty {
            out.push(cur.trim_end().to_string());
            cur.clear();
            *cur_empty = true;
        }
    };

    for (ti, t) in toks.iter().enumerate() {
        let tx = text(t);
        let newlines = src[last_end..t.s].matches('\n').count();
        last_end = t.e;
        if is_p(t, ";") {
            need_break = true;
            continue;
        }
        let comment = t.k == K::Comment;
        let block_close = is_p(t, "}") && !enum_brace[ti];
        let joins_else = after_block_close && t.k == K::Ident && tx == "else";

        if comment && newlines == 0 && !cur_empty {
            // A trailing comment stays on its line.
        } else if joins_else {
            need_break = false;
        } else if newlines > 0 || need_break || block_close {
            flush(&mut out, &mut cur, &mut cur_empty);
            need_break = false;
            let keep_blank = newlines >= 2
                && !block_close
                && out.last().is_some_and(|l| !l.is_empty() && !l.ends_with('{'));
            if keep_blank {
                out.push(String::new());
            }
        }

        let closer = t.k == K::Punct && matches!(tx, "}" | ")" | "]");
        if closer {
            depth = depth.saturating_sub(1);
        }
        let piece = if comment { tx.trim_end() } else { tx };
        if cur_empty {
            cur.push_str(&"    ".repeat(depth));
            cur.push_str(piece);
            cur_empty = false;
        } else {
            if space_between(pc.map(|p| (p.k, text_of(src, &p))), after_unary, t.k, tx) {
                cur.push(' ');
            }
            cur.push_str(piece);
        }
        if t.k == K::Punct && matches!(tx, "{" | "(" | "[") {
            depth += 1;
        }
        if is_p(t, "{") && !enum_brace[ti] {
            need_break = true;
        }
        if block_close {
            need_break = true;
        }
        after_block_close = block_close;
        if !comment {
            // A minus is unary after an operator, an opening bracket, a
            // comma, a keyword, the start, or a declaration's type
            // (`option x: int -5..5`).
            after_unary = is_p(t, "-")
                && match pc {
                    None => true,
                    Some(p) => match p.k {
                        K::Punct => !matches!(text(&p), ")" | "]" | "}"),
                        K::Ident => is_keyword(text(&p)) || ppc.is_some_and(|q| is_p(&q, ":")),
                        _ => false,
                    },
                };
            ppc = pc;
            pc = Some(*t);
        } else {
            after_unary = false;
        }
    }
    flush(&mut out, &mut cur, &mut cur_empty);
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        return String::new();
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Whether a space goes between the previous code token and this one on
/// the same line.
fn space_between(prev: Option<(K, &str)>, after_unary: bool, k: K, tx: &str) -> bool {
    let Some((pk, ptx)) = prev else {
        return true;
    };
    if k == K::Comment {
        return true;
    }
    if after_unary {
        return false;
    }
    if pk == K::Punct && matches!(ptx, "(" | "[" | "." | "..") {
        return false;
    }
    if k == K::Punct && matches!(tx, ")" | "]" | "," | ":" | "." | "..") {
        return false;
    }
    let callee = pk == K::Ident && !is_keyword(ptx);
    if k == K::Punct && tx == "(" {
        return !callee;
    }
    if k == K::Punct && tx == "[" {
        return !(callee || (pk == K::Punct && matches!(ptx, ")" | "]")));
    }
    true
}
