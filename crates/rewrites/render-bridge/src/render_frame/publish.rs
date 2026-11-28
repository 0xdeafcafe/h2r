//! The present `0x00013f10` in Rust (docs/graphics/retire-d3d-display.md,
//! "The final display"): what the guest's body leaves in guest memory, with
//! the swap made by the native scene's seal, so no frame enters the
//! guest's present (which calls `D3DDevice_Swap`).
//!
//! The present with its first argument 0 asks no readback and jumps to
//! `0x000140e6` (**Code**). From there:
//!
//! | Guest | What | In Rust |
//! |---|---|---|
//! | `0x00013d80` | with `[0x005093c8]` set, the CPU blit of an image into the rectangle at `0x005093c0` of the back buffer ([`super::blit`]) | the host's [`PlaceBlit`]: one overlay picture |
//! | `0x003f9df0(1, 1)`, `Swap(0)`, `0x003f9df0(1, 1)` | the back buffer scale and the swap | the scene's seal: [`crate::scene_step::seal`] after the frame step, [`crate::frame_wait::swapped`] after a frame loop's present |
//! | `0x00014110..0x0001412a` | `[0x00485acc]` against `[0x00467008]`, byte `[0x00485ad0]` against `[0x0046700c]` | the same test |
//! | `0x0001412c..0x00014163` | when the encoder latch `[0x00408640]` is clear or `[0x00408638]` differs: `AvSendTVEncoderOption(miniport, 0xb, option, 0)`, then the latch set to 1 and the option | the latch written; the host display has no TV encoder |
//! | `0x00014169..0x0001418d` | the flicker filter `0x003f8c70(flicker)`, then `[0x0046700c]` and `[0x00467008]` latched | the latches written; the host display has no flicker filter |
//! | `0x00014196..0x000141b7` | the present counter `0x00485aa0`, 64-bit, +1; EAX its high dword; `ret 8` | `tick_writes::present_counter`, EAX the same |
//!
//! A first argument with `+0x54` set (`0x00013f21..0x000140e4`) reads the
//! front buffer back into a bitmap through D3D surfaces: no frame loop or
//! the frame step asks it, and it is not ported ([`readback`]).
//!
//! The gamma ramp is the display's own: the view begin's brightness
//! update builds it from the game's brightness setting
//! ([`crate::display`]) and the present applies it when it copies a frame
//! to the window (`gfx::display_gamma`). The present reads no device.

use std::sync::OnceLock;

use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::tick_writes;

/// The guest words the present reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// `0x00013d80`'s image, rectangle and scale.
    pub blit: super::blit::Addrs,
    /// The display's TV option (a dword) and flicker byte.
    pub display: (u32, u32),
    /// What the present last sent of them.
    pub latched: (u32, u32),
    /// The TV encoder's latch: whether an option was sent, and which.
    pub encoder: (u32, u32),
    /// The present counter.
    pub ticks: tick_writes::Addrs,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        blit: super::blit::Addrs::HALO2,
        display: (0x0048_5acc, 0x0048_5ad0),
        latched: (0x0046_7008, 0x0046_700c),
        encoder: (0x0040_8640, 0x0040_8638),
        ticks: tick_writes::Addrs::HALO2,
    };
}

/// The host's placing of `0x00013d80`'s blit over the frame (one overlay
/// picture, [`super::blit::overlay`]). `Err`: not placed, by cause.
pub type PlaceBlit = fn(&mut dyn GuestMemory, &super::blit::Blit) -> Result<(), &'static str>;

static PLACE_BLIT: OnceLock<PlaceBlit> = OnceLock::new();

/// Registers the host's [`PlaceBlit`] (the overlay's install). Once.
pub fn set_blit(place: PlaceBlit) {
    let _ = PLACE_BLIT.set(place);
}

pub fn installed_blit() -> Option<PlaceBlit> {
    PLACE_BLIT.get().copied()
}

/// What one present wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presented {
    /// The guest's EAX on return: the counter's high dword.
    pub eax: u32,
    /// The blit was placed.
    pub blit: bool,
    /// The TV option or flicker byte changed and was latched.
    pub display: bool,
}

/// The present `0x00013f10(0, _)`: the blit placed with `place`, the
/// display latched, the counter moved. The swap is the caller's (the
/// scene's seal). A blit the host does not place is a gap.
pub fn present(a: &Addrs, place: Option<PlaceBlit>, g: &mut dyn GuestMemory) -> Result<Presented, MemFault> {
    let blit = match super::blit::read(&a.blit, g)? {
        Some(blit) => {
            let place = place.unwrap_or_else(|| unimplemented!("render: the present's blit 0x00013d80: no host places it"));
            if let Err(cause) = place(g, &blit) {
                unimplemented!("render: the present's blit 0x00013d80: not placed ({cause})");
            }
            true
        }
        None => false,
    };
    let display = latch(a, g)?;
    tick_writes::present_counter(&a.ticks, g)?;
    Ok(Presented { eax: g.u32(a.ticks.present_counter.wrapping_add(4))?, blit, display })
}

/// `0x00014110..0x0001418d`: a changed TV option or flicker byte latched
/// as the guest latches it once sent. Whether it changed.
fn latch(a: &Addrs, g: &mut dyn GuestMemory) -> Result<bool, MemFault> {
    let option = g.u32(a.display.0)?;
    let flicker = u32::from(g.u8(a.display.1)?);
    if g.u32(a.latched.0)? == option && g.u32(a.latched.1)? == flicker {
        return Ok(false);
    }
    if g.u32(a.encoder.0)? == 0 || g.u32(a.encoder.1)? != option {
        g.set_u32(a.encoder.0, 1)?;
        g.set_u32(a.encoder.1, option)?;
    }
    g.set_u32(a.latched.1, flicker)?;
    g.set_u32(a.latched.0, option)?;
    Ok(true)
}

