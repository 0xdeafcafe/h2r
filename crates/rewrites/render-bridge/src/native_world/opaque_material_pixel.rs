//! Variant pixel-shader definition builder (0x0001ba00). The definition and
//! fragment offsets live in the material cache across draws. Later constant
//! bindings patch individual channels in this same buffer.

use guest_layout::memory::{GuestMemory, MemFault};

const PASS: u32 = 0x20;
const IMPLEMENTATION: u32 = 0x24;
const ENGINE_INTS: u32 = 0x300;
const DEFINITION: u32 = 0x1424;
const FRAGMENT_OFFSETS: u32 = 0x1520;

const PASS_FRAGMENTS: u32 = 0x24;
const PASS_VARIANTS: u32 = 0x2c;
const PASS_STAGES: u32 = 0x34;
const IMPLEMENTATION_FRAGMENTS: u32 = 0xf6;
const IMPLEMENTATION_MODES: u32 = 0xde;

const ALPHA_INPUTS: u32 = 0;
const FINAL_ABCD: u32 = 8;
const FINAL_EFG: u32 = 9;
const CONSTANT0: u32 = 10;
const CONSTANT1: u32 = 18;
const ALPHA_OUTPUTS: u32 = 26;
const RGB_INPUTS: u32 = 34;
const FINAL_CONSTANT0: u32 = 43;
const FINAL_CONSTANT1: u32 = 44;
const RGB_OUTPUTS: u32 = 45;
const COMBINER_COUNT: u32 = 53;
const TEXTURE_MODES: u32 = 54;
const DOT_MAPPING: u32 = 55;
const INPUT_TEXTURE: u32 = 56;

const DEFAULT_FINAL_ABCD: u32 = 0xc;
const DEFAULT_FINAL_EFG: u32 = 0x2000;
const COMBINER_FLAGS: u32 = 0x11100;

