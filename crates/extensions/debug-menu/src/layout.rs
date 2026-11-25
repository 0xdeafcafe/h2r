//! The menu as lines for the host overlay (docs/runtime/debug-menu.md
//! "Layout"): a title bar with the breadcrumb, the pinned header (status)
//! right under it, the search being typed, a window of rows around the
//! cursor (label left, value right, a page's chevron in one column just
//! after the labels), what the selected row is or why it is greyed, the
//! call it makes, the last result and the window's own notices, and the
//! controls of the device last used. Parts are split by [`Tone::Rule`]
//! lines, which the overlay draws as thin lines, not characters. Fixed
//! width ([`COLS`]), so the overlay is one block.

use crate::model::{Device, Edit, Menu, PAGE};

/// Characters a line.
pub const COLS: usize = 64;
/// The most a value takes of a line.
pub const VALUE_COLS: usize = 40;
/// Rows shown at once; the rest scroll.
pub const ROWS: usize = PAGE;
/// Between the parts of the status line and of the controls.
pub const SEP: &str = " \u{b7} ";
/// The header takes at most this many lines.
const HEADER_LINES: usize = 2;
/// The selected row's description takes at most this many lines.
const HELP_LINES: usize = 2;
/// A result or a window notice takes at most this many lines.
const NOTICE_LINES: usize = 3;
/// Columns between a page row's label and its chevron.
const CHEVRON_GAP: usize = 2;

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The first line: drawn on the title bar.
    Title,
    Normal,
    /// The row under the cursor: drawn on a bar the panel's width.
    Selected,
    /// A row that can't change now (and the reason why).
    Greyed,
    /// Help, status and controls.
    Hint,
    /// An error from the last change, or a window notice about a failure.
    Error,
    /// What the last change did, or a window notice that something is ready.
    Done,
    /// A heading among the rows: its label, then a thin line to the edge.
    Heading,
    /// A thin line across the panel between its parts. Its text is empty.
    Rule,
}

/// One line of the overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub tone: Tone,
}

fn line(text: impl Into<String>, tone: Tone) -> Line {
    Line {
        text: text.into(),
        tone,
    }
}

/// A rule after the last line, unless there is one already, or nothing
/// but the title bar (which is its own edge).
fn push_rule(out: &mut Vec<Line>) {
    if out
        .last()
        .is_some_and(|l| !matches!(l.tone, Tone::Rule | Tone::Title))
    {
        out.push(line("", Tone::Rule));
    }
}

/// What to say about the controls, for the device last used.
pub fn controls(d: Device) -> &'static str {
    match d {
        Device::Keyboard => CONTROLS,
        Device::Touchpad => {
            "swipe move/change \u{b7} click select \u{b7} 2 fingers back \u{b7} Y star"
        }
        Device::Pad => {
            "D-pad move/change \u{b7} A select \u{b7} B back \u{b7} LB/RB page \u{b7} Y star"
        }
    }
}

/// The controls before any input: the keyboard's.
pub const CONTROLS: &str =
    "arrows move/change \u{b7} Enter select \u{b7} Esc back \u{b7} F star \u{b7} / search";

/// The lines for `m` (whether it is open or not: callers check).
pub fn lines(m: &Menu) -> Vec<Line> {
    lines_with(m, &[])
}

