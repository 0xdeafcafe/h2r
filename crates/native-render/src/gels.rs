//! Shader gels: the layer-1 template draw into target 13, before the
//! current light's layers sample engine texture 10. No light-pass policy
//! lives here. Addresses are research points, not copied image data.

use crate::camera::GuestRead;

pub const TARGET: u32 = 13;
pub const LAYER: u32 = 1;
pub const TEMPLATE_RETURN: u32 = 0x0003_a5b0;
pub const QUAD_RETURN: u32 = 0x0001_675a;
pub const BUILDING: u32 = 0x0048_5a77;
pub const DEFINITION: u32 = 0x0048_58c4;
pub const TARGETS: u32 = 0x004b_4b58;
pub const TARGET_STRIDE: u32 = 0x98;

/// Exact call made by `0x0003a550`, after uploading the light constants.
pub fn is_producer(args: [u32; 6], ret: u32, building: u8) -> bool {
    ret == TEMPLATE_RETURN
        && building == 1
        && args[0] != 0
        && args[0] != u32::MAX
        && args[1..] == [LAYER, u32::MAX, 0, TARGET, 1]
}

/// The single-level target this first producer can reproduce. Mip chains
/// need one draw per level and remain on D3D until all levels are planned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub surface: u32,
    pub size: (u32, u32),
}

impl Target {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let at = TARGETS + TARGET_STRIDE * TARGET;
        let surface = mem.read_u32(at + 0x88)?;
        let width = mem.read_u32(at + 0x80)?;
        let height = mem.read_u32(at + 0x84)?;
        let flags = mem.read_u32(at + 0x94)?;
        if surface == 0
            || flags & 0xff00 != 0
            || mem.read_u32(at + 0x78)? != 1
            || width == 0
            || height == 0
        {
            return None;
        }
        Some(Self {
            surface,
            size: (width, height),
        })
    }
}

/// Identity is per invocation, never just the reused target's address or
/// the shader datum. A consumer retains the matching producer's ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub sequence: u64,
    pub definition: u32,
    pub shader: u32,
    pub surface: u32,
}

/// Byte comparison for the target-13 diagnostic. Padding is removed by
/// the readback caller; alpha counts too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Difference {
    pub bytes: usize,
    pub changed: usize,
    pub max: u8,
    pub sum: u64,
}

pub fn compare(native: &[u8], d3d: &[u8]) -> Option<Difference> {
    if native.len() != d3d.len() || native.is_empty() {
        return None;
    }
    let mut d = Difference {
        bytes: native.len(),
        ..Default::default()
    };
    for (&a, &b) in native.iter().zip(d3d) {
        let delta = a.abs_diff(b);
        d.changed += usize::from(delta != 0);
        d.max = d.max.max(delta);
        d.sum += u64::from(delta);
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_thirteen_is_not_shader_layer_thirteen() {
        let args = [0x1234, 1, u32::MAX, 0, 13, 1];
        assert!(is_producer(args, TEMPLATE_RETURN, 1));
        for (slot, value) in [
            (0, 0),
            (0, u32::MAX),
            (1, 13),
            (2, 0),
            (3, 1),
            (4, 1),
            (5, 0),
        ] {
            let mut wrong = args;
            wrong[slot] = value;
            assert!(!is_producer(wrong, TEMPLATE_RETURN, 1));
        }
        assert!(!is_producer(args, TEMPLATE_RETURN + 1, 1));
        assert!(!is_producer(args, TEMPLATE_RETURN, 0));
    }

    #[test]
    fn targets_must_be_readable_visible_and_single_level() {
        use std::collections::HashMap;
        struct Mem(HashMap<u32, u32>);
        impl GuestRead for Mem {
            fn read_u32(&self, at: u32) -> Option<u32> {
                self.0.get(&at).copied()
            }
        }
        let at = TARGETS + TARGET_STRIDE * TARGET;
        let mut mem = Mem([
            (at + 0x78, 1),
            (at + 0x80, 32),
            (at + 0x84, 16),
            (at + 0x88, 0x1234),
            (at + 0x94, 0),
        ]
        .into());
        assert_eq!(
            Target::read(&mem),
            Some(Target {
                surface: 0x1234,
                size: (32, 16)
            })
        );
        for (offset, bad) in [(0x78, 2), (0x80, 0), (0x84, 0), (0x88, 0), (0x94, 0x100)] {
            let old = mem.0.insert(at + offset, bad).unwrap();
            assert!(Target::read(&mem).is_none());
            mem.0.insert(at + offset, old);
        }
        mem.0.remove(&(at + 0x88));
        assert!(Target::read(&mem).is_none());
    }

    #[test]
    fn comparison_counts_every_channel_and_refuses_unpaired_images() {
        assert_eq!(
            compare(&[0, 1, 2, 3], &[0, 3, 2, 7]),
            Some(Difference {
                bytes: 4,
                changed: 2,
                max: 4,
                sum: 6
            })
        );
        assert!(compare(&[], &[]).is_none());
        assert!(compare(&[1], &[1, 2]).is_none());
    }

    #[test]
    fn reused_surface_and_shader_do_not_identify_another_light() {
        let a = Key {
            sequence: 1,
            definition: 2,
            shader: 3,
            surface: 4,
        };
        assert_ne!(a, Key { sequence: 2, ..a });
        assert_ne!(a, Key { definition: 5, ..a });
    }
}
