//! A light part (`ligh`, `0x17ac70`): one call to the light creator
//! `0xc0350` (`ret 0x10`), no draw.
//!
//! - **On the object.** The effect has an object (`[effect+0x48]` not -1)
//!   and `DL` is 0: arguments the tag, the object, the location's node
//!   (`0xffff` is -1) and location `+0x24`; `EAX` the tag, `EDX` location
//!   `+0x24`, `EBX` `+0x30`, `ESI` the object, `EDI` `+0xc`.
//! - **At a point.** Otherwise: the tag, -1, -1 and the entry `EAX`;
//!   `EAX` the group, `ECX` the tag, `EBX` the position pointer, `ESI`
//!   `[effect+0x48]`, `EDI` the direction pointer.
//!
//! Both put the scale in `XMM0` (`movss`, the upper lanes zero).


pub const CREATE: u32 = 0x000c_0350;
/// Where the switch calls it from, on the object and at a point.
pub const ON_OBJECT_RETURN: u32 = 0x0017_acb0;
pub const AT_POINT_RETURN: u32 = 0x0017_ace2;

/// Everything the arm reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub group: u32,
    pub tag: u32,
    /// `[effect+0x48]`.
    pub object: u32,
    /// `DL`, `ECX`, `EDX` and `EAX` at entry; the location's node word
    /// (read on the object path only).
    pub dl: u8,
    pub location: u32,
    pub edx: u32,
    pub source: u32,
    pub node: u16,
    pub position_ptr: u32,
    pub direction_ptr: u32,
    pub scale: u32,
}

/// The call, with the registers in the order `EAX`, `ECX`, `EDX`, `EBX`,
/// `ESI`, `EDI`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub on_object: bool,
    pub args: [u32; 4],
    pub regs: [u32; 6],
    pub xmm0: u32,
}

impl Call {
    pub fn predict(i: &Inputs) -> Call {
        if i.object != u32::MAX && i.dl == 0 {
            let node = if i.node == 0xffff { u32::MAX } else { u32::from(i.node & 0x7fff) };
            let l = i.location;
            return Call {
                on_object: true,
                args: [i.tag, i.object, node, l.wrapping_add(0x24)],
                regs: [i.tag, l, l.wrapping_add(0x24), l.wrapping_add(0x30), i.object, l.wrapping_add(0xc)],
                xmm0: i.scale,
            };
        }
        Call {
            on_object: false,
            args: [i.tag, u32::MAX, u32::MAX, i.source],
            regs: [i.group, i.tag, i.edx, i.position_ptr, i.object, i.direction_ptr],
            xmm0: i.scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> Inputs {
        Inputs {
            group: 0x6c69_6768,
            tag: 0xe100_0004,
            object: 0x0004_0007,
            location: 0x0060_0000,
            edx: 0x0000_1100,
            source: 0x0062_0000,
            node: 0x8003,
            position_ptr: 0x0063_0000,
            direction_ptr: 0x0063_0010,
            scale: 0.5f32.to_bits(),
            ..Inputs::default()
        }
    }

    /// The two calls as `0x17ac70..0x17acdd` sets them up.
    #[test]
    fn the_two_paths_are_retails() {
        let on = Call::predict(&inputs());
        assert_eq!(on.args, [0xe100_0004, 0x0004_0007, 3, 0x0060_0024]);
        assert_eq!(on.regs, [0xe100_0004, 0x0060_0000, 0x0060_0024, 0x0060_0030, 0x0004_0007, 0x0060_000c]);
        assert_eq!(Call::predict(&Inputs { node: 0xffff, ..inputs() }).args[2], u32::MAX);
        for i in [Inputs { dl: 1, ..inputs() }, Inputs { object: u32::MAX, ..inputs() }] {
            let p = Call::predict(&i);
            assert!(!p.on_object);
            assert_eq!(p.args, [0xe100_0004, u32::MAX, u32::MAX, 0x0062_0000]);
            assert_eq!(p.regs, [0x6c69_6768, 0xe100_0004, 0x1100, 0x0063_0000, i.object, 0x0063_0010]);
        }
    }
}
