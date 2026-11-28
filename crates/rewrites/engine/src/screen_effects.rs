//! The scenario's screen effects `render_screen_effects` (`0x00013870`)
//! draws after its colour pass (docs/graphics/native-post-hud.md, stage 5,
//! "The screen effects"): which entries draw this frame and with what
//! values, as pure functions over guest memory, for the native renderer
//! to plan them in the game's place.
//!
//! (Code, `h2 disasm`.) After the colour pass, `0x000139ba..0x00013b72`:
//! with a scenario (`[0x004e0350]`), a view kind (`[0x00485898]`) outside
//! 5..7, the byte `[0x00485607]` and the word `[0x004850c8]` clear and
//! [`full_screen_single_view`] (`0x00022710`), the window `0x004b9dd0`
//! becomes four corners ([`corners`]); then per entry of the scenario
//! block at `+0x3d0` (count, re-read each step) / `+0x3d4` (address,
//! [`ENTRY_SIZE`] bytes): skipped with its tag datum (`+0x14`) -1 or both
//! function indices (`+0x20`, `+0x22`) -1; each index not -1 is evaluated
//! by [`function_value`] (`0x00134fc0`), the other value 0; with either
//! value not equal to zero (NaN draws) `0x0002d140(values, 2, 2,
//! corners)` runs with EAX the datum. The arithmetic is the original's
//! (SSE under the caller's MXCSR); checked against the lifted original by
//! `engine/tests/screen_effects_differential.rs`.

use crate::function_eval;
use crate::light_constants::{Error, Sse};
use guest_layout::memory::GuestMemory;

/// The scenario pointer.
pub const SCENARIO: u32 = 0x004e_0350;
/// The scenario's function block: count, then address
/// ([`FUNCTION_SIZE`] bytes each).
pub const SCENARIO_FUNCTIONS: u32 = 0x3c0;
pub const FUNCTION_SIZE: u32 = 0x18;
/// The scenario's screen-effect block: count, then address.
pub const SCENARIO_SCREEN_EFFECTS: u32 = 0x3d0;
pub const ENTRY_SIZE: u32 = 0x24;
/// The functions' runtime state (`0x20` bytes each: `+0x04` the input,
/// `+0x1c` added to it).
pub const FUNCTION_STATE: u32 = 0x004e_6740;
pub const STATE_SIZE: u32 = 0x20;
/// The view kind; 5..7 skip the screen effects.
pub const VIEW_KIND: u32 = 0x0048_5898;
/// A byte and a word that, set, skip them.
pub const OFF_BYTE: u32 = 0x0048_5607;
pub const OFF_WORD: u32 = 0x0048_50c8;
/// The window (top, left, bottom, right shorts).
pub const WINDOW: u32 = 0x004b_9dd0;
/// 1.0 in the image.
pub const ONE: u32 = 0x0045_dbc0;

/// `0x00022710`: the viewport is the whole 640x480 screen (`0x00485648`
/// top, left, bottom, right shorts) and at most one view
/// (`[0x004ba04c]`).
pub fn full_screen_single_view(mem: &dyn GuestMemory) -> Result<bool, Error> {
    Ok(mem.u16(0x0048_564a)? == 0
        && mem.u16(0x0048_5648)? == 0
        && mem.u16(0x0048_564e)? == 0x280
        && mem.u16(0x0048_564c)? == 0x1e0
        && mem.u32(0x004b_a04c)? as i32 <= 1)
}

/// `0x0013b390` on the data reference at `r` (size, then address) at
/// input `x`, range input 0 ([`function_eval::evaluate`]).
fn evaluate(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, r: u32, x: u32) -> Result<u32, Error> {
    function_eval::evaluate(mem, sse, control, r, x, 0)
}

/// The output range of the blob at `ptr` over `v` unless its flags take it
/// raw: `v` clamped to 0..1, then `lo + (hi - lo) * v`.
pub(crate) fn ranged(mem: &dyn GuestMemory, sse: &mut Sse, ptr: u32, v: u32) -> Result<u32, Error> {
    if mem.u8(ptr.wrapping_add(1))? & 0xf0 != 0 {
        return Ok(v);
    }
    let one = mem.u32(ONE)?;
    let v = sse.clamp(v, 0, one)?;
    let (lo, hi) = (mem.u32(ptr.wrapping_add(4))?, mem.u32(ptr.wrapping_add(8))?);
    let d = sse.sub(hi, lo)?;
    let d = sse.mul(d, v)?;
    sse.add(d, lo)
}

