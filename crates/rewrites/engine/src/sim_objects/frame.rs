//! Retail frame helpers for segment bodies (docs/engine/object-update.md,
//! "The callout contract"). A body keeps every value that lives across a
//! guest call in the stack slot or callee-saved register retail keeps it
//! in, so these are the only ways a body touches its frame.

use super::Result;
use runtime::{CpuAbi, Guest};

/// `push word`: `esp -= 4`, then the store, as the instruction does.
pub fn push(c: &mut CpuAbi, g: &mut Guest, word: u32) -> Result<()> {
    let sp = c.esp.wrapping_sub(4);
    g.set_u32(sp, word)?;
    c.esp = sp;
    Ok(())
}

/// `pop`: the load, then `esp += 4`.
pub fn pop(c: &mut CpuAbi, g: &Guest) -> Result<u32> {
    let v = g.u32(c.esp)?;
    c.esp = c.esp.wrapping_add(4);
    Ok(v)
}

/// The dword at `[esp + off]`.
pub fn slot(c: &CpuAbi, g: &Guest, off: u32) -> Result<u32> {
    g.u32(c.esp.wrapping_add(off))
}

/// Store `v` at `[esp + off]`.
pub fn set_slot(c: &CpuAbi, g: &mut Guest, off: u32, v: u32) -> Result<()> {
    g.set_u32(c.esp.wrapping_add(off), v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn push_pop_and_slots_follow_the_instructions() {
        let mut g = Guest::new();
        g.map_zeroed(0x1000, 0x1000, Permissions::RW).unwrap();
        let mut c = CpuAbi { esp: 0x1800, ..Default::default() };
        push(&mut c, &mut g, 0xaabb_ccdd).unwrap();
        push(&mut c, &mut g, 7).unwrap();
        assert_eq!(c.esp, 0x17f8);
        assert_eq!(slot(&c, &g, 4).unwrap(), 0xaabb_ccdd);
        set_slot(&c, &mut g, 4, 9).unwrap();
        assert_eq!(pop(&mut c, &g).unwrap(), 7);
        assert_eq!(pop(&mut c, &g).unwrap(), 9);
        assert_eq!(c.esp, 0x1800);
    }

    #[test]
    fn a_faulting_push_leaves_esp() {
        let mut g = Guest::new();
        g.map_zeroed(0x1000, 0x1000, Permissions::RW).unwrap();
        let mut c = CpuAbi { esp: 0x1000, ..Default::default() };
        assert!(push(&mut c, &mut g, 1).is_err());
        assert_eq!(c.esp, 0x1000);
    }
}
