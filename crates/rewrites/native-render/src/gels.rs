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

const FIRST_SURFACE: u32 = 0x18;
const SURFACE_STRIDE: u32 = 0x18;
const LEVEL_COUNT: u32 = 0x78;
const MAX_LEVELS: u32 = (LEVEL_COUNT - FIRST_SURFACE) / SURFACE_STRIDE;

/// Exact call made by `0x0003a550`, after uploading the light constants.
pub fn is_producer(args: [u32; 6], ret: u32, building: u8) -> bool {
    ret == TEMPLATE_RETURN
        && building == 1
        && args[0] != 0
        && args[0] != u32::MAX
        && args[1..] == [LAYER, u32::MAX, 0, TARGET, 1]
}

/// The enabled target and its embedded level-zero surface header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub surface: u32,
    pub size: (u32, u32),
    levels: u32,
}

/// One attachment selected by the guest quad helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    pub surface: u32,
    pub size: (u32, u32),
}

impl Target {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        let at = TARGETS + TARGET_STRIDE * TARGET;
        let available = mem.read_u32(at + 0x88)? != 0;
        let hidden = mem.read_u32(at + 0x94)? & 0xff00 != 0;
        let width = mem.read_u32(at + 0x80)?;
        let height = mem.read_u32(at + 0x84)?;
        let levels = mem.read_u32(at + LEVEL_COUNT)?;
        let max_size_levels = u32::BITS - width.max(height).leading_zeros();
        if !available
            || hidden
            || width == 0
            || height == 0
            || !(1..=MAX_LEVELS.min(max_size_levels)).contains(&levels)
        {
            return None;
        }
        // The table's presence word is not the D3D surface header.
        // The level resolver (0x0001dcf0) selects the embedded header.
        Some(Self {
            surface: at + FIRST_SURFACE,
            size: (width, height),
            levels,
        })
    }

    pub fn level(self, index: u32) -> Option<Level> {
        (index < self.levels).then(|| Level {
            surface: self.surface + index * SURFACE_STRIDE,
            size: ((self.size.0 >> index).max(1), (self.size.1 >> index).max(1)),
        })
    }
}

/// Identity is per invocation, never just the reused target's address or
/// the shader datum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub sequence: u64,
    pub definition: u32,
    pub shader: u32,
    pub surface: u32,
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
    fn targets_must_be_readable_enabled_and_fit_the_surface_table() {
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
                surface: at + 0x18,
                size: (32, 16),
                levels: 1,
            })
        );
        for (offset, bad) in [
            (0x78, 0),
            (0x78, 5),
            (0x78, u32::MAX),
            (0x80, 0),
            (0x84, 0),
            (0x88, 0),
            (0x94, 0x100),
        ] {
            let old = mem.0.insert(at + offset, bad).unwrap();
            assert!(Target::read(&mem).is_none());
            mem.0.insert(at + offset, old);
        }
        mem.0.insert(at + 0x78, 4);
        mem.0.insert(at + 0x84, 1);
        let target = Target::read(&mem).unwrap();
        assert_eq!(
            (0..5).map(|index| target.level(index)).collect::<Vec<_>>(),
            [
                Some(Level { surface: at + 0x18, size: (32, 1) }),
                Some(Level { surface: at + 0x30, size: (16, 1) }),
                Some(Level { surface: at + 0x48, size: (8, 1) }),
                Some(Level { surface: at + 0x60, size: (4, 1) }),
                None,
            ],
        );
        mem.0.insert(at + 0x80, 4);
        assert!(Target::read(&mem).is_none());
        mem.0.remove(&(at + 0x88));
        assert!(Target::read(&mem).is_none());
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
