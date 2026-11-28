//! The movie's picture on the native renderer
//! (docs/graphics/retire-d3d-display.md, "Movies").
//!
//! The wrapper's tick (`0x00156040`) runs its decode and blit
//! (`0x001568d0`: `BinkDoFrame`, `BinkNextFrame`, a lock of its texture,
//! `BinkCopyToBuffer`) and its draw (`0x00156960`: one screen quad over that
//! texture). Playback replaces both: the decode and blit only advances
//! the stream, the frame
//! stays in the host buffer, and the draw hands the host the quad the
//! guest would have drawn and the decoded picture ([`MovieDisplay`]). The
//! guest's texture is never locked or written, so a draw the host does not
//! take shows nothing: each one is counted by its reason
//! (`native_ui_movie_*`), never drawn from the guest's texture.

use std::sync::{Arc, Mutex};

use crate::addr::{ST_BINK, ST_FLAGS, ST_HEIGHT, ST_WIDTH};

type R = Result<runtime::TransferAbi, runtime::GuestFault>;

/// The rectangle `0x00156960` places the movie in: four `i16`, top, left,
/// bottom, right.
pub const MOVIE_RECT: u32 = 0x0048_5a8a;
/// The screen-quad record `0x00156960` begins its batch with
/// (`0x00035b90`'s argument), in the wrapper's state; `0x00156090` fills
/// it with the movie's bitmap at stage 0.
pub const MOVIE_RECORD: u32 = 0x004e_91c4;
/// The start flag that stretches the movie over [`MOVIE_RECT`]; without
/// it the movie is centred at its own size.
pub const FLAG_STRETCH: u32 = 0x10;

/// The quad `0x00156960` draws (Code, `0x00156960..0x00156a79`): four
/// corners of `(x, y, u, v)` in the screen quads' pixels and the movie's
/// texels, in the order it pushes them, each white (`0xffffffff`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovieQuad {
    pub corners: [[f32; 4]; 4],
    pub argb: u32,
}

/// The guest's quad for `rect` (top, left, bottom, right), the start
/// `flags` and the movie's `size` (width, height), with its own integer
/// arithmetic: stretched, the rectangle's edges; otherwise each extent is
/// `(rect - size) / 2` to `(rect + size) / 2`, halved toward zero, from 0
/// (not from the rectangle's corner) and kept to 16 bits. Corner 0 is the
/// top left, then clockwise; `u` and `v` run from 0 to the movie's size.
pub fn movie_quad(rect: [i16; 4], flags: u32, size: [i16; 2]) -> MovieQuad {
    let [top, left, bottom, right] = rect;
    let [w, h] = size;
    let (x0, x1, y0, y1) = if flags & FLAG_STRETCH != 0 {
        (left, right, top, bottom)
    } else {
        let half = |a: i32| (a / 2) as i16;
        let (span_x, span_y) = (i32::from(right.wrapping_sub(left)), i32::from(bottom.wrapping_sub(top)));
        let (w, h) = (i32::from(w), i32::from(h));
        (half(span_x - w), half(span_x + w), half(span_y - h), half(span_y + h))
    };
    let (x0, x1, y0, y1) = (f32::from(x0), f32::from(x1), f32::from(y0), f32::from(y1));
    let (u, v) = (f32::from(w), f32::from(h));
    MovieQuad {
        corners: [[x0, y0, 0.0, 0.0], [x1, y0, u, 0.0], [x1, y1, u, v], [x0, y1, 0.0, v]],
        argb: 0xffff_ffff,
    }
}

/// A movie's decoded picture on the host: `width` by `height` texels, B,
/// G, R, A, row-major, as `BinkCopyToBuffer` would have written them to
/// the guest's 32-bit texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoviePicture {
    /// The movie's HBINK handle: one id while it plays.
    pub movie: u32,
    /// The 0-based frame the picture holds.
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<[u8]>,
}

/// One draw of the wrapper's movie: the quad, the record its batch
/// begins with, and the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct MovieDraw {
    pub quad: MovieQuad,
    pub record: u32,
    pub picture: MoviePicture,
}

/// The host that draws the movie natively.
pub trait MovieDisplay: Send + Sync {
    /// Draws `draw` where the guest's quad would be, at this point in the
    /// frame: whether it was placed. A host that refuses counts its
    /// reason.
    fn show(&self, mem: &mut runtime::Guest, draw: &MovieDraw) -> bool;
}

static DISPLAY: Mutex<Option<Arc<dyn MovieDisplay>>> = Mutex::new(None);

/// The host the native movie draw hands its picture to; `None` removes
/// it.
pub fn set_display(display: Option<Arc<dyn MovieDisplay>>) {
    *DISPLAY.lock().unwrap_or_else(|e| e.into_inner()) = display;
}

