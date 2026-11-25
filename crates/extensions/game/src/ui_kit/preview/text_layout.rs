//! A text widget's lines as the game lays them out (docs/formats/ui.md
//! "Text layout"): where a line breaks in its rectangle and which lines
//! are drawn. From the line measure `0x13ef40`, its break test `0x13fd20`
//! and the draw loop `0x13f0e0`; a UI text widget asks for wrapping
//! (`0x22ccc9` sets the request's wrap bit).
//!
//! Pure: the caller supplies the advances, so a test needs no font.

/// One laid-out line: its characters and the width the game justifies it
/// by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub width: i32,
}

/// The characters a line may break after (the 12 ranges at `0x4536cc`).
fn is_space(c: char) -> bool {
    matches!(
        u32::from(c),
        0x09..=0x0d
            | 0x20
            | 0x85
            | 0xa0
            | 0x1680
            | 0x180e
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
    )
}

/// The wide scripts a line may break inside (`0x13fd90`).
fn is_wide(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1100..=0x11ff | 0x3000..=0xd7af | 0xf900..=0xfaff | 0xff00..=0xffdc
    )
}

/// Characters that never start a line (`0x13fde0`), up to U+2103. The
/// test's entries for the wide scripts are not carried: the previews draw
/// the English strings.
fn never_starts(c: char) -> bool {
    matches!(
        c,
        '!' | '"' | '%' | ')' | ',' | '.' | ':' | ';' | '?' | ']' | '}'
            | '\u{b0}'
            | '\u{b7}'
            | '\u{2013}'
            | '\u{2014}'
            | '\u{2019}'
            | '\u{201d}'
            | '\u{2022}'
            | '\u{2026}'
            | '\u{2027}'
            | '\u{2032}'
            | '\u{2033}'
            | '\u{2103}'
    )
}

/// Characters that never end a line (`0x140260`), up to U+2035; the wide
/// scripts' entries are not carried, as for [`never_starts`].
fn never_ends(c: char) -> bool {
    matches!(
        c,
        '"' | '$' | '(' | '[' | '\\' | '{' | '\u{2018}' | '\u{201c}' | '\u{2035}'
    )
}

/// Whether a line may break between `prev` and `next` (`0x13fd20`):
/// after a space or beside a wide character, unless `next` never starts
/// a line or `prev` never ends one.
pub fn breaks_between(prev: char, next: char) -> bool {
    (is_space(prev) || is_wide(prev) || is_wide(next)) && !never_starts(next) && !never_ends(prev)
}

/// Units the scanner (`0x13f5a0`) reads without drawing: controls, and
/// its alignment units U+E405 to U+E407.
fn skipped(c: char) -> bool {
    c < ' ' || ('\u{e405}'..='\u{e407}').contains(&c)
}

/// `text` broken into the lines of a rectangle `room` wide.
/// `advance(prev, c)` is the pen's travel for `c` after `prev` (kerning,
/// x offset and advance).
///
/// As `0x13ef40` measures a line: a glyph joins it while `room` is more
/// than the line's width with that glyph. The first glyph that fails ends
/// the line at the last place a break was allowed ([`breaks_between`]),
/// the space before it staying on the line and in its width. With no
/// such place the line ends after the failing glyph, which is drawn but
/// not measured. A line's first glyph always fits. `\r`, `\n` and `\r\n`
/// end a line (the game's strings use `\r\n` and its scanner ignores
/// `\n`; ours use `\n`). Kerning starts afresh on each line, as the
/// rasteriser draws one.
// Each `start =` in the loop is followed by `continue 'line`, which
// starts a new range from it.
#[allow(clippy::mut_range_bound)]
pub fn lines(text: &str, room: i32, advance: &dyn Fn(Option<char>, char) -> i32) -> Vec<Line> {
    let mut chars: Vec<char> = Vec::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                if it.peek() == Some(&'\n') {
                    it.next();
                }
                chars.push('\r');
            }
            '\n' => chars.push('\r'),
            c => chars.push(c),
        }
    }
    let n = chars.len();
    let mut out = Vec::new();
    let push = |out: &mut Vec<Line>, from: usize, to: usize, width: i32| {
        out.push(Line {
            text: chars[from..to].iter().filter(|c| !skipped(**c)).collect(),
            width,
        });
    };
    let mut start = 0;
    'line: while start < n {
        let (mut width, mut count) = (0i32, 0usize);
        let mut brk: Option<(usize, i32)> = None;
        // The unit before this one, and the glyph before it on this line.
        let (mut before, mut glyph): (Option<char>, Option<char>) = (None, None);
        for i in start..n {
            let c = chars[i];
            if c == '\r' {
                push(&mut out, start, i, width);
                start = i + 1;
                continue 'line;
            }
            if skipped(c) {
                before = Some(c);
                continue;
            }
            // The measure's break index is unset at a line's first unit.
            if i > start && before.is_some_and(|p| breaks_between(p, c)) {
                brk = Some((i, width));
            }
            let a = advance(glyph, c);
            if width + a >= room && count > 0 {
                match brk {
                    Some((at, w)) => {
                        push(&mut out, start, at, w);
                        start = at;
                    }
                    None => {
                        push(&mut out, start, i + 1, width);
                        start = i + 1;
                    }
                }
                continue 'line;
            }
            width += a;
            count += 1;
            before = Some(c);
            glyph = Some(c);
        }
        push(&mut out, start, n, width);
        break;
    }
    out
}

