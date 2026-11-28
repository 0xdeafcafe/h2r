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

/// A segment's justification: the scanner's `+0x10`, which the
/// alignment units U+E405, U+E406 and U+E407 set to left, right and
/// centre (`0x13f614`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Centre,
}

/// What `0x13f0e0` lays a string out in, guest pixels: the rectangle's
/// left and right, the tab stops (canvas pixels), the first row group's
/// and the rest's indents, the advance scale and the justification it
/// starts with. Stops and indents are times the scale, as every metric.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame<'a> {
    pub left: f32,
    pub right: f32,
    pub stops: &'a [i16],
    pub indents: [i16; 2],
    pub scale: f32,
    pub align: Align,
}

/// One measured piece of a row: its characters, where its pen starts
/// once justified, and its row (the row groups' rows above it plus its
/// own in its group).
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub text: String,
    pub x: f32,
    pub row: i32,
}

/// Why the measure (`0x13ef40`) ended a segment: the scanner's type at
/// the unit that ended it (`0x13f5a0`: 0 the end, 1 `\r`, 2 a tab, 3 an
/// alignment unit, 4 a character that did not fit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    Text,
    Line,
    Tab,
    Align(Align),
    Wrap,
}

/// One segment from `start` as `0x13ef40` measures it, the pen at `pen`
/// and the column's right edge at `right`: the units it draws (`start` to
/// the first returned), where the next starts, its width and why it
/// ended. `parts(prev, c)` is the glyph's scaled advance, x offset and
/// kerning; a glyph fits while the right edge is past
/// `pen + advance + x offset + kerning + width`, summed in that order.
fn measure(
    chars: &[char],
    start: usize,
    pen: f32,
    right: f32,
    parts: &dyn Fn(Option<char>, char) -> [f32; 3],
) -> (usize, usize, f32, End) {
    let (mut width, mut count) = (0f32, 0usize);
    let mut brk: Option<(usize, f32)> = None;
    let (mut before, mut glyph): (Option<char>, Option<char>) = (None, None);
    for (i, &c) in chars.iter().enumerate().skip(start) {
        let end = match c {
            '\r' => Some(End::Line),
            '\t' => Some(End::Tab),
            '\u{e405}' => Some(End::Align(Align::Left)),
            '\u{e406}' => Some(End::Align(Align::Right)),
            '\u{e407}' => Some(End::Align(Align::Centre)),
            _ => None,
        };
        if let Some(end) = end {
            return (i, i + 1, width, end);
        }
        if i > start && before.is_some_and(|p| breaks_between(p, c)) {
            brk = Some((i, width));
        }
        let [a, x, k] = parts(glyph, c);
        // `comiss; ja`: unordered does not fit.
        let fits = right > pen + a + x + k + width;
        if !fits && count > 0 {
            return match brk {
                Some((at, w)) => (at, at, w, End::Wrap),
                None => (i + 1, i + 1, width, End::Wrap),
            };
        }
        width += a + x + k;
        count += 1;
        before = Some(c);
        glyph = Some(c);
    }
    (chars.len(), chars.len(), width, End::Text)
}

