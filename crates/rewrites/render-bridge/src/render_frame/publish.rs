//! The publish (docs/graphics/retire-d3d-display.md, "The final display"):
//! the frame step's present `0x00013f10(0, 0)` made in Rust for a frame
//! whose every slot draws from the scene.
//!
//! The present with its first argument 0 asks no readback and jumps to
//! `0x000140e6` (**Code**). From there:
//!
//! | Guest | What | The publish |
//! |---|---|---|
//! | `0x00013d80` | with `[0x005093c8]` set, the CPU blit of an image into the rectangle at `0x005093c0` of the back buffer ([`super::blit`]) | [`Publish::blit`]: one overlay picture; not placed: kept, `publish_kept_border` |
//! | `0x003f9df0(1, 1)`, `Swap(0)`, `0x003f9df0(1, 1)` | the back buffer scale and the swap | [`Publish::present`] with the frame step's gamma ramp |
//! | `0x00014110..0x00014194` | `[0x00485acc]` against `[0x00467008]`, byte `[0x00485ad0]` against `[0x0046700c]`: when either differs, the TV encoder option, the flicker filter `0x003f8c70` and the latch | kept: `publish_kept_display` |
//! | `0x00014196..0x000141ae` | the present counter `0x00485aa0`, 64-bit, +1 | `tick_writes::present_counter` |
//!
//! The gamma ramp is the one the brightness update (`0x00012c71..
//! 0x00012d08`) last loaded into the miniport: device `[0x00407488]`,
//! miniport `+0x1c28`, ramp `+0x1dc + 0x300 * [+0x7e4]`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use engine::render_loop::tick_writes;

/// Bytes of a gamma ramp: 256 red, 256 green, then 256 blue.
pub const RAMP_BYTES: usize = 768;

/// The guest words the publish reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// `0x00013d80`'s image, rectangle and scale.
    pub blit: super::blit::Addrs,
    /// The display's TV option (a dword) and flicker byte.
    pub display: (u32, u32),
    /// What the present last sent of them.
    pub latched: (u32, u32),
    /// The D3D device pointer.
    pub device: u32,
    /// The present counter.
    pub ticks: tick_writes::Addrs,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        blit: super::blit::Addrs::HALO2,
        display: (0x0048_5acc, 0x0048_5ad0),
        latched: (0x0046_7008, 0x0046_700c),
        device: 0x0040_7488,
        ticks: tick_writes::Addrs::HALO2,
    };
}

/// The miniport in the device, its ramps and the selected one.
const MINIPORT: u32 = 0x1c28;
const RAMPS: u32 = 0x1dc;
const RAMP_STRIDE: u32 = 0x300;
const RAMP_SELECT: u32 = 0x7e4;

/// What the open frame's guest ops were filed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    /// No guest op: every slot draws from the scene.
    Scene,
    /// The first slot with a guest op, by its `render.scene` name.
    Carried(&'static str),
    /// A guest write while no slot was declared.
    Untracked,
}

/// The host's side of the publish: the device and the renderer.
pub trait Publish: Send + Sync {
    /// The open frame, after the device has run what game code wrote.
    fn open(&self, mem: &mut dyn GuestMemory) -> Open;
    /// Shows the frame from the scene with `gamma`, and counts its swap
    /// for the next vertical blank's record.
    fn present(&self, mem: &mut dyn GuestMemory, gamma: Option<&[u8; RAMP_BYTES]>) -> Result<(), String>;
    /// Draws `0x00013d80`'s blit over the frame before its present (one
    /// overlay picture, [`super::blit::overlay`]). `Err`: not placed, by
    /// cause (`publish_kept_border_<cause>`), and the guest's present
    /// draws it.
    fn blit(&self, _mem: &mut dyn GuestMemory, _blit: &super::blit::Blit) -> Result<(), &'static str> {
        Err("host")
    }
}

static INSTALLED: Mutex<Option<Arc<dyn Publish>>> = Mutex::new(None);

/// Registers the host's [`Publish`] (the D3D install).
pub fn set(publish: Arc<dyn Publish>) {
    *INSTALLED.lock().unwrap_or_else(|e| e.into_inner()) = Some(publish);
}

