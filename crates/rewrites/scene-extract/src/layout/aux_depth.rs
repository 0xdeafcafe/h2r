//! The auxiliary state toggle `0x00012d50` (BL on or off, EAX the view,
//! 0 for the current one at `0x0048568c`; one stack word, 0 for its own
//! buffer), which the geometry draw's prologue `0x0004d640` calls on and
//! its epilogue `0x0004d720` off for a render record with bit 11
//! (docs/graphics/framescene.md, slice 3). **Code**, `h2 disasm`:
//!
//! - **The clip range.** The device's program clip range (`+0x1c10`,
//!   `+0x1c14`) becomes `[near · k, far]`, `near` and `far` the game's
//!   clip words ([`CLIP_NEAR`], [`CLIP_FAR`]) and `k` [`AUX_SCALE`] on,
//!   1.0 ([`ONE`]) off; the game owns it (`+0x1c18` bit 1), and the
//!   viewport fragment `0x003f6190` pushes it.
//! - **The rows.** It builds the view-projection rows from the view: per
//!   row `i`, the projection column at `+0x78 + 4i` (four floats `0x10`
//!   apart) through the view's 3x4 matrix at `+4`, times
//!   [`SCALE_NUMERATOR`] / [`SCALE_DENOMINATOR`] ([`rows`] gives the
//!   order of the operations); on, it scales all sixteen by `k`. It
//!   uploads them to `c[0..3]` (`0x003f7280`, ECX 0) only when BL differs
//!   from the byte at [`CACHED`], which it then sets.
//!
//! The arithmetic is single precision under the default rounding (**S**:
//! the game's MXCSR rounds to nearest without flushing denormals here).

use frame_scene::{AuxDepth, Row};

use crate::GuestRead;

/// The view the toggle reads when called with EAX 0.
pub const VIEW: u32 = 0x0048_568c;
/// `k`, the scale the toggle applies on.
pub const AUX_SCALE: u32 = 0x0044_07b8;
/// 1.0, the near factor off.
pub const ONE: u32 = 0x0045_dbc0;
/// The game's program clip range, near and far, in W-buffer units.
pub const CLIP_NEAR: u32 = 0x0048_5adc;
pub const CLIP_FAR: u32 = 0x0048_5ae0;
/// The rows' scale: this over [`SCALE_DENOMINATOR`].
pub const SCALE_NUMERATOR: u32 = 0x0045_dd40;
pub const SCALE_DENOMINATOR: u32 = 0x0048_565c;
/// The byte: whether `c[0..3]` hold the rows on (1) or off (0).
pub const CACHED: u32 = 0x0055_e6bc;
/// The view's 3x4 matrix (twelve floats from `+4`) and the projection's
/// columns (`+0x78`).
const VIEW_MATRIX: u32 = 4;
const PROJECTION: u32 = 0x78;

fn float(mem: &dyn GuestRead, at: u32) -> Option<f32> {
    mem.read_u32(at).map(f32::from_bits)
}

/// The rows `0x00012de2`..`0x00012edc` build from the view at `view`,
/// unscaled, as the toggle orders the operations.
pub fn rows(mem: &dyn GuestRead, view: u32) -> Option<[Row; 4]> {
    let s = float(mem, SCALE_NUMERATOR)? / float(mem, SCALE_DENOMINATOR)?;
    let v = |k: u32| float(mem, view.checked_add(VIEW_MATRIX + 4 * (k - 1))?);
    let (v1, v2, v3, v4, v5, v6) = (v(1)?, v(2)?, v(3)?, v(4)?, v(5)?, v(6)?);
    let (v7, v8, v9, v10, v11, v12) = (v(7)?, v(8)?, v(9)?, v(10)?, v(11)?, v(12)?);
    let mut out = [[0.0; 4]; 4];
    for (i, row) in out.iter_mut().enumerate() {
        let a = |k: u32| float(mem, view.checked_add(PROJECTION + 4 * i as u32 + 0x10 * k)?);
        let (a0, a1, a2, a3) = (a(0)?, a(1)?, a(2)?, a(3)?);
        *row = [
            (a2 * v3 + v1 * a0 + a1 * v2) * s,
            (a2 * v6 + a0 * v4 + a1 * v5) * s,
            (a2 * v9 + v8 * a1 + a0 * v7) * s,
            (a2 * v12 + v11 * a1 + a0 * v10) * s + a3 * s,
        ];
    }
    Some(out)
}

