//! Stroke digits for the number on a plate (a hill's, a territory's, a
//! site's): seven segments a digit, drawn as lines, so a number needs no
//! font and no texture (docs/game/gametype-marker-forms.md, "The parts
//! of a marker").
//!
//! A digit's cell is [`WIDTH`] wide and one tall, its origin the bottom
//! left; digits stand [`GAP`] apart. All in the units of the height a
//! caller gives.

/// A digit's width against its height.
pub const WIDTH: f32 = 0.5;
/// The space between two digits against their height.
pub const GAP: f32 = 0.2;

/// The seven segments as (from, to) in a digit's cell: top, top right,
/// bottom right, bottom, bottom left, top left, middle.
const SEGMENTS: [[[f32; 2]; 2]; 7] = [
    [[0.0, 1.0], [WIDTH, 1.0]],
    [[WIDTH, 1.0], [WIDTH, 0.5]],
    [[WIDTH, 0.5], [WIDTH, 0.0]],
    [[0.0, 0.0], [WIDTH, 0.0]],
    [[0.0, 0.5], [0.0, 0.0]],
    [[0.0, 1.0], [0.0, 0.5]],
    [[0.0, 0.5], [WIDTH, 0.5]],
];

/// The segments each digit lights, as bits of [`SEGMENTS`]' order.
const LIT: [u8; 10] = [
    0b011_1111, // 0
    0b000_0110, // 1
    0b101_1011, // 2
    0b100_1111, // 3
    0b110_0110, // 4
    0b110_1101, // 5
    0b111_1101, // 6
    0b000_0111, // 7
    0b111_1111, // 8
    0b110_1111, // 9
];

/// The digits of `n`, most significant first.
pub fn digits_of(n: u32) -> Vec<u8> {
    n.to_string().bytes().map(|b| b - b'0').collect()
}

/// The lines of the one digit `d` (0 to 9) in its cell of height 1.
pub fn digit(d: u8) -> Vec<[[f32; 2]; 2]> {
    let lit = LIT[usize::from(d % 10)];
    SEGMENTS
        .iter()
        .enumerate()
        .filter(|(i, _)| lit & (1 << i) != 0)
        .map(|(_, s)| *s)
        .collect()
}

/// How wide the number `n` is at height 1.
pub fn width(n: u32) -> f32 {
    let k = digits_of(n).len() as f32;
    k * WIDTH + (k - 1.0) * GAP
}

/// The lines of the number `n`, `height` tall, its bottom left at
/// `origin`.
pub fn number(n: u32, origin: [f32; 2], height: f32) -> Vec<[[f32; 2]; 2]> {
    let mut out = Vec::new();
    for (i, d) in digits_of(n).into_iter().enumerate() {
        let x = origin[0] + i as f32 * (WIDTH + GAP) * height;
        out.extend(
            digit(d)
                .into_iter()
                .map(|seg| seg.map(|[u, v]| [x + u * height, origin[1] + v * height])),
        );
    }
    out
}