pub fn installed() -> Option<Arc<dyn Publish>> {
    INSTALLED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Why a frame keeps the guest's present.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// A slot with a guest op, by name.
    Slot(&'static str),
    Untracked,
    /// `0x00013d80` would write its image and the host did not place it,
    /// by cause: `host` (no overlay), `target` (the back buffer is not
    /// the bound target), `pass` (the device did not take the pass).
    Border(&'static str),
    /// The TV option or flicker byte changed.
    Display,
    /// No host registered a [`Publish`].
    Host,
    /// The host's present failed.
    Failed,
}

/// The frame step's present, decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Shown from the scene; the guest's present is skipped.
    Native,
    /// The guest's present runs.
    Kept(Kept),
}

impl Outcome {
    /// The `render.scene` counter it adds to.
    pub fn name(self) -> String {
        match self {
            Outcome::Native => "publish_native".to_owned(),
            Outcome::Kept(Kept::Slot(slot)) => format!("publish_kept_{slot}"),
            Outcome::Kept(Kept::Untracked) => "publish_kept_untracked".to_owned(),
            Outcome::Kept(Kept::Border(cause)) => format!("publish_kept_border_{cause}"),
            Outcome::Kept(Kept::Display) => "publish_kept_display".to_owned(),
            Outcome::Kept(Kept::Host) => "publish_kept_host".to_owned(),
            Outcome::Kept(Kept::Failed) => "publish_failed".to_owned(),
        }
    }
}

static COUNTS: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());
static FAILED_LOGGED: AtomicBool = AtomicBool::new(false);

fn count(outcome: Outcome) {
    count_name(&outcome.name());
}

fn count_name(name: &str) {
    let mut counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    match counts.iter_mut().find(|(n, _)| *n == name) {
        Some((_, v)) => *v += 1,
        None => counts.push((name.to_owned(), 1)),
    }
}