/// What the toggle sets for the current view, on and off; `None` when a
/// word does not read.
pub fn read(mem: &dyn GuestRead) -> Option<AuxDepth> {
    let k = float(mem, AUX_SCALE)?;
    let one = float(mem, ONE)?;
    let (near, far) = (float(mem, CLIP_NEAR)?, float(mem, CLIP_FAR)?);
    let rest_rows = rows(mem, VIEW)?;
    Some(AuxDepth {
        scale: k,
        rows: rest_rows.map(|r| r.map(|x| x * k)),
        clip: [k * near, far],
        rest_rows,
        rest_clip: [one * near, far],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    /// A view whose matrix is the identity's first three rows and whose
    /// projection columns hold `p[i][k]` at `+0x78 + 4i + 0x10k`.
    fn view(mem: &mut Mem, p: [[f32; 4]; 4]) {
        let m = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 5.0, 6.0, 7.0];
        for (k, x) in m.into_iter().enumerate() {
            mem.f(VIEW + VIEW_MATRIX + 4 * k as u32, x);
        }
        for (i, col) in p.iter().enumerate() {
            for (k, x) in col.iter().enumerate() {
                mem.f(VIEW + PROJECTION + 4 * i as u32 + 0x10 * k as u32, *x);
            }
        }
    }

    fn globals(mem: &mut Mem, k: f32) {
        mem.f(AUX_SCALE, k);
        mem.f(ONE, 1.0);
        mem.f(CLIP_NEAR, 0.25);
        mem.f(CLIP_FAR, 1000.0);
        mem.f(SCALE_NUMERATOR, 2.0);
        mem.f(SCALE_DENOMINATOR, 4.0);
    }

    #[test]
    fn the_rows_are_the_projection_through_the_view_scaled() {
        let mut mem = Mem::default();
        globals(&mut mem, 0.5);
        // Row i reads a0..a3 = p[i]; with v1..v9 the identity and v10..v12
        // a translation, row i = (a0, a1, a2, a0·5 + a1·6 + a2·7 + a3) · s.
        view(&mut mem, [[1.0, 2.0, 3.0, 4.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);
        let r = rows(&mem, VIEW).unwrap();
        assert_eq!(r[0], [0.5, 1.0, 1.5, (5.0 + 12.0 + 21.0) * 0.5 + 2.0]);
        assert_eq!(r[1], [0.0, 0.5, 0.0, 3.0]);
        assert_eq!(r[3], [0.0, 0.0, 0.0, 0.5]);
    }

    #[test]
    fn on_scales_the_rows_and_the_near_clip_and_off_does_not() {
        let mut mem = Mem::default();
        globals(&mut mem, 0.125);
        view(&mut mem, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]);
        let a = read(&mem).unwrap();
        assert_eq!(a.scale, 0.125);
        assert_eq!(a.rest_clip, [0.25, 1000.0]);
        assert_eq!(a.clip, [0.25 * 0.125, 1000.0], "the far end is the game's either way");
        for (on, off) in a.rows.iter().zip(&a.rest_rows) {
            assert_eq!(*on, off.map(|x| x * 0.125));
        }
        assert_eq!(a.rest_rows, rows(&mem, VIEW).unwrap());
        mem.0.remove(&AUX_SCALE);
        assert!(read(&mem).is_none());
    }

    #[test]
    fn the_sums_keep_the_toggles_order() {
        // Row 0's first entry is (a2·v3 + v1·a0) + a1·v2. With those terms
        // 1e8, -1e8 and 1 it is 1; summed the other way round it is 0.
        let mut mem = Mem::default();
        globals(&mut mem, 1.0);
        mem.f(SCALE_NUMERATOR, 1.0);
        mem.f(SCALE_DENOMINATOR, 1.0);
        let m = [1.0, 1.0, 1.0e8, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        for (k, x) in m.into_iter().enumerate() {
            mem.f(VIEW + VIEW_MATRIX + 4 * k as u32, x);
        }
        for i in 0..4u32 {
            for k in 0..4u32 {
                let x = match (i, k) {
                    (0, 0) => -1.0e8,
                    (0, 1) | (0, 2) => 1.0,
                    _ => 0.0,
                };
                mem.f(VIEW + PROJECTION + 4 * i + 0x10 * k, x);
            }
        }
        let r = rows(&mem, VIEW).unwrap();
        assert_eq!(r[0][0], 1.0);
        assert_eq!(1.0e8f32 + (-1.0e8f32 + 1.0), 0.0, "the other order");
    }
}
