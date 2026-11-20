//! The menu as lines of text for the host overlay: a title with the
//! breadcrumb, a window of rows around the cursor (label left, value
//! right), what the selected row is or why it is greyed, the last error,
//! and the controls. Fixed width ([`COLS`]), so the overlay is one block.

use crate::model::{Edit, Menu};

/// Characters a line.
pub const COLS: usize = 48;
/// Rows shown at once; the rest scroll.
pub const ROWS: usize = 14;

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

/// What to say about the controls.
pub const CONTROLS: &str = "swipe move/change, click select, 2 fingers back";

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
    let rows = m.rows();
    let cursor = m.cursor().min(rows.len().saturating_sub(1));
    let (from, to) = window(rows.len(), cursor, ROWS);
    if from > 0 {
        out.push(line(format!("  ... {from} more"), Tone::Hint));
    }
    for (i, r) in rows.iter().enumerate().take(to).skip(from) {
        let mark = if i == cursor { "> " } else { "  " };
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
    if let Some(s) = m.status() {
        out.push(line(fit(s, COLS), Tone::Error));
    }
    out.push(line(fit(CONTROLS, COLS), Tone::Hint));
    out
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
    let value = fit(value, COLS / 2);
    let room = COLS - mark.len() - value.chars().count() - 1;
    let label = fit(label, room);
    let pad = room - label.chars().count() + 1;
    format!("{mark}{label}{}{value}", " ".repeat(pad))
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