fn display() -> Option<Arc<dyn MovieDisplay>> {
    DISPLAY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What the draw reads of the wrapper's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawInputs {
    pub handle: u32,
    pub flags: u32,
    pub rect: [i16; 4],
    pub size: [i16; 2],
}

impl DrawInputs {
    pub fn read(mem: &runtime::Guest) -> Result<DrawInputs, runtime::GuestFault> {
        let short = |at: u32| mem.u16(at).map(|v| v as i16);
        Ok(DrawInputs {
            handle: mem.u32(ST_BINK)?,
            flags: u32::from(mem.u8(ST_FLAGS)?),
            rect: [short(MOVIE_RECT)?, short(MOVIE_RECT + 2)?, short(MOVIE_RECT + 4)?, short(MOVIE_RECT + 6)?],
            size: [short(ST_WIDTH)?, short(ST_HEIGHT)?],
        })
    }
}

impl crate::BinkHle {
    /// `0x001568d0` in Rust: `BinkDoFrame` and
    /// `BinkNextFrame` on the open movie, as the guest calls them; no lock
    /// and no copy. EAX 0, the guest's after a copy that wrote.
    pub(crate) fn decode_native(&self, cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx) -> R {
        let mut mem = ctx.memory();
        let h = mem.u32(ST_BINK)?;
        self.do_frame_at(&mem, h)?;
        self.next_frame_at(&mut mem, h)?;
        cpu.eax = 0;
        mem.ret(cpu, 0)
    }

    /// `0x00156960` in Rust: the guest's quad and the movie's picture to
    /// the host set by [`set_display`], in place of the screen-quad batch
    /// (`0x00035b90`, `0x000363a0`), which never runs.
    pub(crate) fn draw_native(&self, cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx) -> R {
        use timeline::native::Ui;
        let mut mem = ctx.memory();
        let inputs = DrawInputs::read(&mem)?;
        match self.picture(inputs.handle) {
            None => refuse(Ui::MovieRefusedPicture, "no decoded picture"),
            Some(picture) => {
                let draw = MovieDraw {
                    quad: movie_quad(inputs.rect, inputs.flags, inputs.size),
                    record: MOVIE_RECORD,
                    picture,
                };
                match display() {
                    None => refuse(Ui::MovieRefusedHost, "no native display is installed"),
                    Some(host) => {
                        if host.show(&mut mem, &draw) {
                            Ui::MoviePlaced.count();
                        }
                    }
                }
            }
        }
        mem.ret(cpu, 0)
    }
}

/// Counts a draw the host never saw and logs its reason once.
fn refuse(counter: timeline::native::Ui, why: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    counter.count();
    if !LOGGED.swap(true, Ordering::Relaxed) {
        eprintln!("movies: a frame was not drawn ({why}); the native draw has no guest texture behind it");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_quads_halve_toward_zero_from_the_origin() {
        // 640 x 480 rectangle, a 600 x 360 movie: 20..620, 60..420.
        let q = movie_quad([0, 0, 480, 640], 0x1c6, [600, 360]);
        assert_eq!(q.corners[0], [20.0, 60.0, 0.0, 0.0]);
        assert_eq!(q.corners[1], [620.0, 60.0, 600.0, 0.0]);
        assert_eq!(q.corners[2], [620.0, 420.0, 600.0, 360.0]);
        assert_eq!(q.corners[3], [20.0, 420.0, 0.0, 360.0]);
        assert_eq!(q.argb, 0xffff_ffff);
        // From 0, not from the rectangle's corner; an odd difference
        // halves toward zero on both sides, a movie wider than the
        // rectangle goes negative.
        let q = movie_quad([10, 20, 491, 661], 0, [642, 480]);
        assert_eq!([q.corners[0][0], q.corners[2][0]], [0.0, 641.0], "(641 - 642) / 2 is 0");
        assert_eq!([q.corners[0][1], q.corners[2][1]], [0.0, 480.0]);
        let q = movie_quad([0, 0, 480, 640], 0, [700, 480]);
        assert_eq!(q.corners[0][0], -30.0);
    }

    #[test]
    fn a_stretched_movie_takes_the_rectangles_edges() {
        let q = movie_quad([8, 16, 472, 624], FLAG_STRETCH, [320, 240]);
        assert_eq!(q.corners[0], [16.0, 8.0, 0.0, 0.0]);
        assert_eq!(q.corners[2], [624.0, 472.0, 320.0, 240.0]);
    }

    #[test]
    fn spans_keep_to_sixteen_bits() {
        // right - left wraps as the guest's 16-bit subtraction does.
        // 32767 - -32768 is -1 in 16 bits: -1 / 2 is 0.
        let q = movie_quad([0, -32768, 0, 32767], 0, [2, 0]);
        assert_eq!([q.corners[0][0], q.corners[1][0]], [-1.0, 0.0]);
    }
}
