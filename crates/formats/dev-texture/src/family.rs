//! The six colour families and their numbers.

/// What a face is for, by colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Floor,
    Wall,
    Ceiling,
    Trim,
    /// Faces left out of the render model.
    Nodraw,
    /// Barriers.
    Clip,
}

pub const ALL: [Family; 6] = [
    Family::Floor,
    Family::Wall,
    Family::Ceiling,
    Family::Trim,
    Family::Nodraw,
    Family::Clip,
];

/// Least squared distance between the mid colours of any two families.
pub const MIN_FAMILY_CONTRAST: u32 = 6000;
/// Most squared distance between one family's two tones.
pub const MAX_TONE_SPREAD: u32 = 1600;

impl Family {
    /// The name of the colour, as the design page gives it.
    pub fn colour_name(self) -> &'static str {
        match self {
            Family::Floor => "grey",
            Family::Wall => "orange",
            Family::Ceiling => "blue",
            Family::Trim => "green",
            Family::Nodraw => "yellow",
            Family::Clip => "magenta",
        }
    }

    /// The (light, dark) tones of the checker, as RGB.
    pub fn tones(self) -> ([u8; 3], [u8; 3]) {
        match self {
            Family::Floor => ([150, 150, 150], [128, 128, 128]),
            Family::Wall => ([232, 140, 52], [208, 120, 40]),
            Family::Ceiling => ([70, 120, 215], [56, 100, 190]),
            Family::Trim => ([80, 190, 90], [66, 168, 74]),
            Family::Nodraw => ([240, 225, 70], [218, 203, 56]),
            Family::Clip => ([215, 70, 200], [190, 56, 178]),
        }
    }

    /// The average of the two tones: the family's colour at a glance.
    pub fn mid(self) -> [u8; 3] {
        let (a, b) = self.tones();
        [
            ((a[0] as u16 + b[0] as u16) / 2) as u8,
            ((a[1] as u16 + b[1] as u16) / 2) as u8,
            ((a[2] as u16 + b[2] as u16) / 2) as u8,
        ]
    }
}

/// Squared distance between two RGB colours.
pub fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    let d = |i: usize| (a[i] as i32 - b[i] as i32).pow(2) as u32;
    d(0) + d(1) + d(2)
}
