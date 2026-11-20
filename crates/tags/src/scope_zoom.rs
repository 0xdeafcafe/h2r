//! Per-weapon inputs for the Halo 1 zoom effect, in the unused prefix of
//! the converted egor. The renderer only reads this on HUD screen draws.

pub const MAGIC: u32 = u32::from_le_bytes(*b"H1Z1");
pub const WORDS: usize = 13;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Blur {
    pub zoom_only: bool,
    pub fov: [f32; 2],
    pub radius: [f32; 2],
}

impl Blur {
    pub fn enabled(self) -> bool {
        self.radius
            .iter()
            .any(|&radius| radius > 0.0 && radius.is_finite())
    }

    /// Halo 1 0x000d6bef: clamp the fraction between the authored angles.
    /// The second radius applies when the two angles are equal.
    pub fn at(self, fov: f32, zoomed: bool) -> f32 {
        if (self.zoom_only && !zoomed) || !fov.is_finite() {
            return 0.0;
        }
        let [first, last] = self.fov;
        let [near, far] = self.radius;
        let radius = if first == last {
            far
        } else {
            let t = ((fov - first) / (last - first)).clamp(0.0, 1.0);
            near * (1.0 - t) + far * t
        };
        if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Filter {
    pub flags: u16,
    pub channel: u16,
    pub strength: f32,
}

impl Filter {
    pub fn zoom_only(self) -> bool {
        self.flags & 1 != 0
    }

    pub fn uses_night_vision(self) -> bool {
        self.flags & 2 != 0
    }

    pub fn enabled(self) -> bool {
        self.channel < 4 && self.strength.is_finite() && self.strength > 0.0
    }

    pub fn amount(self, zoomed: bool, night_vision: f32) -> f32 {
        if !self.enabled() || (self.zoom_only() && !zoomed) {
            return 0.0;
        }
        // All four script channels start at 1. Retail maps never call the
        // channel setter. Out-of-range channels return 0 (0x000618e0).
        let amount = if self.uses_night_vision() {
            self.strength * night_vision.clamp(0.0, 1.0)
        } else {
            self.strength
        };
        amount.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Profile {
    pub blur: Blur,
    pub desaturation: Filter,
    pub tint: Filter,
    pub tint_rgb: [f32; 3],
}

impl Profile {
    pub fn enabled(self) -> bool {
        self.blur.enabled() || self.desaturation.enabled() || self.tint.enabled()
    }

    pub fn without_zoom_only(mut self) -> Self {
        if self.blur.zoom_only {
            self.blur = Blur::default();
        }
        if self.desaturation.zoom_only() {
            self.desaturation = Filter::default();
        }
        if self.tint.zoom_only() {
            self.tint = Filter::default();
        }
        self
    }

    pub fn words(self) -> [u32; WORDS] {
        let filter = |f: Filter| u32::from(f.flags) | (u32::from(f.channel) << 16);
        [
            MAGIC,
            self.blur.zoom_only as u32,
            self.blur.fov[0].to_bits(),
            self.blur.fov[1].to_bits(),
            self.blur.radius[0].to_bits(),
            self.blur.radius[1].to_bits(),
            filter(self.desaturation),
            self.desaturation.strength.to_bits(),
            filter(self.tint),
            self.tint.strength.to_bits(),
            self.tint_rgb[0].to_bits(),
            self.tint_rgb[1].to_bits(),
            self.tint_rgb[2].to_bits(),
        ]
    }

    pub fn decode(words: [u32; WORDS]) -> Option<Self> {
        let float = |i: usize| f32::from_bits(words[i]);
        let filter = |i: usize| Filter {
            flags: words[i] as u16,
            channel: (words[i] >> 16) as u16,
            strength: float(i + 1),
        };
        (words[0] == MAGIC).then(|| Self {
            blur: Blur {
                zoom_only: words[1] != 0,
                fov: [float(2), float(3)],
                radius: [float(4), float(5)],
            },
            desaturation: filter(6),
            tint: filter(8),
            tint_rgb: [float(10), float(11), float(12)],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_blur_clamps_and_interpolates_the_authored_angles() {
        let blur = Blur {
            zoom_only: true,
            fov: [1.0, 0.5],
            radius: [0.0, 12.0],
        };
        assert_eq!(blur.at(0.5, false), 0.0);
        assert_eq!(blur.at(1.5, true), 0.0);
        assert_eq!(blur.at(0.75, true), 6.0);
        assert_eq!(blur.at(0.25, true), 12.0);
        assert_eq!(
            Blur {
                fov: [1.0; 2],
                ..blur
            }
            .at(1.5, true),
            12.0
        );
    }

    #[test]
    fn zoom_colour_keeps_the_two_gates_and_script_channel() {
        let filter = Filter {
            flags: 3,
            channel: 1,
            strength: 0.75,
        };
        let profile = Profile {
            desaturation: filter,
            tint: Filter {
                flags: 0,
                strength: 0.25,
                ..filter
            },
            tint_rgb: [0.25, 1.0, 0.5],
            ..Profile::default()
        };
        assert_eq!(Profile::decode(profile.words()), Some(profile));
        assert_eq!(Profile::decode([0; WORDS]), None);
        assert_eq!(profile.desaturation.amount(true, 0.5), 0.375);
        assert_eq!(profile.desaturation.amount(false, 1.0), 0.0);
        assert_eq!(profile.tint.amount(false, 0.0), 0.25);
        assert!(!profile.without_zoom_only().desaturation.enabled());
        assert!(profile.without_zoom_only().tint.enabled());
        assert_eq!(
            Filter {
                channel: 4,
                ..filter
            }
            .amount(true, 1.0),
            0.0
        );
    }
}
