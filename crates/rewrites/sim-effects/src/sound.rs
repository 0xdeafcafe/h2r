//! A sound part (`snd!`, `0x17acef`), as the part switch builds its call.
//!
//! Two paths, both one leaf call and no random draw:
//!
//! - **On the object.** The effect has an object (`[effect + 0x48]` not
//!   `-1`) and `DL` is 0: `0x189060` with `EAX` = location `+0xc`, `ECX` =
//!   location `+0x30`, `EBX` = the part's tag, `ESI` = the object, `EDX` =
//!   the location's node (`[location + 2]`, `0xffff` for none), and the
//!   object, the node and the scale on the stack (`ret 0xc`).
//! - **At a point.** Otherwise `0x1895f0` (`ret 4`) with `EDX` = a
//!   `0x24`-byte record in the switch's frame (`[esp+0x10]`), the tag on
//!   the stack and the scale in `XMM0`. The record is the position, the
//!   direction packed by `0x188dd0` ([`pack_direction`]), the vector at
//!   `[0x4687a4]` and the effect's `+0x1c` and `+0x20`.


/// The leaves.
pub const ON_OBJECT: u32 = 0x0018_9060;
pub const AT_POINT: u32 = 0x0018_95f0;
/// Where the switch calls them from.
pub const ON_OBJECT_RETURN: u32 = 0x0017_ad29;
pub const AT_POINT_RETURN: u32 = 0x0017_ad9f;
/// The point record's size and its place below the switch's entry `ESP`
/// (the frame `0x110`, then `[esp+0x10]`).
pub const RECORD: usize = 0x24;
pub const RECORD_BELOW_ENTRY: u32 = crate::damage::FRAME - 0x10;
/// The vector the record's `+0x10` copies, through this pointer.
pub const SOURCE: u32 = 0x0046_87a4;

/// `0x188dd0`'s constants: the clamp's low end, its high end (`1.0`), and
/// the scales of z and of x and y.
pub const PACK_LOW: u32 = 0x0045_dbcc;
pub const PACK_HIGH: u32 = 0x0045_dbc0;
pub const PACK_Z: u32 = 0x0045_de28;
pub const PACK_XY: u32 = 0x0045_de2c;

/// The packing's constants and float environment, read from the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pack {
    pub low: u32,
    pub high: u32,
    pub z: u32,
    pub xy: u32,
    /// MXCSR and the x87 control word at the switch's entry.
    pub mxcsr: u32,
    pub control: u16,
}

/// The arithmetic faulted under the guest's MXCSR (an unmasked exception):
/// the guest answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unmasked;

fn ss(op: fn(&mut u32, u128, u32) -> Result<u128, simd::Xm>, mxcsr: u32, a: u32, b: u32) -> Result<u32, Unmasked> {
    let mut m = mxcsr;
    op(&mut m, u128::from(a), b).map(|v| v as u32).map_err(|_| Unmasked)
}

/// `comiss a, b` then `jbe`: taken when `a <= b` or unordered.
fn above(a: u32, b: u32) -> bool {
    let (a, b) = (f32::from_bits(a), f32::from_bits(b));
    a > b
}

/// One component: `0x188dd0`'s clamp to `[low, high]` (a NaN passes), then
/// `(v - low) * scale` in SSE and `fistp` under the x87 control word.
fn component(p: &Pack, v: u32, scale: u32) -> Result<u32, Unmasked> {
    let clamped = if above(p.low, v) {
        p.low
    } else if above(v, p.high) {
        p.high
    } else {
        v
    };
    let d = ss(simd::subss, p.mxcsr, clamped, p.low)?;
    let s = ss(simd::mulss, p.mxcsr, d, scale)?;
    let (i, _) = x87::F80::from_f32_bits(s).to_i32(x87::Mode::from_cw(p.control), false);
    Ok(i as u32)
}

/// `0x188dd0`: a direction as 11, 11 and 10 bits (z, y, x from the top).
pub fn pack_direction(p: &Pack, d: [u32; 3]) -> Result<u32, Unmasked> {
    let z = component(p, d[2], p.z)?;
    let y = component(p, d[1], p.xy)?;
    let x = component(p, d[0], p.xy)?;
    Ok((((z << 11) | y) << 11) | x)
}

/// Everything the arm's call is built from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub tag: u32,
    /// `[effect + 0x48]`.
    pub object: u32,
    /// `DL` at the switch's entry.
    pub dl: u8,
    /// `ECX` at entry and its `+2` word.
    pub location: u32,
    pub node: u16,
    /// The effect datum, its `+0x1c` and `+0x20`, and `EAX` at entry.
    pub effect: u32,
    pub effect_1c: u32,
    pub effect_20: u32,
    pub source_eax: u32,
    pub position: [u32; 3],
    pub direction: [u32; 3],
    pub scale: u32,
    /// The vector at `[0x4687a4]`.
    pub listener: [u32; 3],
    pub pack: Pack,
}

/// The call the arm makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    OnObject { eax: u32, ecx: u32, ebx: u32, esi: u32, edx: u32, args: [u32; 3] },
    AtPoint { record: [u8; RECORD], tag: u32, scale: u32 },
}

