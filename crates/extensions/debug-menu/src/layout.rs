//! The menu as lines of text for the host overlay: a title with the
//! breadcrumb, the pinned header (status), the search being typed, a
//! window of rows around the cursor (label left, value right), what the
//! selected row is or why it is greyed, the call it makes, the last
//! result or error, and the controls of the device last used. Fixed
//! width ([`COLS`]), so the overlay is one block.

use crate::model::{Device, Edit, Menu, PAGE};

/// Characters a line.
pub const COLS: usize = 64;
/// The most a value takes of a line.
pub const VALUE_COLS: usize = 40;
/// Rows shown at once; the rest scroll.
pub const ROWS: usize = PAGE;
/// The header takes at most this many lines.
const HEADER_LINES: usize = 2;

/// How a line is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Title,
    Normal,
    /// The row under the cursor.
    Selected,
    /// A row that can't change now (and the reason why).
    Greyed,
    /// Help and controls.
    Hint,
    /// An error from the last change.
    Error,
    /// What the last change did.
    Done,
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

/// What to say about the controls, for the device last used.
pub fn controls(d: Device) -> &'static str {
    match d {
        Device::Keyboard => "arrows move/change  Enter select  Esc back  F star  / search",
        Device::Touchpad => "swipe move/change  click select  2 fingers back  Y star",
        Device::Pad => "D-pad move/change  A select  B back  LB/RB page  Y star",
    }
}

/// The controls before any input: the keyboard's.
pub const CONTROLS: &str = "arrows move/change  Enter select  Esc back  F star  / search";

/// The lines for `m` (whether it is open or not: callers check).
pub fn lines(m: &Menu) -> Vec<Line> {
    let mut out = Vec::new();
    let mut title = String::from("DEBUG MENU");
    for p in m.path() {
        title.push_str(" > ");
        title.push_str(p);
    }
    out.push(line(fit(&title, COLS), Tone::Title));
    out.push(line("-".repeat(COLS), Tone::Hint));
    let header = header_lines(m);
    if !header.is_empty() {
        out.extend(header);
        out.push(line("-".repeat(COLS), Tone::Hint));
    }
    if m.typing() || !m.filter().is_empty() {
        out.push(line(fit(&format!("search: {}_", m.filter()), COLS), Tone::Title));
    }
    let rows = m.rows();
    let starred = m.starred();
    let cursor = m.cursor().min(rows.len().saturating_sub(1));
    let (from, to) = window(rows.len(), cursor, ROWS);
    if from > 0 {
        out.push(line(format!("  ... {from} more"), Tone::Hint));
    }
    for (i, r) in rows.iter().enumerate().take(to).skip(from) {
        if r.heading {
            out.push(line(fit(&format!("  -- {} --", r.label), COLS), Tone::Hint));
            continue;
        }
        let mark = match (i == cursor, starred.get(i).copied().unwrap_or(false)) {
            (true, true) => ">*",
            (true, false) => "> ",
            (false, true) => " *",
            (false, false) => "  ",
        };
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
        let tone = match (i == cursor, r.disabled.is_some()) {
            (true, _) => Tone::Selected,
            (false, true) => Tone::Greyed,
            (false, false) => Tone::Normal,
        };
        out.push(line(row_text(mark, &r.label, &value), tone));
    }
    if to < rows.len() {
        out.push(line(format!("  ... {} more", rows.len() - to), Tone::Hint));
    }
    out.push(line("-".repeat(COLS), Tone::Hint));
    if let Some(r) = rows.get(cursor) {
        let (text, tone) = match &r.disabled {
            Some(why) => (why.clone(), Tone::Greyed),
            None => (r.about.clone(), Tone::Hint),
        };
        for l in wrap(&text, COLS).into_iter().take(2) {
            out.push(line(l, tone));
        }
    }
    if let Some(c) = m.selected_call() {
        out.push(line(fit_mid(&format!("agent: {c}"), COLS), Tone::Hint));
    }
    if let Some(t) = m.toast() {
        out.push(toast_line(t));
    }
    out.push(line(fit(controls(m.device()), COLS), Tone::Hint));
    out
}

/// The result toast alone, for the overlay while the menu is closed.
pub fn toast_lines(m: &Menu) -> Vec<Line> {
    m.toast().map(toast_line).into_iter().collect()
}

fn toast_line(t: &crate::model::Toast) -> Line {
    let tone = if t.error { Tone::Error } else { Tone::Done };
    line(fit(&t.text, COLS), tone)
}

/// The pinned header: its rows as `label value`, side by side, in at
/// most [`HEADER_LINES`] lines.
fn header_lines(m: &Menu) -> Vec<Line> {
    let parts: Vec<String> = m
        .header_rows()
        .iter()
        .map(|r| match (r.label.is_empty(), r.shown()) {
            (true, v) => v,
            (false, v) if v.is_empty() => r.label.clone(),
            (false, v) => format!("{} {v}", r.label),
        })
        .filter(|s| !s.is_empty())
        .collect();
    let mut lines: Vec<String> = Vec::new();
    for p in parts {
        let p = fit(&p, COLS);
        match lines.last_mut() {
            Some(l) if l.chars().count() + 2 + p.chars().count() <= COLS => {
                l.push_str("  ");
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