/// The lines for `m` with the window's own notices (a build banner, the
/// paired editor's status) inside the panel, above the controls, so
/// nothing is drawn over the open menu. A notice's text may hold several
/// lines (`\n`); its tone is kept.
pub fn lines_with(m: &Menu, notices: &[Line]) -> Vec<Line> {
    let mut out = Vec::new();
    let mut title = String::from("DEBUG MENU");
    for p in m.path() {
        title.push_str(" > ");
        title.push_str(p);
    }
    out.push(line(fit(&title, COLS), Tone::Title));
    let header = header_lines(m);
    if !header.is_empty() {
        out.extend(header);
        push_rule(&mut out);
    }
    if m.typing() || !m.filter().is_empty() {
        out.push(line(
            fit(&format!("search: {}_", m.filter()), COLS),
            Tone::Normal,
        ));
        push_rule(&mut out);
    }
    let rows = m.rows();
    let starred = m.starred();
    let cursor = m.cursor().min(rows.len().saturating_sub(1));
    let (from, to) = window(rows.len(), cursor, ROWS);
    // One chevron column for the pages shown, just after their longest
    // label, so a chevron sits by its label rather than at the far edge.
    let chevron_at = rows[from..to]
        .iter()
        .filter(|r| matches!(r.edit, Edit::Open(_)) && !r.heading)
        .map(|r| r.label.chars().count())
        .max()
        .unwrap_or(0)
        .min(COLS - 2 - CHEVRON_GAP - 1);
    if from > 0 {
        out.push(line(format!("  ... {from} more"), Tone::Hint));
    }
    for (i, r) in rows.iter().enumerate().take(to).skip(from) {
        if r.heading {
            out.push(line(fit(&format!("  {}", r.label), COLS), Tone::Heading));
            continue;
        }
        let mark = match (i == cursor, starred.get(i).copied().unwrap_or(false)) {
            (true, true) => ">*",
            (true, false) => "> ",
            (false, true) => " *",
            (false, false) => "  ",
        };
        let tone = match (i == cursor, r.disabled.is_some()) {
            (true, _) => Tone::Selected,
            (false, true) => Tone::Greyed,
            (false, false) => Tone::Normal,
        };
        if matches!(r.edit, Edit::Open(_)) {
            out.push(line(page_text(mark, &r.label, chevron_at), tone));
            continue;
        }
        let mut value = r.shown();
        if matches!(
            r.edit,
            Edit::Bool | Edit::Int { .. } | Edit::Float { .. } | Edit::Choice(_)
        ) && r.disabled.is_none()
            && r.set.is_some()
            && i == cursor
        {
            value = format!("< {value} >");
        }
        out.push(line(row_text(mark, &r.label, &value), tone));
    }
    if to < rows.len() {
        out.push(line(format!("  ... {} more", rows.len() - to), Tone::Hint));
    }
    push_rule(&mut out);
    if let Some(r) = rows.get(cursor) {
        let (text, tone) = match &r.disabled {
            Some(why) => (why.clone(), Tone::Greyed),
            None => (r.about.clone(), Tone::Hint),
        };
        for l in clip(wrap_phrases(&text, COLS), HELP_LINES) {
            out.push(line(l, tone));
        }
    }
    if let Some(c) = m.selected_call() {
        out.push(line(fit_mid(&format!("agent: {c}"), COLS), Tone::Hint));
    }
    if let Some(t) = m.toast() {
        out.extend(toast_line(t));
    }
    for n in notices {
        out.extend(notice_lines(&n.text, n.tone));
    }
    push_rule(&mut out);
    out.push(line(fit(controls(m.device()), COLS), Tone::Hint));
    out
}

/// The result toast alone, for the overlay while the menu is closed.
pub fn toast_lines(m: &Menu) -> Vec<Line> {
    m.toast().map(toast_line).unwrap_or_default()
}

fn toast_line(t: &crate::model::Toast) -> Vec<Line> {
    let tone = if t.error { Tone::Error } else { Tone::Done };
    notice_lines(&t.text, tone)
}

/// `text` (split at `\n`, each line wrapped on phrases) in at most
/// [`NOTICE_LINES`] lines of `tone`.
fn notice_lines(text: &str, tone: Tone) -> Vec<Line> {
    let wrapped: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .flat_map(|l| wrap_phrases(l, COLS))
        .collect();
    clip(wrapped, NOTICE_LINES)
        .into_iter()
        .map(|l| line(l, tone))
        .collect()
}

/// `lines` cut to `max`: the last one holds the rest, cut with `..`.
fn clip(mut lines: Vec<String>, max: usize) -> Vec<String> {
    if lines.len() > max && max > 0 {
        let rest = lines[max - 1..].join(" ");
        lines.truncate(max - 1);
        lines.push(fit(&rest, COLS));
    }
    lines
}

