//! Folding ranges: bracketed spans over several lines, runs of comment
//! lines and `// region` ... `// endregion` pairs, from the lossless scan
//! (so a script that does not parse still folds).

use std::cmp::Reverse;

use super::model::Model;
use super::scan::K;
use super::{FoldingRange, fold_kind};

pub(crate) fn folding_ranges(m: &Model) -> Vec<FoldingRange> {
    let mut out = Vec::new();
    let line = |off: usize| m.li.line_of(off) as u32;

    // A block, a table, a list or an argument list: the opener's line to
    // the line before the closer, so the closer stays in sight.
    for (i, t) in m.code.iter().enumerate() {
        if t.k != K::Punct || !matches!(m.text(t), "{" | "(" | "[") {
            continue;
        }
        let Some(j) = m.partner(i).filter(|&j| j > i) else {
            continue;
        };
        let (start, end) = (line(t.s), line(m.code[j].s));
        if end > start + 1 {
            out.push(FoldingRange {
                start_line: start,
                end_line: end - 1,
                kind: None,
            });
        }
    }

    // Comments that are a whole line: region markers pair up, the rest
    // fold in runs of two or more.
    let mut regions: Vec<u32> = Vec::new();
    let mut run: Option<(u32, u32)> = None;
    let flush = |run: &mut Option<(u32, u32)>, out: &mut Vec<FoldingRange>| {
        if let Some((s, e)) = run.take()
            && e > s
        {
            out.push(FoldingRange {
                start_line: s,
                end_line: e,
                kind: Some(fold_kind::COMMENT),
            });
        }
    };
    for t in m.all.iter().filter(|t| t.k == K::Comment) {
        let l = line(t.s);
        let whole_line = m.li.line_text(l as usize).trim_start().starts_with("//");
        if !whole_line {
            continue;
        }
        let body = m.text(t).trim_start_matches('/').trim();
        let body = body.strip_prefix('#').unwrap_or(body);
        if marker(body, "region") {
            flush(&mut run, &mut out);
            regions.push(l);
            continue;
        }
        if marker(body, "endregion") {
            flush(&mut run, &mut out);
            if let Some(s) = regions.pop()
                && l > s
            {
                out.push(FoldingRange {
                    start_line: s,
                    end_line: l,
                    kind: Some(fold_kind::REGION),
                });
            }
            continue;
        }
        if let Some((s, e)) = run
            && e + 1 == l
        {
            run = Some((s, l));
        } else {
            flush(&mut run, &mut out);
            run = Some((l, l));
        }
    }
    flush(&mut run, &mut out);

    out.sort_by_key(|f| (f.start_line, Reverse(f.end_line)));
    out.dedup_by_key(|f| f.start_line);
    out
}

/// Whether a comment's text is the marker `word`, alone or followed by a
/// space and a name.
fn marker(body: &str, word: &str) -> bool {
    body.strip_prefix(word)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}
