//! Halo 1's automatic-migration coordinates and route records. The string
//! starts with one directory entry per encounter, then the referenced rows.

use super::{decode_words, encode_words};

pub const GROUPS: usize = super::FOLLOW_WORDS as usize / 3;
pub const HEADER: usize = 12;
pub const DIRECTORY: usize = GROUPS * HEADER * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Location {
    pub offset: u32,
    pub rows: u16,
}

impl Location {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let [hi, lo, rows] = decode_words(bytes)?.try_into().ok()?;
        Some(Self {
            offset: join(hi, lo),
            rows,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub squad: u16,
    pub orders: [u16; 2],
    pub posture_squad: u16,
    pub defending: bool,
    pub masks: [u32; 2],
}

impl Route {
    pub const WIDTH: usize = 36;

    pub fn append(self, out: &mut String) {
        let [attack, defend] = self.masks;
        encode_words(
            out,
            [
                self.squad,
                self.orders[0],
                self.orders[1],
                self.posture_squad,
                u16::from(self.defending),
                (attack >> 16) as u16,
                attack as u16,
                (defend >> 16) as u16,
                defend as u16,
            ],
        );
    }

    pub fn decode(words: &[u16]) -> Option<Self> {
        let &[
            squad,
            attack,
            defend,
            posture_squad,
            defending,
            ah,
            al,
            dh,
            dl,
        ] = words
        else {
            return None;
        };
        Some(Self {
            squad,
            orders: [attack, defend],
            posture_squad,
            defending: defending != 0,
            masks: [join(ah, al), join(dh, dl)],
        })
    }
}

pub const POINT_WIDTH: usize = 28;

/// A route's group is the bit index in its source firing-position mask.
pub fn point(out: &mut String, group: u16, position: [f32; 3]) {
    encode_words(
        out,
        std::iter::once(group).chain(position.into_iter().flat_map(|coordinate| {
            let bits = coordinate.to_bits();
            [(bits >> 16) as u16, bits as u16]
        })),
    );
}

pub fn read_point(words: &[u16]) -> Option<(usize, [f32; 3])> {
    let &[group, xh, xl, yh, yl, zh, zl] = words else {
        return None;
    };
    let group = usize::from(group);
    (group < 26).then_some((
        group,
        [join(xh, xl), join(yh, yl), join(zh, zl)].map(f32::from_bits),
    ))
}

fn join(high: u16, low: u16) -> u32 {
    (u32::from(high) << 16) | u32::from(low)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_and_position_keep_authored_group_bits_and_coordinates() {
        let route = Route {
            squad: 348,
            orders: [412, 416],
            posture_squad: u16::MAX,
            defending: true,
            masks: [0x2004, 1 << 25],
        };
        let mut text = String::new();
        route.append(&mut text);
        assert_eq!(text.len(), Route::WIDTH);
        assert_eq!(
            Route::decode(&decode_words(text.as_bytes()).unwrap()),
            Some(route)
        );
        text.clear();
        let xyz = [0.25, -2.0, 18.375];
        point(&mut text, 25, xyz);
        assert_eq!(text.len(), POINT_WIDTH);
        assert_eq!(
            read_point(&decode_words(text.as_bytes()).unwrap()),
            Some((25, xyz))
        );
    }
}
