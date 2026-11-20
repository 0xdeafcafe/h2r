//! Published text shares fixed retail rectangles with status marks. Budget
//! space by glyph width so wide names leave the same room for the mark.
//! The bounds are deliberately coarse and work without loading a disc font.
use crate::ui_kit::spec::wrap;

pub(super) const ROW: i32 = 190;
pub(super) const SUMMARY: i32 = 221;
// The header is 340 units in the larger title font.
pub(super) const TITLE: i32 = 218;

fn advance(c: char) -> i32 {
    match c {
        ' ' | '.' | ',' | ':' | ';' | 'i' | 'l' | '!' => 5,
        'w' | 'm' | 'W' | 'M' => 14,
        'a'..='z' | '0'..='9' => 8,
        c if c.is_ascii() => 12,
        c => 24 * c.len_utf16() as i32,
    }
}

pub(super) fn width(text: &str) -> i32 {
    text.chars().map(advance).sum()
}

fn ellipsis(text: &str, room: i32) -> String {
    let mut left = room - width("...");
    let prefix: String = text
        .chars()
        .take_while(|&c| {
            left -= advance(c);
            left >= 0
        })
        .collect();
    format!("{}...", prefix.trim_end())
}

pub(super) fn line(text: &str, room: i32) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if width(&text) <= room {
        text
    } else {
        ellipsis(&text, room)
    }
}

/// Wrap up to the available number of lines. A long unbroken word also
/// fits, and an ellipsis tells the player when there was more to read.
pub(super) fn paragraph(text: &str, room: i32, rows: usize) -> String {
    let words: Vec<_> = text
        .split_whitespace()
        .map(|word| line(word, room))
        .collect();
    let mut lines = wrap(&words.join(" "), room, &width);
    if lines.len() > rows {
        lines.truncate(rows);
        if let Some(last) = lines.last_mut() {
            *last = ellipsis(last, room);
        }
    }
    lines.join("\n")
}
