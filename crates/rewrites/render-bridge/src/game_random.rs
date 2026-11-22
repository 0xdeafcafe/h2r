//! The game's random step, for code that seeks along its stream.

/// The game's random step (`0x00027be8`: `imul 0x19660d`, `add
/// 0x3c6ef35f`).
pub const RANDOM_A: u32 = 0x0019_660d;
pub const RANDOM_C: u32 = 0x3c6e_f35f;
/// Steps [`random_steps`] searches between two seed values.
const RANDOM_STEPS: u32 = 1 << 16;

/// `n` such that `n` steps take `from` to `to`, if at most
/// [`RANDOM_STEPS`].
pub fn random_steps(from: u32, to: u32) -> Option<u32> {
    let mut x = from;
    for n in 0..=RANDOM_STEPS {
        if x == to {
            return Some(n);
        }
        x = x.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
    }
    None
}