/// The present's readback (`0x00013f21`): its first argument set with
/// `+0x54` set.
pub fn readback(g: &dyn GuestMemory, arg: u32) -> Result<bool, MemFault> {
    Ok(arg != 0 && g.u32(arg.wrapping_add(0x54))? != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use std::sync::Mutex;

    const A: Addrs = Addrs::HALO2;
    const IMAGE: u32 = 0x0060_0000;

    /// What `0x00013d50` leaves for a 2 x 1 image at (4, 3).
    fn set_blit(g: &mut FakeGuest) {
        g.map(IMAGE, 0x1000).put(IMAGE, &[1, 3]);
        g.put_u32(A.blit.image, IMAGE).put_u32(A.blit.rect, 3 | 4 << 16).put_u32(A.blit.rect + 4, 4 | 6 << 16);
        g.put_u32(A.blit.scale, 0x55);
    }

    /// A guest with the display words latched.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x0040_7000, 0x2000).map(0x0046_7000, 0x1000).map(0x0048_5000, 0x1000).map(0x0050_9000, 0x1000);
        g.put_u32(A.display.0, 7).put_u32(A.latched.0, 7).put_u8(A.display.1, 1).put_u32(A.latched.1, 1);
        g.put_u32(A.encoder.0, 1).put_u32(A.encoder.1, 7);
        g.put_u32(A.ticks.present_counter, 41);
        g
    }

    fn counter(g: &FakeGuest) -> u64 {
        let lo = g.u32(A.ticks.present_counter).unwrap();
        let hi = g.u32(A.ticks.present_counter + 4).unwrap();
        u64::from(hi) << 32 | u64::from(lo)
    }

    #[test]
    fn a_present_moves_the_counter_and_returns_its_high_dword() {
        let mut g = guest();
        let p = present(&A, None, &mut g).unwrap();
        assert_eq!(p, Presented { eax: 0, blit: false, display: false });
        assert_eq!(counter(&g), 42, "the present counter, as 0x000141ae leaves it");
        g.put_u32(A.ticks.present_counter, u32::MAX);
        assert_eq!(present(&A, None, &mut g).unwrap().eax, 1, "the carry, as the guest's adc makes it");
        assert_eq!(counter(&g), 1 << 32);
        assert_eq!(g.u32(A.latched.0).unwrap(), 7, "nothing changed: the latch stays");
    }

    #[test]
    fn a_changed_display_is_latched_as_the_guest_latches_it() {
        let mut g = guest();
        g.put_u8(A.display.1, 0);
        assert!(present(&A, None, &mut g).unwrap().display, "the flicker byte changed");
        assert_eq!(g.u32(A.latched.1).unwrap(), 0);
        assert_eq!((g.u32(A.encoder.0).unwrap(), g.u32(A.encoder.1).unwrap()), (1, 7), "the option was sent already");
        assert!(!present(&A, None, &mut g).unwrap().display, "latched: the next present sends nothing");
        g.put_u32(A.display.0, 8);
        assert!(present(&A, None, &mut g).unwrap().display, "the TV option changed");
        assert_eq!((g.u32(A.encoder.0).unwrap(), g.u32(A.encoder.1).unwrap()), (1, 8));
        assert_eq!(g.u32(A.latched.0).unwrap(), 8);
        g.put_u32(A.encoder.0, 0).put_u32(A.latched.0, 0);
        present(&A, None, &mut g).unwrap();
        assert_eq!(g.u32(A.encoder.0).unwrap(), 1, "an encoder never sent to is sent to");
        assert_eq!(counter(&g), 45, "every present counts, latched or not");
    }

    static PLACED: Mutex<Vec<super::super::blit::Blit>> = Mutex::new(Vec::new());

    fn place(_: &mut dyn GuestMemory, blit: &super::super::blit::Blit) -> Result<(), &'static str> {
        PLACED.lock().unwrap().push(blit.clone());
        Ok(())
    }

    fn refuse(_: &mut dyn GuestMemory, _: &super::super::blit::Blit) -> Result<(), &'static str> {
        Err("pass")
    }

    #[test]
    fn the_blit_is_placed_by_the_host() {
        let mut g = guest();
        set_blit(&mut g);
        assert!(present(&A, Some(place), &mut g).unwrap().blit);
        let placed = PLACED.lock().unwrap().clone();
        assert_eq!(placed.len(), 1, "one blit a present");
        assert_eq!((placed[0].at, placed[0].size), ([4, 3], [2, 1]));
        assert_eq!(placed[0].bgra, [[0x55; 4], [0xff; 4]].concat());
        assert_eq!(counter(&g), 42);
    }

    #[test]
    #[should_panic(expected = "not placed (pass)")]
    fn a_blit_the_host_does_not_place_is_a_named_gap() {
        let mut g = guest();
        set_blit(&mut g);
        let _ = present(&A, Some(refuse), &mut g);
    }

    #[test]
    #[should_panic(expected = "no host places it")]
    fn a_blit_with_no_host_is_a_named_gap() {
        let mut g = guest();
        set_blit(&mut g);
        let _ = present(&A, None, &mut g);
    }

    #[test]
    fn a_readback_is_its_first_argument_with_its_bitmap_set() {
        let mut g = guest();
        g.map(0x0060_0000, 0x1000);
        assert!(!readback(&g, 0).unwrap());
        assert!(!readback(&g, 0x0060_0000).unwrap(), "+0x54 clear: the guest jumps to 0x000140e6");
        g.put_u32(0x0060_0054, 1);
        assert!(readback(&g, 0x0060_0000).unwrap());
    }
}