/// `text`'s segments as the line renderer `0x13f0e0` places them. Each
/// row has columns: with no stops one, the rectangle's left plus the
/// indent to its right; with stops, column 0 runs from there to stop 0,
/// column `n` from stop `n - 1` to stop `n`, the last to the rectangle's
/// right. The indent is the first's while no `\r` has passed, else the
/// second's (`0x13f1d3`). Each segment is justified in its column
/// (`0x13f2d7`): left at its pen, right with its width against the
/// column's right, centred in the room left over.
///
/// What ended a segment moves the next (`0x13f3d2`): `\r` starts a row
/// group under the deepest row of this one, back at column 0; a tab
/// moves one column on, to the group's first row, unless it is past the
/// last stop, when nothing moves; an alignment unit sets the next
/// segment's justification and goes back to the group's first row; a
/// wrap goes one row down. `\n` is skipped (`0x13f5c8`).
pub fn segments(text: &str, frame: &Frame, parts: &dyn Fn(Option<char>, char) -> [f32; 3]) -> Vec<Segment> {
    let chars: Vec<char> = text.chars().filter(|&c| c != '\n').collect();
    let s = frame.scale;
    let (mut tab, mut group, mut deepest, mut row) = (0usize, 0i32, 0i32, 0i32);
    let mut align = frame.align;
    let mut pos = 0;
    let mut out = Vec::new();
    loop {
        let indent = frame.indents[usize::from(group != 0)];
        let first = f32::from(indent) * s + frame.left;
        let (x0, right) = if frame.stops.is_empty() {
            (first, frame.right)
        } else {
            let x0 = if tab == 0 { first } else { f32::from(frame.stops[tab - 1]) * s };
            let right = frame.stops.get(tab).map_or(frame.right, |&stop| f32::from(stop) * s);
            (x0, right)
        };
        let (to, next, width, end) = measure(&chars, pos, x0, right, parts);
        let x = match align {
            Align::Left => x0,
            Align::Right => (right - x0) + x0 - width,
            Align::Centre => (right - x0 - width) * 0.5 + x0,
        };
        out.push(Segment { text: chars[pos..to].iter().collect(), x, row: group + row });
        pos = next;
        match end {
            End::Text => break,
            End::Line => {
                group += deepest + 1;
                (tab, deepest, row) = (0, 0, 0);
            }
            End::Tab => {
                if tab < frame.stops.len() {
                    tab += 1;
                    row = 0;
                }
            }
            End::Align(a) => {
                align = a;
                row = 0;
            }
            End::Wrap => {
                row += 1;
                deepest = deepest.max(row);
            }
        }
    }
    out
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

    fn tens(_: Option<char>, _: char) -> [f32; 3] {
        [10.0, 0.0, 0.0]
    }

    fn frame(stops: &[i16]) -> Frame<'_> {
        Frame { left: 100.0, right: 300.0, stops, indents: [0; 2], scale: 1.0, align: Align::Left }
    }

    fn placed(s: &[Segment]) -> Vec<(&str, f32, i32)> {
        s.iter().map(|s| (s.text.as_str(), s.x, s.row)).collect()
    }

    #[test]
    fn tabs_start_columns_at_the_stops_which_are_not_the_rectangles() {
        let s = segments("ab\tcd\tef\tgh", &frame(&[150, 250]), &tens);
        // Past the last stop a tab moves nothing: "gh" draws over "ef".
        assert_eq!(placed(&s), [("ab", 100.0, 0), ("cd", 150.0, 0), ("ef", 250.0, 0), ("gh", 250.0, 0)]);
        // Right-justified in its column: against the next stop.
        let right = Frame { align: Align::Right, ..frame(&[150, 250]) };
        assert_eq!(segments("ab\tcd", &right, &tens)[0].x, 130.0);
        assert_eq!(segments("ab\tcd", &right, &tens)[1].x, 230.0);
    }

    #[test]
    fn a_row_group_ends_under_its_deepest_column() {
        // Column 0 is 50 wide: four glyphs fit and the fifth, with no
        // break, joins them; the tab goes back to row 0 and "\r" starts
        // the next group under row 1.
        let s = segments("aaaaaa\tb\rc", &frame(&[150]), &tens);
        assert_eq!(placed(&s), [("aaaaa", 100.0, 0), ("a", 100.0, 1), ("b", 150.0, 0), ("c", 100.0, 2)]);
    }

    #[test]
    fn an_alignment_unit_justifies_what_follows_on_the_groups_first_row() {
        let s = segments("ab\u{e406}cd\u{e407}e", &frame(&[]), &tens);
        assert_eq!(placed(&s), [("ab", 100.0, 0), ("cd", 280.0, 0), ("e", 195.0, 0)]);
    }

    #[test]
    fn the_scale_multiplies_the_stops_and_indents_and_the_first_indent_holds_for_the_first_group() {
        let half = |_: Option<char>, _: char| [5.0, 0.0, 0.0];
        let f = Frame { indents: [8, 2], scale: 0.5, ..frame(&[400]) };
        let s = segments("a\tb\rc", &f, &half);
        assert_eq!(placed(&s), [("a", 104.0, 0), ("b", 200.0, 0), ("c", 101.0, 1)]);
        // A wrapped row of the first group keeps the first indent.
        let narrow = Frame { right: 135.0, indents: [8, 2], scale: 1.0, ..frame(&[]) };
        assert_eq!(placed(&segments("a a", &narrow, &tens)), [("a ", 108.0, 0), ("a", 108.0, 1)]);
    }
}