/// `0x0013bb90`: [`evaluate`] then the output range.
fn property(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, r: u32, x: u32) -> Result<u32, Error> {
    let ptr = mem.u32(r.wrapping_add(4))?;
    if ptr == 0 || mem.u32(r)? as i32 <= 0 {
        return Ok(0);
    }
    let v = evaluate(mem, sse, control, r, x)?;
    ranged(mem, sse, ptr, v)
}

/// `0x00134fe0(index, t)`: function `index` at `t` plus its state's
/// `+0x1c`; with a parent (`+0x16`, itself without one), the product of
/// the parent's value and its own raw value, through its output range.
fn value_at(mem: &dyn GuestMemory, sse: &mut Sse, control: u16, index: i32, t: u32) -> Result<u32, Error> {
    let base = mem.u32(FUNCTION_STATE)?;
    let state = (index as u32).wrapping_shl(5).wrapping_add(base).wrapping_add(4);
    if state == 0 {
        return Ok(0);
    }
    let scenario = mem.u32(SCENARIO)?;
    let f = sse.add(mem.u32(state.wrapping_add(0x18))?, t)?;
    let functions = mem.u32(scenario.wrapping_add(SCENARIO_FUNCTIONS + 4))?;
    let entry = functions.wrapping_add((index as u32).wrapping_mul(FUNCTION_SIZE));
    let parent = i32::from(mem.u16(entry.wrapping_add(0x16))? as i16);
    let clamped = if parent < 0 {
        0
    } else {
        let last = (mem.u32(scenario.wrapping_add(SCENARIO_FUNCTIONS))? as i32).wrapping_sub(1);
        if parent <= last { parent } else { last }
    };
    let data = entry.wrapping_add(0xc);
    if clamped != parent {
        return property(mem, sse, control, data, f);
    }
    let parents = functions.wrapping_add((parent as u32).wrapping_mul(FUNCTION_SIZE));
    if mem.u16(parents.wrapping_add(0x16))? != 0xffff {
        return Ok(0);
    }
    let own = evaluate(mem, sse, control, data, f)?;
    let parent_state = (parent as u32).wrapping_shl(5).wrapping_add(base).wrapping_add(4);
    let p = if parent_state == 0 {
        0
    } else {
        value_at(mem, sse, control, parent, mem.u32(parent_state)?)?
    };
    let ptr = mem.u32(data.wrapping_add(4))?;
    let v = sse.mul(p, own)?;
    ranged(mem, sse, ptr, v)
}

/// `0x00134fc0` (ECX `index`): scenario function `index` at its state's
/// input, as f32 bits, under `mxcsr` and the x87 control word `control`.
pub fn function_value(mem: &dyn GuestMemory, index: i32, mxcsr: u32, control: u16) -> Result<u32, Error> {
    let mut sse = Sse { mxcsr };
    let state = (index as u32).wrapping_shl(5).wrapping_add(mem.u32(FUNCTION_STATE)?).wrapping_add(4);
    if state == 0 {
        return Ok(0);
    }
    let t = mem.u32(state)?;
    value_at(mem, &mut sse, control, index, t)
}

/// The four corners `0x0002d140` gets (x, y, u, v each, f32 bits): the
/// window's top left, top right, bottom left, bottom right, with u and v
/// 0 or 1.
pub fn corners(mem: &dyn GuestMemory, mxcsr: u32) -> Result<[[u32; 4]; 4], Error> {
    let mut sse = Sse { mxcsr };
    let mut at = |o: u32| -> Result<u32, Error> { sse.int(i32::from(mem.u16(WINDOW + o)? as i16)) };
    let (top, left, bottom, right) = (at(0)?, at(2)?, at(4)?, at(6)?);
    let one = mem.u32(ONE)?;
    Ok([[left, top, 0, 0], [right, top, one, 0], [left, bottom, 0, one], [right, bottom, one, one]])
}

/// One screen effect `0x0002d140` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drawn {
    /// Its entry in the scenario block.
    pub entry: u32,
    /// The `egor` tag datum (EAX).
    pub datum: u32,
    /// The four values it gets (the two functions', then two zeros).
    pub values: [u32; 4],
}

/// Whether the loop runs at all: a scenario, a view kind outside 5..7,
/// both off flags clear, [`full_screen_single_view`].
pub fn gated(mem: &dyn GuestMemory) -> Result<bool, Error> {
    if mem.u32(SCENARIO)? == 0 {
        return Ok(false);
    }
    let kind = mem.u32(VIEW_KIND)? as i32;
    if (5..=7).contains(&kind) || mem.u8(OFF_BYTE)? != 0 || mem.u32(OFF_WORD)? != 0 {
        return Ok(false);
    }
    full_screen_single_view(mem)
}

