//! The frame's tick-visible writes (docs/graphics/framescene.md, 5.2): the
//! three words the guest's frame leaves once per frame that the next tick
//! reads, made in Rust.
//!
//! | Word | Guest writer | Tick reader |
//! |---|---|---|
//! | render frame counter `0x004ba034`, +1 | `render_views` `0x0002b5d0` (frame begin) | particle update `0x00248620`/`0x00248750` |
//! | one-shot byte `0x004e6098`, cleared | `0x00223080` | the geometry cache, which sets it |
//! | present counter `0x00485aa0`, 64-bit +1 | `rasterizer_present` `0x00013f10` (`0x00014196..0x000141ae`) | the tick decal add `0x0017ef10`, into the decal's `+0x28` |
//!
//! [`frame`] makes them in that order. Both frame owners use it: the render
//! frame step (`render_bridge::frame_step`) and the headless step
//! (`sim_bridge::sim_frame`), so a host that draws nothing still leaves the
//! tick what the guest's frame would have. [`Words`] reads them so a frame
//! the guest renders can be checked against [`Words::predict`].

use crate::memory::{GuestMemory, MemFault};

/// The guest words the frame's writes go to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// The 64-bit present counter `rasterizer_present` (`0x00013f10`) adds to.
    pub present_counter: u32,
    /// The render frame counter `render_views` adds to.
    pub frame_counter: u32,
    /// The one-shot byte `0x00223080` clears.
    pub one_shot: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        present_counter: 0x0048_5aa0,
        frame_counter: 0x004b_a034,
        one_shot: 0x004e_6098,
    };
}

/// The render frame counter, one frame on.
pub fn frame_counter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let f = g.u32(a.frame_counter)?;
    g.write(a.frame_counter, &f.wrapping_add(1).to_le_bytes())
}

/// The one-shot byte, cleared.
pub fn one_shot(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    g.write(a.one_shot, &[0])
}

/// The present counter, one present on, carrying into the high word
/// (`add ecx, 1` / `adc eax, 0`).
pub fn present_counter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let lo = g.u32(a.present_counter)?;
    let hi = g.u32(a.present_counter.wrapping_add(4))?;
    let v = (u64::from(hi) << 32 | u64::from(lo)).wrapping_add(1);
    g.write(a.present_counter, &v.to_le_bytes())
}

/// The three writes, in the guest frame's order. Reads every word first,
/// so a fault writes nothing.
pub fn frame(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let next = Words::read(a, g)?.predict();
    next.write(a, g)
}

/// The three words as the guest holds them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Words {
    pub frame_counter: u32,
    pub one_shot: u8,
    pub present_counter: u64,
}

/// A word whose guest value is not what [`Words::predict`] made, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Word {
    FrameCounter,
    OneShot,
    PresentCounter,
}

impl Word {
    pub const ALL: [Word; 3] = [Word::FrameCounter, Word::OneShot, Word::PresentCounter];

    pub fn name(self) -> &'static str {
        match self {
            Word::FrameCounter => "frame_counter",
            Word::OneShot => "one_shot",
            Word::PresentCounter => "present_counter",
        }
    }
}

impl Words {
    pub fn read(a: &Addrs, g: &dyn GuestMemory) -> Result<Words, MemFault> {
        let lo = g.u32(a.present_counter)?;
        let hi = g.u32(a.present_counter.wrapping_add(4))?;
        Ok(Words {
            frame_counter: g.u32(a.frame_counter)?,
            one_shot: g.u8(a.one_shot)?,
            present_counter: u64::from(hi) << 32 | u64::from(lo),
        })
    }

    /// What one guest frame that views the world and presents leaves.
    pub fn predict(self) -> Words {
        Words {
            frame_counter: self.frame_counter.wrapping_add(1),
            one_shot: 0,
            present_counter: self.present_counter.wrapping_add(1),
        }
    }

    fn write(self, a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
        g.write(a.frame_counter, &self.frame_counter.to_le_bytes())?;
        g.write(a.one_shot, &[self.one_shot])?;
        g.write(a.present_counter, &self.present_counter.to_le_bytes())
    }

    /// The words that differ between `self` and `other`, in [`Word::ALL`]
    /// order.
    pub fn diff(self, other: Words) -> Vec<Word> {
        Word::ALL
            .into_iter()
            .filter(|w| match w {
                Word::FrameCounter => self.frame_counter != other.frame_counter,
                Word::OneShot => self.one_shot != other.one_shot,
                Word::PresentCounter => self.present_counter != other.present_counter,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    const A: Addrs = Addrs {
        present_counter: 0x2030,
        frame_counter: 0x2038,
        one_shot: 0x203c,
    };

    #[test]
    fn the_frame_makes_the_three_words_as_the_guest_leaves_them() {
        let mut g = FakeGuest::new();
        g.map(0x2000, 0x100);
        g.put_u32(A.present_counter, 0xffff_ffff)
            .put_u32(A.present_counter + 4, 7);
        g.put_u32(A.frame_counter, 41).put_u8(A.one_shot, 1);
        let before = Words::read(&A, &g).unwrap();
        frame(&A, &mut g).unwrap();
        let after = Words::read(&A, &g).unwrap();
        assert_eq!(after, before.predict());
        assert_eq!(
            after,
            Words {
                frame_counter: 42,
                one_shot: 0,
                present_counter: 8 << 32
            },
            "the add carries"
        );
        assert!(after.diff(before.predict()).is_empty());
        let mut off = after;
        off.frame_counter += 1;
        off.present_counter += 2;
        assert_eq!(after.diff(off), [Word::FrameCounter, Word::PresentCounter]);
    }

    #[test]
    fn a_fault_writes_nothing() {
        let a = Addrs {
            one_shot: 0x9000,
            ..A
        };
        let mut g = FakeGuest::new();
        g.map(0x2000, 0x100);
        g.put_u32(a.frame_counter, 5);
        assert!(frame(&a, &mut g).is_err(), "the one-shot byte is unmapped");
        assert_eq!(g.u32(A.frame_counter), Ok(5));
    }
}
