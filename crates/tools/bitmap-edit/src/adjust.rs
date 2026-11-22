//! Whole-image adjustments: levels, hue/saturation/value, invert.

use serde::{Deserialize, Serialize};

use crate::canvas::Canvas;
use crate::color::Channels;
use crate::error::Error;
use crate::rect::Rect;

/// One adjustment, applied to a rectangle and some channels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Adjust {
    /// `255 - v` on the enabled channels.
    Invert,
    /// Black point, white point and gamma, on the enabled channels'
    /// encoded values: `((v - black) / (white - black)) ^ (1 / gamma)`.
    Levels {
        #[serde(default)]
        black: f32,
        #[serde(default = "one")]
        white: f32,
        #[serde(default = "one")]
        gamma: f32,
    },
    /// Hue turned by `hue` degrees; saturation and value multiplied.
    /// Needs colour channels; alpha is left as it is.
    Hsv {
        #[serde(default)]
        hue: f32,
        #[serde(default = "one")]
        saturation: f32,
        #[serde(default = "one")]
        value: f32,
    },
}

fn one() -> f32 {
    1.0
}

impl Adjust {
    pub fn validate(&self) -> Result<(), Error> {
        let finite = |what: &str, v: f32| {
            if v.is_finite() {
                Ok(())
            } else {
                Err(Error::Params(format!("{what} is not a number")))
            }
        };
        match *self {
            Adjust::Invert => Ok(()),
            Adjust::Levels {
                black,
                white,
                gamma,
            } => {
                finite("black", black)?;
                finite("white", white)?;
                finite("gamma", gamma)?;
                if !(0.0..1.0).contains(&black) || white <= black || white > 1.0 {
                    return Err(Error::Params(
                        "levels need 0 <= black < white <= 1".to_owned(),
                    ));
                }
                if gamma <= 0.0 || gamma > 10.0 {
                    return Err(Error::Params(format!("gamma {gamma} is not in (0, 10]")));
                }
                Ok(())
            }
            Adjust::Hsv {
                hue,
                saturation,
                value,
            } => {
                finite("hue", hue)?;
                finite("saturation", saturation)?;
                finite("value", value)?;
                if saturation < 0.0 || value < 0.0 {
                    return Err(Error::Params(
                        "saturation and value are multipliers of 0 or more".to_owned(),
                    ));
                }
                Ok(())
            }
        }
    }
}

/// Applies `adjust` to `r` (clipped) on `channels`. Returns the clipped
/// rectangle.
pub fn apply(
    canvas: &mut Canvas,
    adjust: &Adjust,
    r: Rect,
    channels: Channels,
) -> Result<Rect, Error> {
    adjust.validate()?;
    let r = r.intersect(canvas.bounds());
    if r.is_empty() {
        return Ok(r);
    }
    let (_, mut data) = canvas.read_rect(r);
    match *adjust {
        Adjust::Invert => {
            for px in data.chunks_exact_mut(4) {
                for (c, v) in px.iter_mut().enumerate() {
                    if channels.get(c) {
                        *v = 255 - *v;
                    }
                }
            }
        }
        Adjust::Levels {
            black,
            white,
            gamma,
        } => {
            let mut lut = [0u8; 256];
            for (i, o) in lut.iter_mut().enumerate() {
                let t = ((i as f32 / 255.0 - black) / (white - black)).clamp(0.0, 1.0);
                *o = (t.powf(1.0 / gamma) * 255.0 + 0.5) as u8;
            }
            for px in data.chunks_exact_mut(4) {
                for (c, v) in px.iter_mut().enumerate() {
                    if channels.get(c) {
                        *v = lut[*v as usize];
                    }
                }
            }
        }
        Adjust::Hsv {
            hue,
            saturation,
            value,
        } => {
            if channels.r || channels.g || channels.b {
                for px in data.chunks_exact_mut(4) {
                    let (h, s, v) = rgb_to_hsv(px[0], px[1], px[2]);
                    let h = (h + hue).rem_euclid(360.0);
                    let out = hsv_to_rgb(h, (s * saturation).clamp(0.0, 1.0), (v * value).clamp(0.0, 1.0));
                    for (c, o) in out.into_iter().enumerate() {
                        if channels.get(c) {
                            px[c] = o;
                        }
                    }
                }
            }
        }
    }
    canvas.write_rect(r, &data)?;
    Ok(r)
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |f: f32| ((f + m) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    [byte(r), byte(g), byte(b)]
}