/// `cache` is the material context in ESI. Read the selector from the
/// current engine integer bank, after this draw's bindings have updated it.
pub(super) fn build(mem: &mut dyn GuestMemory, cache: u32) -> Result<(), MemFault> {
    let pass = mem.u32(cache.wrapping_add(PASS))?;
    let implementation = mem.u32(cache.wrapping_add(IMPLEMENTATION))?;
    let variants = mem.u32(pass.wrapping_add(PASS_VARIANTS))?;
    let stages = mem.u32(pass.wrapping_add(PASS_STAGES))?;
    let ps = cache.wrapping_add(DEFINITION);
    let word = |index: u32| ps.wrapping_add(4 * index);
    for (from, to) in [(0, TEXTURE_MODES), (8, INPUT_TEXTURE), (4, DOT_MAPPING)] {
        let value = mem.u32(implementation.wrapping_add(IMPLEMENTATION_MODES + from))?;
        mem.set_u32(word(to), value)?;
    }

    let range = u32::from(mem.u16(implementation.wrapping_add(IMPLEMENTATION_FRAGMENTS))?);
    let mut general = 0;
    let mut has_final = false;
    for k in 0..range >> 9 {
        let fragments = mem.u32(pass.wrapping_add(PASS_FRAGMENTS))?;
        let fragment = fragments.wrapping_add(4 * ((range & 0x1ff) + k));
        mem.set_u8(cache.wrapping_add(FRAGMENT_OFFSETS + k), general as u8)?;
        let selector = mem.u16(fragment)? as i16;
        let base = u32::from(mem.u16(fragment.wrapping_add(2))? & 0x1ff);
        let variant = if selector == -1 {
            base
        } else {
            let offset = (i32::from(selector) * 4) as u32;
            base.wrapping_add(mem.u32(cache.wrapping_add(ENGINE_INTS).wrapping_add(offset))?)
        };
        let variant = variants.wrapping_add(variant.wrapping_mul(6));
        let stage_range = u32::from(mem.u16(variant.wrapping_add(4))?);
        let count = stage_range >> 9;
        let final_stage = count != 0 && mem.u8(variant.wrapping_add(2))? & 1 != 0;

        for j in 0..count {
            let stage = stages.wrapping_add(32 * ((stage_range & 0x1ff) + j));
            if final_stage && j + 1 == count {
                for (from, to) in [(0, FINAL_ABCD), (8, FINAL_EFG),
                    (16, FINAL_CONSTANT0), (20, FINAL_CONSTANT1)] {
                    let value = mem.u32(stage.wrapping_add(from))?;
                    mem.set_u32(word(to), value)?;
                }
                has_final = true;
            } else {
                for (i, to) in [RGB_INPUTS, RGB_OUTPUTS, ALPHA_INPUTS, ALPHA_OUTPUTS,
                    CONSTANT0, CONSTANT1].into_iter().enumerate() {
                    let value = mem.u32(stage.wrapping_add(4 * i as u32))?;
                    mem.set_u32(word(to + general), value)?;
                }
                general += 1;
            }
        }
    }
    if !has_final {
        mem.set_u32(word(FINAL_ABCD), DEFAULT_FINAL_ABCD)?;
        mem.set_u32(word(FINAL_EFG), DEFAULT_FINAL_EFG)?;
    }
    mem.set_u32(word(COMBINER_COUNT), general.max(1) | COMBINER_FLAGS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn selected_fragments_patch_only_their_combiner_words_and_offset_bytes() {
        let (cache, pass, imp, fragments, variants, stages) =
            (0x1000u32, 0x3000u32, 0x3100u32, 0x3300u32, 0x3400u32, 0x3500u32);
        let mut mem = FlatMemory::new(cache, 0x3000);
        for (at, value) in [
            (cache + PASS, pass), (cache + IMPLEMENTATION, imp),
            (pass + PASS_FRAGMENTS, fragments), (pass + PASS_VARIANTS, variants),
            (pass + PASS_STAGES, stages),
            (cache + ENGINE_INTS + 12, (-1i32) as u32),
        ] {
            mem.set_u32(at, value).unwrap();
        }
        for i in 0..60 {
            mem.set_u32(cache + DEFINITION + 4*i, 0xaa00 + i).unwrap();
        }
        mem.set_u32(cache + FRAGMENT_OFFSETS, u32::MAX).unwrap();
        for (at, value) in [
            (imp + IMPLEMENTATION_FRAGMENTS, 2 << 9 | 1),
            (fragments + 4, u16::MAX), (fragments + 6, 0),
            (fragments + 8, 3), (fragments + 10, 3),
            (variants + 4, 1 << 9),
            (variants + 2*6 + 4, 2 << 9 | 1),
        ] {
            mem.set_u16(at, value).unwrap();
        }
        mem.set_u8(variants + 2*6 + 2, 1).unwrap();
        for i in 0..3 {
            mem.set_u32(imp + IMPLEMENTATION_MODES + 4*i, 0xc000+i).unwrap();
            for j in 0..6 {
                mem.set_u32(stages + 32*i + 4*j, 0x100*i+j).unwrap();
            }
        }

        build(&mut mem, cache).unwrap();
        let mut expected: [u32; 60] = std::array::from_fn(|i| 0xaa00 + i as u32);
        for (out, source) in [(0,2), (10,4), (18,5), (26,3), (34,0), (45,1)] {
            expected[out] = source;
            expected[out + 1] = 0x100 + source;
        }
        for (out, value) in [(8,0x200), (9,0x202), (43,0x204), (44,0x205),
            (53,0x11102), (54,0xc000), (55,0xc001), (56,0xc002)] {
            expected[out] = value;
        }
        let words = |m: &FlatMemory| -> [u32; 60] {
            std::array::from_fn(|i| m.u32(cache + DEFINITION + 4*i as u32).unwrap())
        };
        assert_eq!(words(&mem), expected);
        assert_eq!(mem.u32(cache + FRAGMENT_OFFSETS).unwrap(), 0xffff0100);

        // Switch to an empty variant. Its absent final combiner resets two
        // words while the preceding final constants and general slots survive.
        mem.set_u16(imp + IMPLEMENTATION_FRAGMENTS, 1 << 9 | 2).unwrap();
        mem.set_u32(cache + ENGINE_INTS + 12, 0).unwrap();
        build(&mut mem, cache).unwrap();
        expected[8] = DEFAULT_FINAL_ABCD;
        expected[9] = DEFAULT_FINAL_EFG;
        expected[53] = 0x11101;
        assert_eq!(words(&mem), expected);
        assert_eq!(mem.u32(cache + FRAGMENT_OFFSETS).unwrap(), 0xffff0100);
    }
}