/// The publish's counters since the start (`render.scene`): only those
/// that have counted.
pub fn counters() -> Vec<(String, u64)> {
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The ramp the brightness update last loaded; `None` before one is (the
/// miniport's ramps are zero until then, and an all-zero ramp is black).
pub fn gamma(a: &Addrs, g: &dyn GuestMemory) -> Result<Option<Box<[u8; RAMP_BYTES]>>, MemFault> {
    let device = g.u32(a.device)?;
    if device == 0 {
        return Ok(None);
    }
    let miniport = device.wrapping_add(MINIPORT);
    let select = g.u32(miniport.wrapping_add(RAMP_SELECT))? & 1;
    let mut ramp = Box::new([0u8; RAMP_BYTES]);
    g.read(miniport.wrapping_add(RAMPS).wrapping_add(RAMP_STRIDE * select), &mut ramp[..])?;
    Ok(ramp.iter().any(|&b| b != 0).then_some(ramp))
}

/// The frame step's present: shown from the scene when `publish` says
/// every slot drew from it and the guest's present would do nothing the
/// scene does not, else kept. Either way counted by name.
pub fn present(a: &Addrs, publish: Option<&dyn Publish>, g: &mut dyn GuestMemory) -> Result<Outcome, MemFault> {
    let outcome = decide(a, publish, g)?;
    count(outcome);
    Ok(outcome)
}

fn decide(a: &Addrs, publish: Option<&dyn Publish>, g: &mut dyn GuestMemory) -> Result<Outcome, MemFault> {
    let Some(publish) = publish else { return Ok(Outcome::Kept(Kept::Host)) };
    match publish.open(g) {
        Open::Scene => {}
        Open::Carried(slot) => return Ok(Outcome::Kept(Kept::Slot(slot))),
        Open::Untracked => return Ok(Outcome::Kept(Kept::Untracked)),
    }
    if g.u32(a.display.0)? != g.u32(a.latched.0)? || u32::from(g.u8(a.display.1)?) != g.u32(a.latched.1)? {
        return Ok(Outcome::Kept(Kept::Display));
    }
    // The blit is the last write before the swap: placed after every
    // check that could keep the frame, so a kept frame draws it once.
    if let Some(blit) = super::blit::read(&a.blit, g)? {
        if let Err(cause) = publish.blit(g, &blit) {
            return Ok(Outcome::Kept(Kept::Border(cause)));
        }
        count_name("publish_blit_native");
    }
    let ramp = gamma(a, g)?;
    // The counter reads before the present, so a fault leaves the frame
    // to the guest.
    g.u32(a.ticks.present_counter.wrapping_add(4))?;
    if let Err(e) = publish.present(g, ramp.as_deref()) {
        if !FAILED_LOGGED.swap(true, Ordering::Relaxed) {
            eprintln!("render: the publish failed, the guest presents: {e}");
        }
        return Ok(Outcome::Kept(Kept::Failed));
    }
    tick_writes::present_counter(&a.ticks, g)?;
    Ok(Outcome::Native)
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const A: Addrs = Addrs::HALO2;
    const DEVICE: u32 = 0x0070_0000;

    /// A host that answers `open` and records each present's ramp.
    struct Host {
        open: Open,
        fail: bool,
        /// Whether it places a blit.
        blits: bool,
        presents: Mutex<Vec<Option<Vec<u8>>>>,
        placed: Mutex<Vec<super::super::blit::Blit>>,
    }

    impl Host {
        fn new(open: Open) -> Self {
            Host { open, fail: false, blits: false, presents: Mutex::new(Vec::new()), placed: Mutex::new(Vec::new()) }
        }
        fn presents(&self) -> Vec<Option<Vec<u8>>> {
            self.presents.lock().unwrap().clone()
        }
    }

    impl Publish for Host {
        fn open(&self, _: &mut dyn GuestMemory) -> Open {
            self.open
        }
        fn present(&self, _: &mut dyn GuestMemory, gamma: Option<&[u8; RAMP_BYTES]>) -> Result<(), String> {
            if self.fail {
                return Err("no surface".into());
            }
            self.presents.lock().unwrap().push(gamma.map(|r| r.to_vec()));
            Ok(())
        }
        fn blit(&self, _: &mut dyn GuestMemory, blit: &super::super::blit::Blit) -> Result<(), &'static str> {
            if !self.blits {
                return Err("pass");
            }
            self.placed.lock().unwrap().push(blit.clone());
            Ok(())
        }
    }

    const IMAGE: u32 = 0x0060_0000;

    /// What `0x00013d50` leaves for a 2 x 1 image at (4, 3).
    fn set_blit(g: &mut FakeGuest) {
        g.map(IMAGE, 0x1000).put(IMAGE, &[1, 3]);
        g.put_u32(A.blit.image, IMAGE).put_u32(A.blit.rect, 3 | 4 << 16).put_u32(A.blit.rect + 4, 4 | 6 << 16);
        g.put_u32(A.blit.scale, 0x55);
    }

    fn ramp(n: u8) -> Vec<u8> {
        (0..RAMP_BYTES).map(|i| (i as u8).wrapping_mul(3).wrapping_add(n)).collect()
    }

    /// A guest with the device, a loaded ramp in the miniport's second
    /// slot and the display words latched.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x0040_7000, 0x1000).map(0x0046_7000, 0x1000).map(0x0048_5000, 0x1000).map(0x0050_9000, 0x1000);
        g.map(DEVICE, 0x3000);
        g.put_u32(A.device, DEVICE);
        let miniport = DEVICE + MINIPORT;
        g.put(miniport + RAMPS, &ramp(1)).put(miniport + RAMPS + RAMP_STRIDE, &ramp(2));
        g.put_u32(miniport + RAMP_SELECT, 1);
        g.put_u32(A.display.0, 7).put_u32(A.latched.0, 7).put_u8(A.display.1, 1).put_u32(A.latched.1, 1);
        g.put_u32(A.ticks.present_counter, 41);
        g
    }

    fn counter(g: &FakeGuest) -> u64 {
        let lo = g.u32(A.ticks.present_counter).unwrap();
        let hi = g.u32(A.ticks.present_counter + 4).unwrap();
        u64::from(hi) << 32 | u64::from(lo)
    }

    #[test]
    fn a_frame_with_every_slot_from_the_scene_is_shown_from_it_with_the_ramp_and_counted() {
        let mut g = guest();
        let host = Host::new(Open::Scene);
        assert_eq!(present(&A, Some(&host), &mut g).unwrap(), Outcome::Native);
        assert_eq!(host.presents(), [Some(ramp(2))], "the selected ramp moves with the frame");
        assert_eq!(counter(&g), 42, "the present counter, as 0x000141ae leaves it");
        assert_eq!(g.u32(A.latched.0).unwrap(), 7, "nothing to send: the latch stays");
        assert!(counters().iter().any(|(n, v)| n == "publish_native" && *v >= 1));
    }

    #[test]
    fn one_carried_slot_keeps_the_guests_present_and_counts_it_by_name() {
        let mut g = guest();
        let host = Host::new(Open::Carried("overlay"));
        let outcome = present(&A, Some(&host), &mut g).unwrap();
        assert_eq!(outcome, Outcome::Kept(Kept::Slot("overlay")));
        assert_eq!(outcome.name(), "publish_kept_overlay");
        assert!(host.presents().is_empty(), "the scene does not present");
        assert_eq!(counter(&g), 41, "the guest's present adds to the counter");
        assert!(counters().iter().any(|(n, v)| n == "publish_kept_overlay" && *v >= 1));
        let host = Host::new(Open::Untracked);
        assert_eq!(present(&A, Some(&host), &mut g).unwrap().name(), "publish_kept_untracked");
    }

    #[test]
    fn what_the_scene_cannot_do_keeps_the_guests_present() {
        let mut g = guest();
        assert_eq!(present(&A, None, &mut g).unwrap(), Outcome::Kept(Kept::Host));
        let host = Host::new(Open::Scene);
        set_blit(&mut g);
        let outcome = present(&A, Some(&host), &mut g).unwrap();
        assert_eq!(outcome, Outcome::Kept(Kept::Border("pass")), "a host that does not place the blit");
        assert_eq!(outcome.name(), "publish_kept_border_pass");
        g.put_u32(A.blit.image, 0).put_u8(A.display.1, 0);
        assert_eq!(present(&A, Some(&host), &mut g).unwrap(), Outcome::Kept(Kept::Display), "the flicker byte changed");
        g.put_u8(A.display.1, 1).put_u32(A.display.0, 8);
        assert_eq!(present(&A, Some(&host), &mut g).unwrap(), Outcome::Kept(Kept::Display), "the TV option changed");
        g.put_u32(A.display.0, 7);
        let failing = Host { fail: true, ..Host::new(Open::Scene) };
        assert_eq!(present(&A, Some(&failing), &mut g).unwrap().name(), "publish_failed");
        assert!(host.presents().is_empty());
        assert_eq!(counter(&g), 41, "every kept frame leaves the counter to the guest");
    }

    #[test]
    fn the_blit_is_drawn_from_the_scene_and_the_frame_published() {
        let mut g = guest();
        set_blit(&mut g);
        let host = Host { blits: true, ..Host::new(Open::Scene) };
        assert_eq!(present(&A, Some(&host), &mut g).unwrap(), Outcome::Native);
        let placed = host.placed.lock().unwrap().clone();
        assert_eq!(placed.len(), 1, "one blit, placed before the present");
        assert_eq!((placed[0].at, placed[0].size), ([4, 3], [2, 1]));
        assert_eq!(placed[0].bgra, [[0x55; 4], [0xff; 4]].concat());
        assert_eq!(host.presents().len(), 1);
        assert!(counters().iter().any(|(n, v)| n == "publish_blit_native" && *v >= 1));
        // A frame kept for another reason leaves the blit to the guest.
        g.put_u8(A.display.1, 0);
        assert_eq!(present(&A, Some(&host), &mut g).unwrap(), Outcome::Kept(Kept::Display));
        assert_eq!(host.placed.lock().unwrap().len(), 1, "a kept frame's blit is the guest's alone");
    }

    #[test]
    fn the_ramp_is_the_selected_one_and_none_before_a_load() {
        let mut g = guest();
        assert_eq!(gamma(&A, &g).unwrap().map(|r| r.to_vec()), Some(ramp(2)));
        g.put_u32(DEVICE + MINIPORT + RAMP_SELECT, 0);
        assert_eq!(gamma(&A, &g).unwrap().map(|r| r.to_vec()), Some(ramp(1)));
        g.put(DEVICE + MINIPORT + RAMPS, &[0; RAMP_BYTES]);
        assert_eq!(gamma(&A, &g).unwrap(), None, "zero until the brightness update loads one");
        g.put_u32(A.device, 0);
        assert_eq!(gamma(&A, &g).unwrap(), None, "no device");
    }
}
