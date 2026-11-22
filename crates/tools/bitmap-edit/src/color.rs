//! Colour: sRGB bytes, linear light, channel masks and the blend the
//! brush uses.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

fn table() -> &'static [f32; 256] {
    static T: OnceLock<[f32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let c = i as f32 / 255.0;
            *v = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        t
    })
}

/// An sRGB-encoded byte in linear light, 0 to 1.
pub fn to_linear(v: u8) -> f32 {
    table()[v as usize]
}

/// Linear light (clamped to 0 to 1) as an sRGB-encoded byte.
pub fn to_srgb(l: f32) -> u8 {
    let l = if l.is_nan() { 0.0 } else { l.clamp(0.0, 1.0) };
    let c = if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0 + 0.5) as u8
}

/// Which of the four channels a tool writes: alpha is often a mask, so
/// channels paint apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channels {
    pub r: bool,
    pub g: bool,
    pub b: bool,
    pub a: bool,
}

impl Channels {
    pub const ALL: Channels = Channels {
        r: true,
        g: true,
        b: true,
        a: true,
    };

    pub const fn get(&self, i: usize) -> bool {
        match i {
            0 => self.r,
            1 => self.g,
            2 => self.b,
            _ => self.a,
        }
    }

    pub fn is_all(&self) -> bool {
        *self == Channels::ALL
    }

    pub fn any(&self) -> bool {
        self.r || self.g || self.b || self.a
    }

    /// `"rgba"`, `"a"`, `"rgb"`: the letters of the channels written
    /// (upper or lower case, any order). `None` for another letter or for
    /// none at all.
    pub fn parse(s: &str) -> Option<Channels> {
        let mut c = Channels {
            r: false,
            g: false,
            b: false,
            a: false,
        };
        for ch in s.chars() {
            match ch.to_ascii_lowercase() {
                'r' => c.r = true,
                'g' => c.g = true,
                'b' => c.b = true,
                'a' => c.a = true,
                _ => return None,
            }
        }
        c.any().then_some(c)
    }

    /// The letters, as [`Channels::parse`] reads them.
    pub fn name(&self) -> String {
        let mut s = String::new();
        for (i, l) in ['r', 'g', 'b', 'a'].into_iter().enumerate() {
            if self.get(i) {
                s.push(l);
            }
        }
        s
    }
}

impl Default for Channels {
    fn default() -> Self {
        Channels::ALL
    }
}

/// Paints `src` onto `dst` with coverage `a` (0 to 1), in linear light.
///
/// With every channel on, this is "over" on straight alpha (the colour's
/// own alpha scales `a`). With some channels off, each enabled channel is
/// moved towards the colour's value by `a` and the others stay: painting
/// into alpha alone, or colour alone, leaves the rest as it was. `erase`
/// takes alpha away by `a` (when alpha is enabled) and leaves colour.
pub fn blend(dst: [u8; 4], src: [u8; 4], a: f32, erase: bool, ch: Channels) -> [u8; 4] {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.0 || !a.is_finite() {
        return dst;
    }
    let mut out = dst;
    if erase {
        if ch.a {
            out[3] = (f32::from(dst[3]) * (1.0 - a) + 0.5) as u8;
        }
        return out;
    }
    if ch.is_all() {
        let sa = a * f32::from(src[3]) / 255.0;
        let da = f32::from(dst[3]) / 255.0;
        let ra = sa + da * (1.0 - sa);
        if ra <= 0.0 {
            out[3] = 0;
            return out;
        }
        for c in 0..3 {
            let lin = (to_linear(src[c]) * sa + to_linear(dst[c]) * da * (1.0 - sa)) / ra;
            out[c] = to_srgb(lin);
        }
        out[3] = (ra * 255.0 + 0.5) as u8;
        return out;
    }
    for c in 0..3 {
        if ch.get(c) {
            let d = to_linear(dst[c]);
            out[c] = to_srgb(d + (to_linear(src[c]) - d) * a);
        }
    }
    if ch.a {
        let d = f32::from(dst[3]);
        out[3] = (d + (f32::from(src[3]) - d) * a + 0.5) as u8;
    }
    out
}

/// `src` written over `dst` on the enabled channels only.
pub fn replace(dst: [u8; 4], src: [u8; 4], ch: Channels) -> [u8; 4] {
    let mut out = dst;
    for c in 0..4 {
        if ch.get(c) {
            out[c] = src[c];
        }
    }
    out
}