/// How many lines a rectangle `height` high draws (`0x13f331`): line `n`
/// is drawn while the rectangle's bottom is below its baseline, `ascent`
/// plus `n` line heights under the top.
pub fn lines_drawn(height: i32, ascent: i32, line_height: i32) -> usize {
    if height <= ascent {
        return 0;
    }
    ((height - ascent - 1) / line_height.max(1)) as usize + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every glyph 10 wide, no kerning.
    fn ten(_: Option<char>, _: char) -> i32 {
        10
    }

    fn texts(l: &[Line]) -> Vec<&str> {
        l.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn a_line_breaks_at_the_last_space_that_fits_and_keeps_it() {
        // "ab cd" is 50 wide; the space after it makes 60, which does not
        // fit in 60, so the line goes back to the break before "cd".
        let l = lines("ab cd ef", 60, &ten);
        assert_eq!(texts(&l), ["ab ", "cd ef"]);
        // The space stays on the line and in the width it is justified by.
        assert_eq!(l[0].width, 30);
        assert_eq!(l[1].width, 50);
    }

    #[test]
    fn text_that_fits_is_one_line() {
        let l = lines("ab cd", 51, &ten);
        assert_eq!(texts(&l), ["ab cd"]);
        assert_eq!(l[0].width, 50);
        // A width that only equals the room does not fit.
        assert_eq!(texts(&lines("ab cd", 50, &ten)), ["ab ", "cd"]);
    }

    #[test]
    fn a_word_with_no_break_keeps_the_glyph_that_crossed_the_edge() {
        let l = lines("abcdefgh", 35, &ten);
        // Three fit; the fourth is drawn on the line but not measured.
        assert_eq!(texts(&l), ["abcd", "efgh"]);
        assert_eq!(l[0].width, 30);
    }

    #[test]
    fn a_first_glyph_always_fits() {
        let l = lines("abc", 5, &ten);
        assert_eq!(texts(&l), ["ab", "c"]);
    }

    #[test]
    fn a_break_never_parts_a_word_from_its_punctuation() {
        // "b ," may not break before the comma, so the break is after "a ".
        assert!(!breaks_between(' ', ','));
        assert!(!breaks_between('(', 'a'));
        assert!(breaks_between(' ', 'a'));
        assert!(!breaks_between('a', 'b'));
        let l = lines("a b ,c", 45, &ten);
        assert_eq!(texts(&l), ["a ", "b ,c"]);
    }

    #[test]
    fn returns_and_newlines_end_a_line() {
        assert_eq!(texts(&lines("a\r\nb\nc\rd", 1000, &ten)), ["a", "b", "c", "d"]);
        assert_eq!(texts(&lines("a\n\nb", 1000, &ten)), ["a", "", "b"]);
        assert!(lines("", 100, &ten).is_empty());
    }

    #[test]
    fn a_rect_draws_the_lines_whose_baseline_is_above_its_bottom() {
        // Ascent 12, lines 16 apart: baselines at 12, 28, 44.
        assert_eq!(lines_drawn(12, 12, 16), 0);
        assert_eq!(lines_drawn(13, 12, 16), 1);
        assert_eq!(lines_drawn(28, 12, 16), 1);
        assert_eq!(lines_drawn(29, 12, 16), 2);
        assert_eq!(lines_drawn(45, 12, 16), 3);
    }
}