impl Call {
    pub fn predict(i: &Inputs) -> Result<Call, Unmasked> {
        if i.object != u32::MAX && i.dl == 0 {
            let node = if i.node == 0xffff { u32::MAX } else { u32::from(i.node & 0x7fff) };
            return Ok(Call::OnObject {
                eax: i.location.wrapping_add(0xc),
                ecx: i.location.wrapping_add(0x30),
                ebx: i.tag,
                esi: i.object,
                edx: node,
                args: [i.object, node, i.scale],
            });
        }
        let packed = pack_direction(&i.pack, i.direction)?;
        let words = [
            i.position[0],
            i.position[1],
            i.position[2],
            packed,
            i.listener[0],
            i.listener[1],
            i.listener[2],
            i.effect_1c,
            i.effect_20,
        ];
        let mut record = [0u8; RECORD];
        for (n, w) in words.iter().enumerate() {
            record[4 * n..4 * n + 4].copy_from_slice(&w.to_le_bytes());
        }
        Ok(Call::AtPoint { record, tag: i.tag, scale: i.scale })
    }

    /// The registers the guest's arm holds at the point call besides `EDX`:
    /// `EAX` = the effect's `+0x20`, `ECX` = the tag, `EBX` = the effect,
    /// `ESI` = its object, `EDI` = the entry `EAX`.
    pub fn point_regs(i: &Inputs) -> [u32; 5] {
        [i.effect_20, i.tag, i.effect, i.object, i.source_eax]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> Pack {
        Pack {
            low: (-1.0f32).to_bits(),
            high: 1.0f32.to_bits(),
            z: 511.0f32.to_bits(),
            xy: 1023.5f32.to_bits(),
            mxcsr: 0x1f80,
            control: 0x027f,
        }
    }

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    /// `0x188dd0` worked by hand: clamp, offset, scale, round to nearest
    /// even, then z, y and x from the top bits.
    #[test]
    fn the_direction_packs_as_retail() {
        let p = pack();
        // (0 + 1) * 1023.5 = 1023.5 rounds to 1024 (even); z (1 + 1) * 511 = 1022.
        assert_eq!(pack_direction(&p, [f(0.0), f(0.0), f(1.0)]), Ok(((1022 << 11 | 1024) << 11) | 1024));
        // Out of range clamps: -3 to -1 (0), 7 to 1 (2047).
        assert_eq!(pack_direction(&p, [f(-3.0), f(7.0), f(-1.0)]), Ok(2047 << 11));
        // A NaN passes the clamp and fistp stores the indefinite.
        let nan = pack_direction(&p, [0x7fc0_0001, f(0.0), f(0.0)]).unwrap();
        assert_eq!(nan & 0x7ff, 0, "x is 0x80000000, the low bits zero");
        assert_eq!(nan, 0x8000_0000 | (1024 << 11) | ((511u32 << 11) << 11));
        // Truncation instead of nearest under a chop control word.
        let chop = Pack { control: 0x0e7f, ..p };
        assert_eq!(pack_direction(&chop, [f(0.0), f(0.0), f(-1.0)]).unwrap() & 0x7ff, 1023);
    }

    fn inputs() -> Inputs {
        Inputs {
            tag: 0xe100_0001,
            object: 0x0004_0007,
            location: 0x0060_0000,
            node: 0x8003,
            effect: 0x0061_0000,
            effect_1c: 0x11,
            effect_20: 0x22,
            source_eax: 0x0062_0000,
            position: [f(1.0), f(2.0), f(3.0)],
            direction: [f(0.0), f(0.0), f(1.0)],
            scale: f(0.5),
            listener: [7, 8, 9],
            pack: pack(),
            ..Inputs::default()
        }
    }

    #[test]
    fn the_two_paths_are_chosen_as_retail() {
        let on = Call::predict(&inputs()).unwrap();
        assert_eq!(
            on,
            Call::OnObject { eax: 0x0060_000c, ecx: 0x0060_0030, ebx: 0xe100_0001, esi: 0x0004_0007, edx: 3, args: [0x0004_0007, 3, f(0.5)] }
        );
        let none = Inputs { node: 0xffff, ..inputs() };
        assert!(matches!(Call::predict(&none).unwrap(), Call::OnObject { edx: u32::MAX, .. }));
        for i in [Inputs { dl: 1, ..inputs() }, Inputs { object: u32::MAX, ..inputs() }] {
            let Call::AtPoint { record, tag, scale } = Call::predict(&i).unwrap() else { panic!("a point call") };
            let w = |n: usize| u32::from_le_bytes(record[4 * n..4 * n + 4].try_into().unwrap());
            assert_eq!([w(0), w(1), w(2)], i.position);
            assert_eq!(w(3), pack_direction(&i.pack, i.direction).unwrap());
            assert_eq!([w(4), w(5), w(6), w(7), w(8)], [7, 8, 9, 0x11, 0x22]);
            assert_eq!((tag, scale), (i.tag, i.scale));
        }
    }
}
