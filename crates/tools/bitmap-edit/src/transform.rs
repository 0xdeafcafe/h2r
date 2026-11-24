//! Channel copy and the turns: edits that move values between the channels
//! of a pixel, or between the places of a rectangle, without resampling.

use serde::{Deserialize, Serialize};

use crate::canvas::Canvas;
use crate::color::Channels;
use crate::error::Error;
use crate::rect::Rect;

/// What a channel copy reads: one of a pixel's channels, or the luma of
/// its colour (Rec. 601 of the encoded values, as the map's encoder writes
/// `L8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    R,
    G,
    B,
    A,
    Luma,
}

impl Channel {
    /// Every source, in the order a picker lists them.
    pub const ALL: [Channel; 5] = [
        Channel::R,
        Channel::G,
        Channel::B,
        Channel::A,
        Channel::Luma,
    ];

    /// What this reads of a pixel.
    pub fn of(self, px: [u8; 4]) -> u8 {
        match self {
            Channel::R => px[0],
            Channel::G => px[1],
            Channel::B => px[2],
            Channel::A => px[3],
            Channel::Luma => {
                ((77 * u32::from(px[0]) + 150 * u32::from(px[1]) + 29 * u32::from(px[2]) + 128)
                    >> 8) as u8
            }
        }
    }

    /// Its name, as [`Channel::parse`] reads it and the op log writes it.
    pub fn name(self) -> &'static str {
        match self {
            Channel::R => "r",
            Channel::G => "g",
            Channel::B => "b",
            Channel::A => "a",
            Channel::Luma => "luma",
        }
    }

    pub fn parse(s: &str) -> Option<Channel> {
        Channel::ALL.into_iter().find(|c| c.name() == s)
    }
}

/// A flip or a turn of a rectangle's pixels by a multiple of a quarter
/// turn. The names are those [`Turn::parse`] reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Turn {
    /// Left and right swap.
    #[default]
    FlipHorizontal,
    /// Top and bottom swap.
    FlipVertical,
    /// A quarter turn clockwise: the top row becomes the right column.
    Quarter,
    /// Half a turn.
    Half,
    /// Three quarters of a turn clockwise, a quarter anticlockwise: the
    /// top row becomes the left column.
    ThreeQuarters,
}

impl Turn {
    /// Every turn, in the order a picker lists them.
    pub const ALL: [Turn; 5] = [
        Turn::FlipHorizontal,
        Turn::FlipVertical,
        Turn::Quarter,
        Turn::Half,
        Turn::ThreeQuarters,
    ];

    /// A quarter turn moves a column onto a row, so it needs a square; the
    /// flips and the half turn take any rectangle.
    pub fn needs_square(self) -> bool {
        matches!(self, Turn::Quarter | Turn::ThreeQuarters)
    }

    /// Its name, as [`Turn::parse`] reads it and the op log writes it.
    pub fn name(self) -> &'static str {
        match self {
            Turn::FlipHorizontal => "flip_horizontal",
            Turn::FlipVertical => "flip_vertical",
            Turn::Quarter => "quarter",
            Turn::Half => "half",
            Turn::ThreeQuarters => "three_quarters",
        }
    }

    pub fn parse(s: &str) -> Option<Turn> {
        Turn::ALL.into_iter().find(|t| t.name() == s)
    }
}

/// Copies what `from` reads of every pixel of `r` (clipped to the canvas)
/// into the channels `to`; the other channels stay. Returns the clipped
/// rectangle, or an empty one when no pixel changes (nothing is written
/// then, so no undo step and no tile stands out as edited).
pub fn copy_channel(
    canvas: &mut Canvas,
    from: Channel,
    to: Channels,
    r: Rect,
) -> Result<Rect, Error> {
    if !to.any() {
        return Err(Error::Params("no channel is written".to_owned()));
    }
    let r = r.intersect(canvas.bounds());
    if r.is_empty() {
        return Ok(r);
    }
    let (_, before) = canvas.read_rect(r);
    let mut data = before.clone();
    for px in data.chunks_exact_mut(4) {
        let v = from.of([px[0], px[1], px[2], px[3]]);
        for (c, o) in px.iter_mut().enumerate() {
            if to.get(c) {
                *o = v;
            }
        }
    }
    if data == before {
        return Ok(Rect::default());
    }
    canvas.write_rect(r, &data)?;
    Ok(r)
}

/// Turns the pixels of `r` (clipped to the canvas) by `t`; the rest of the
/// canvas stays. A quarter turn needs a square `r`. Returns the clipped
/// rectangle, or an empty one when no pixel moves (as on a uniform layer).
pub fn turn(canvas: &mut Canvas, t: Turn, r: Rect) -> Result<Rect, Error> {
    let r = r.intersect(canvas.bounds());
    if r.is_empty() {
        return Ok(r);
    }
    if t.needs_square() && r.w != r.h {
        return Err(Error::Rect(format!(
            "a quarter turn needs a square, not {}x{} at {},{}",
            r.w, r.h, r.x, r.y
        )));
    }
    let (_, src) = canvas.read_rect(r);
    let (w, h) = (r.w as usize, r.h as usize);
    let at = |x: usize, y: usize| (y * w + x) * 4;
    let mut out = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            // Where the pixel at (x, y) lands.
            let (dx, dy) = match t {
                Turn::FlipHorizontal => (w - 1 - x, y),
                Turn::FlipVertical => (x, h - 1 - y),
                Turn::Quarter => (h - 1 - y, x),
                Turn::Half => (w - 1 - x, h - 1 - y),
                Turn::ThreeQuarters => (y, w - 1 - x),
            };
            let (from, to) = (at(x, y), at(dx, dy));
            out[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    if out == src {
        return Ok(Rect::default());
    }
    canvas.write_rect(r, &out)?;
    Ok(r)
}