/// The most entries [`drawn`] walks.
pub const MAX_ENTRIES: u32 = 256;

/// The screen effects that draw this frame, in order (none when not
/// [`gated`]), under the caller's `mxcsr` and x87 `control`. More than [`MAX_ENTRIES`]
/// entries is refused ([`Error::Unbounded`]).
pub fn drawn(mem: &dyn GuestMemory, mxcsr: u32, control: u16) -> Result<Vec<Drawn>, Error> {
    if !gated(mem)? {
        return Ok(Vec::new());
    }
    let mut sse = Sse { mxcsr };
    let mut out = Vec::new();
    let mut i = 0u32;
    loop {
        let scenario = mem.u32(SCENARIO)?;
        let count = mem.u32(scenario.wrapping_add(SCENARIO_SCREEN_EFFECTS))? as i32;
        if i as i32 >= count {
            break;
        }
        if i >= MAX_ENTRIES {
            return Err(Error::Unbounded);
        }
        let base = mem.u32(scenario.wrapping_add(SCENARIO_SCREEN_EFFECTS + 4))?;
        let e = base.wrapping_add(i * ENTRY_SIZE);
        let datum = mem.u32(e.wrapping_add(0x14))?;
        let (a, b) = (mem.u16(e.wrapping_add(0x20))?, mem.u16(e.wrapping_add(0x22))?);
        if datum != u32::MAX && !(a == 0xffff && b == 0xffff) {
            let value = |idx: u16| -> Result<u32, Error> {
                if idx == 0xffff { Ok(0) } else { function_value(mem, i32::from(idx as i16), mxcsr, control) }
            };
            let (v0, v1) = (value(a)?, value(b)?);
            if !sse.equal(v0, 0)? || !sse.equal(v1, 0)? {
                out.push(Drawn {
                    entry: i,
                    datum,
                    values: [v0, v1, 0, 0],
                });
            }
        }
        i += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const SCEN: u32 = 0x0060_0000;
    const FUNCS: u32 = 0x0061_0000;
    const STATE: u32 = 0x0062_0000;
    const BLOBS: u32 = 0x0063_0000;
    const EFFECTS: u32 = 0x0064_0000;

    /// A linear function `a x + b` (type 4, set 0 at payload words 4, 5)
    /// with output range `lo..hi`, raw when `raw`.
    fn linear(m: &mut FlatMemory, at: u32, a: f32, b: f32, lo: f32, hi: f32, raw: bool) {
        m.write(at, &[0; 0x44]).unwrap();
        m.set_u8(at, 4).unwrap();
        m.set_u8(at + 1, if raw { 0x10 } else { 0 }).unwrap();
        m.set_f32(at + 4, lo).unwrap();
        m.set_f32(at + 8, hi).unwrap();
        m.set_f32(at + 0x14 + 16, a).unwrap();
        m.set_f32(at + 0x14 + 20, b).unwrap();
    }

    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(ONE, 1.0f32.to_bits()).unwrap();
        m.set_u32(SCENARIO, SCEN).unwrap();
        m.set_u32(FUNCTION_STATE, STATE).unwrap();
        m.set_u32(SCEN + SCENARIO_FUNCTIONS, 3).unwrap();
        m.set_u32(SCEN + SCENARIO_FUNCTIONS + 4, FUNCS).unwrap();
        for i in 0..3 {
            let f = FUNCS + i * FUNCTION_SIZE;
            m.set_u16(f + 0x16, 0xffff).unwrap();
            m.set_u32(f + 0xc, 0x2c).unwrap();
            m.set_u32(f + 0x10, BLOBS + i * 0x100).unwrap();
        }
        // 0: 0.5 x + 0.25 into 0..2; 1: raw x; 2: the product with 0's.
        linear(&mut m, BLOBS, 0.5, 0.25, 0.0, 2.0, false);
        linear(&mut m, BLOBS + 0x100, 1.0, 0.0, 0.0, 1.0, true);
        linear(&mut m, BLOBS + 0x200, 1.0, 0.0, 10.0, 20.0, false);
        m.set_u16(FUNCS + 2 * FUNCTION_SIZE + 0x16, 0).unwrap();
        for (i, (t, add)) in [(0.5f32, 0.0f32), (0.25, 0.5), (1.0, -0.5)].iter().enumerate() {
            m.set_f32(STATE + 0x20 * i as u32 + 4, *t).unwrap();
            m.set_f32(STATE + 0x20 * i as u32 + 0x1c, *add).unwrap();
        }
        m
    }

    fn value(m: &FlatMemory, i: i32) -> f32 {
        f32::from_bits(function_value(m, i, 0x1f80, 0x27f).unwrap())
    }

    #[test]
    fn a_function_is_its_curve_through_its_range() {
        let m = world();
        // 0.5 * 0.5 + 0.25 = 0.5, clamped by the evaluator, then 0..2.
        assert_eq!(value(&m, 0), 1.0);
        // Raw: x = 0.25 + 0.5.
        assert_eq!(value(&m, 1), 0.75);
        // With parent 0: 0's value 1.0 (at its own input) times its own
        // raw 0.5 (1.0 - 0.5), into 10..20.
        assert_eq!(value(&m, 2), 15.0);
    }

    #[test]
    fn a_parent_with_a_parent_or_out_of_range_changes_the_path() {
        let mut m = world();
        m.set_u16(FUNCS + 0x16, 1).unwrap();
        assert_eq!(value(&m, 2), 0.0, "a grandparent: 0");
        let mut m = world();
        m.set_u16(FUNCS + 2 * FUNCTION_SIZE + 0x16, 7).unwrap();
        assert_eq!(value(&m, 2), 15.0, "past the count: no parent, its own 0.5 into 10..20");
        let mut m = world();
        m.set_u32(FUNCS + 0xc, 0).unwrap();
        assert_eq!(value(&m, 0), 0.0, "an empty blob");
    }

    #[test]
    fn the_entries_that_draw() {
        let mut m = world();
        m.set_u32(VIEW_KIND, 1).unwrap();
        m.set_u16(0x0048_564e, 0x280).unwrap();
        m.set_u16(0x0048_564c, 0x1e0).unwrap();
        m.set_u32(SCEN + SCENARIO_SCREEN_EFFECTS, 4).unwrap();
        m.set_u32(SCEN + SCENARIO_SCREEN_EFFECTS + 4, EFFECTS).unwrap();
        let entry = |m: &mut FlatMemory, i: u32, datum: u32, a: u16, b: u16| {
            let e = EFFECTS + i * ENTRY_SIZE;
            m.set_u32(e + 0x14, datum).unwrap();
            m.set_u16(e + 0x20, a).unwrap();
            m.set_u16(e + 0x22, b).unwrap();
        };
        entry(&mut m, 0, 0xe001_0002, 0, 0xffff);
        entry(&mut m, 1, u32::MAX, 0, 0);
        entry(&mut m, 2, 0xe003_0004, 0xffff, 0xffff);
        entry(&mut m, 3, 0xe005_0006, 0xffff, 1);
        let d = drawn(&m, 0x1f80, 0x27f).unwrap();
        assert_eq!(
            d,
            vec![
                Drawn { entry: 0, datum: 0xe001_0002, values: [1.0f32.to_bits(), 0, 0, 0] },
                Drawn { entry: 3, datum: 0xe005_0006, values: [0, 0.75f32.to_bits(), 0, 0] },
            ]
        );
        // A zero value draws nothing.
        m.set_f32(STATE + 0x20 + 0x1c, -0.25).unwrap();
        assert_eq!(drawn(&m, 0x1f80, 0x27f).unwrap().len(), 1);
        // Split screen, or a view kind 5..7: none.
        m.set_u32(0x004b_a04c, 2).unwrap();
        assert!(drawn(&m, 0x1f80, 0x27f).unwrap().is_empty());
        m.set_u32(0x004b_a04c, 1).unwrap();
        m.set_u32(VIEW_KIND, 6).unwrap();
        assert!(drawn(&m, 0x1f80, 0x27f).unwrap().is_empty());
    }

    #[test]
    fn the_corners_are_the_window() {
        let mut m = world();
        for (o, v) in [(0u32, 10u16), (2, 20), (4, 470), (6, 630)] {
            m.set_u16(WINDOW + o, v).unwrap();
        }
        let c = corners(&m, 0x1f80).unwrap();
        let f = |v: f32| v.to_bits();
        assert_eq!(c[0], [f(20.0), f(10.0), 0, 0]);
        assert_eq!(c[3], [f(630.0), f(470.0), f(1.0), f(1.0)]);
    }
}