/// The pinned header: its rows as `label value`, side by side and split
/// by [`SEP`], in at most [`HEADER_LINES`] lines.
fn header_lines(m: &Menu) -> Vec<Line> {
    let parts: Vec<String> = m
        .header_rows()
        .iter()
        .map(|r| match (r.label.is_empty(), r.shown()) {
            (true, v) => v,
            (false, v) if v.is_empty() => r.label.clone(),
            (false, v) => format!("{} {v}", r.label),
        })
        // A row that is a whole status line (`sources::header`) breaks
        // between its parts, not mid-part, when it runs past a line.
        .flat_map(|s| s.split(SEP).map(str::to_owned).collect::<Vec<_>>())
        .filter(|s| !s.is_empty())
        .collect();
    let sep = SEP.chars().count();
    let mut lines: Vec<String> = Vec::new();
    for p in parts {
        let p = fit(&p, COLS);
        match lines.last_mut() {
            Some(l) if l.chars().count() + sep + p.chars().count() <= COLS => {
                l.push_str(SEP);
                l.push_str(&p);
            }
            _ => lines.push(p),
        }
    }
    lines
        .into_iter()
        .take(HEADER_LINES)
        .map(|l| line(l, Tone::Hint))
        .collect()
}

/// `[from, to)` of `n` rows showing `size` with `cursor` inside, kept
/// away from the edges when it can be.
pub fn window(n: usize, cursor: usize, size: usize) -> (usize, usize) {
    if n <= size {
        return (0, n);
    }
    let from = cursor.saturating_sub(size / 2).min(n - size);
    (from, from + size)
}

/// `mark label ..... value` in [`COLS`], the label cut first.
fn row_text(mark: &str, label: &str, value: &str) -> String {
    let value = fit_value(value, VALUE_COLS);
    let room = COLS - mark.chars().count() - value.chars().count() - 1;
    let label = fit(label, room);
    let pad = room - label.chars().count() + 1;
    format!("{mark}{label}{}{value}", " ".repeat(pad))
}

/// A page row: `mark label`, then its chevron [`CHEVRON_GAP`] columns
/// after column `at` (the longest page label shown), the label cut first.
fn page_text(mark: &str, label: &str, at: usize) -> String {
    let at = at.max(1);
    let label = fit(label, at);
    let pad = at - label.chars().count() + CHEVRON_GAP;
    format!("{mark}{label}{}>", " ".repeat(pad))
}

/// A value cut to `max`: a path keeps its start and its end.
fn fit_value(v: &str, max: usize) -> String {
    if v.contains('/') || v.contains('\\') {
        fit_mid(v, max)
    } else {
        fit(v, max)
    }
}

/// `s` cut to `max` characters, ending in `..` when cut.
pub fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    if max <= 2 {
        return ".".repeat(max);
    }
    s.chars().take(max - 2).chain("..".chars()).collect()
}

/// `s` cut to `max` characters with `..` in the middle, so a path keeps
/// its root and its file name.
pub fn fit_mid(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_owned();
    }
    if max <= 2 {
        return ".".repeat(max);
    }
    let keep = max - 2;
    let tail = keep / 2;
    let head = keep - tail;
    s.chars()
        .take(head)
        .chain("..".chars())
        .chain(s.chars().skip(n - tail))
        .collect()
}

/// `s` broken into lines of at most `width`, at spaces where it can be.
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let word: String = word.chars().take(width).collect();
        let need = cur.chars().count() + usize::from(!cur.is_empty()) + word.chars().count();
        if need > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `s` broken into lines of at most `width`: at the end of a phrase
/// (after `,` `;` `:` `.`, or before ` - `) when one is at least half a
/// line in, else at a space, else mid-word. Text that fits stays one
/// line; a longer one breaks where a reader would pause, not mid-phrase.
pub fn wrap_phrases(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s.split_whitespace().collect::<Vec<_>>().join(" ");
    while width > 0 && rest.chars().count() > width {
        let mut phrase = None;
        let mut word = None;
        for (i, c) in rest.char_indices() {
            if c != ' ' {
                continue;
            }
            let n = rest[..i].chars().count();
            if n > width {
                break;
            }
            word = Some(i);
            let ends_phrase =
                rest[..i].ends_with([',', ';', ':', '.']) || rest[i..].starts_with(" - ");
            if ends_phrase && n * 2 >= width {
                phrase = Some(i);
            }
        }
        match phrase.or(word) {
            Some(i) => {
                out.push(rest[..i].to_owned());
                rest = rest[i + 1..].to_owned();
            }
            None => {
                let head: String = rest.chars().take(width).collect();
                rest = rest[head.len()..].trim_start().to_owned();
                out.push(head);
            }
        }
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}
