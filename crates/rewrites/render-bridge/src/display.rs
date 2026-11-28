//! The display pieces the retail code kept in the D3D device, in Rust
//! (docs/graphics/d3d-device.md, "Brightness and gamma"): the view begin
//! `0x00012a50` with its brightness gamma ramp, and the vblank install
//! `0x0012b070`.
//!
//! **The view begin** (`RENDER_VIEW_BEGIN`, EAX = a 0x18-byte record; from
//! `render_views` and the frames outside a game view):
//!
//! | Guest | What | Here |
//! |---|---|---|
//! | `0x12a50..0x12af4` | the record to `0x485898..0x4858ac`; the clip range `0x485adc`/`0x485ae0` from `0x485ad8`/`0x485ad4` | [`begin_words`] |
//! | `0x12af4..0x12b6e` | `0x485ad4`/`0x485ad8` from the view's setting `0x5093e0` (`+0x190`, `+0x194`) or `0x467014`/`0x467018` | [`begin_words`] |
//! | `0x12b6e..0x12b7d` | `0x206a0`, `0x12dd80`, `0x12c600`, `0x1de50` | the same calls |
//! | `0x12b82..0x12ba3` | with `[0x5093bc]` and `[0x485aa0]` bit 0: `0x14980`, `0x14600`, the latch cleared | the same calls |
//! | `0x12baa..0x12bb1` | `[0x4b8478] = 0`, `SetRenderState_Dxt1NoiseEnable(0)` | the same call |
//! | `0x12bb6..0x12d0d` | the brightness: the setting at the view's `+0x3f8`, faded toward `0x4e673c` by `0x4e6738` at 640 x 480, then the gamma ramp `(i / 255) ^ brightness * 255` for each channel | [`brightness`], [`ramp`] |
//! | `0x12d1d..0x12d39` | the four latch bytes, `0x22850` | the same |
//!
//! The ramp is the display's own ([`frame_types::display`]): the retail code
//! copied it into the device's miniport and loaded it through `0x3ff5c6`;
//! here the present applies it when it copies a frame to the window
//! (`gfx::display_gamma`), so no miniport word is read or written.
//!
//! **The vblank install** (`VBLANK_INSTALL`, from `game_main` through
//! `0x12b690`): the retail code stored the vblank callback `0x12b2a0` at
//! device `+0x1db8`. The callback is [`vblank_callback`] here, which the
//! frame wait and the vblank source read ([`crate::frame_wait`]).

use std::sync::atomic::{AtomicU32, Ordering};

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

/// The feature id.
pub const ID: &str = "display";

/// The vblank callback the game installed (`0x12b2a0`), 0 before.
static CALLBACK: AtomicU32 = AtomicU32::new(0);

/// The guest function the vblank source calls, 0 before the install.
pub fn vblank_callback() -> u32 {
    CALLBACK.load(Ordering::Relaxed)
}

/// The vblank callback the install sets.
pub const VBLANK_CALLBACK: u32 = 0x0012_b2a0;

/// The guest words the view begin reads and writes.
mod at {
    /// The record's six words are copied here.
    pub const RECORD_COPY: u32 = 0x0048_5898;
    pub const NEAR: u32 = 0x0048_5ad4;
    pub const FAR: u32 = 0x0048_5ad8;
    pub const RANGE_NEAR: u32 = 0x0048_5adc;
    pub const RANGE_FAR: u32 = 0x0048_5ae0;
    pub const DEFAULT_NEAR: u32 = 0x0046_7014;
    pub const DEFAULT_FAR: u32 = 0x0046_7018;
    /// The view's setting block (a pointer) and its two clip words.
    pub const SETTING: u32 = 0x0050_93e0;
    pub const SETTING_NEAR: u32 = 0x190;
    pub const SETTING_FAR: u32 = 0x194;
    /// The brightness within the setting block.
    pub const SETTING_BRIGHTNESS: u32 = 0x3f8;
    pub const RESET_LATCH: u32 = 0x0050_93bc;
    pub const PRESENTS: u32 = 0x0048_5aa0;
    pub const NOISE_LATCH: u32 = 0x004b_8478;
    pub const FADE: u32 = 0x004e_6738;
    pub const FADE_TO: u32 = 0x004e_673c;
    /// The size of the display: `[0x485648]` and `[0x48564a]` (the origin,
    /// both 0), `[0x48564c]` the height, `[0x48564e]` the width.
    pub const ORIGIN: [u32; 2] = [0x0048_5648, 0x0048_564a];
    pub const HEIGHT: u32 = 0x0048_564c;
    pub const WIDTH: u32 = 0x0048_564e;
    /// Players in the frame; the fade is for one or none.
    pub const PLAYERS: u32 = 0x004b_a04c;
    /// The brightness the ramp was last built for.
    pub const LAST: u32 = 0x0047_fd10;
    /// The latch bytes the begin ends with: cleared, cleared, set, set.
    pub const END_CLEAR: [u32; 2] = [0x0048_5a45, 0x0048_5a46];
    pub const END_SET: [u32; 2] = [0x0048_5af1, 0x0048_5af0];
}

/// Guest functions the begin calls, in order.
mod calls {
    pub const BEFORE: [u32; 4] = [0x0002_06a0, 0x0012_dd80, 0x0012_c600, 0x0001_de50];
    pub const RELEASE: u32 = 0x0001_4980;
    pub const RESET: u32 = 0x0001_4600;
    /// `D3DDevice_SetRenderState_Dxt1NoiseEnable`, stdcall(value).
    pub const NOISE: u32 = 0x003f_57a0;
    /// The lightmap cycle's step.
    pub const CYCLE: u32 = 0x0002_2850;
}

/// The largest depth word: 2^24 - 1.
const DEPTH_MAX: f32 = 16_777_215.0;
/// `1 / 255` as the image stores it (`0x45dbb4`, a float).
const ONE_OVER_255: f32 = f32::from_bits(0x3b80_8081);
/// 255 as the image stores it (`0x45dc38`).
const LEVELS: f32 = 255.0;
/// Below this the brightness is reset to 1 (`0x45dc68`).
const MIN_BRIGHTNESS: f32 = 0.1;

/// `minss`/`maxss` as the begin clamps: below 0 is 0, above the maximum is
/// the maximum, and NaN passes.
fn clamp_depth(x: f32) -> f32 {
    if 0.0 > x {
        0.0
    } else if x > DEPTH_MAX {
        DEPTH_MAX
    } else {
        x
    }
}

/// `0x12a50..0x12b6e`: the record copied, the clip range and the two clip
/// words set.
pub fn begin_words(m: &mut dyn GuestMemory, record: u32) -> Result<(), MemFault> {
    for i in 0..6 {
        let word = m.u32(record.wrapping_add(4 * i))?;
        m.set_u32(at::RECORD_COPY + 4 * i, word)?;
    }
    let (near, far) = (m.f32(at::NEAR)?, m.f32(at::FAR)?);
    let inverse = 1.0f32 / far;
    m.set_f32(at::RANGE_NEAR, clamp_depth(inverse * near * DEPTH_MAX))?;
    m.set_f32(at::RANGE_FAR, clamp_depth(inverse * far * DEPTH_MAX))?;
    // The view's own clip words when it has them and they are positive.
    let setting = m.u32(at::SETTING)?;
    let mut new_near = m.f32(at::DEFAULT_NEAR)?;
    let mut new_far = m.f32(at::DEFAULT_FAR)?;
    if setting != 0 {
        let near = m.f32(setting.wrapping_add(at::SETTING_NEAR))?;
        if near > 0.0 {
            new_near = near;
        }
        let far = m.f32(setting.wrapping_add(at::SETTING_FAR))?;
        if far > 0.0 {
            new_far = far;
        }
    }
    m.set_f32(at::NEAR, new_near)?;
    m.set_f32(at::FAR, new_far)?;
    Ok(())
}

/// What `0x12bb6..0x12c58` decides the brightness is: the setting, faded
/// toward `fade_to` by `fade` (when `faded`: a fade in progress at the
/// 640 x 480 display of one player), and 1 when it is below 0.1.
pub fn brightness(setting: f32, fade: f32, fade_to: f32, faded: bool) -> f32 {
    let mut b = setting;
    if fade > 0.0 && faded {
        let t = if fade > 1.0 { 1.0 } else { fade };
        b = (fade_to - b) * t + b;
    }
    if MIN_BRIGHTNESS > b {
        b = 1.0;
    }
    b
}

/// The gamma ramp for `brightness`: for each channel level `i`,
/// `trunc((i / 255) ^ brightness * 255)` (the retail loop's `pow` and
/// `_ftol`, in 53-bit precision). 256 red, 256 green, 256 blue.
pub fn ramp(brightness: f32) -> [u8; frame_types::display::RAMP_BYTES] {
    let mut out = [0u8; frame_types::display::RAMP_BYTES];
    let exponent = f64::from(brightness);
    for i in 0..256usize {
        let base = (i as f64) * f64::from(ONE_OVER_255);
        let level = base.powf(exponent) * f64::from(LEVELS);
        let byte = (level as i64) as u8;
        out[i] = byte;
        out[256 + i] = byte;
        out[512 + i] = byte;
    }
    out
}

/// `0x12bb6..0x12d0d`: the brightness, and when it differs from the one
/// the ramp was last built for, the new ramp as the display's.
fn update_brightness(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let setting = m.u32(at::SETTING)?;
    let setting = m.f32(setting.wrapping_add(at::SETTING_BRIGHTNESS))?;
    let fade = m.f32(at::FADE)?;
    let fade_to = m.f32(at::FADE_TO)?;
    let faded = m.u16(at::ORIGIN[1])? == 0
        && m.u16(at::ORIGIN[0])? == 0
        && m.u16(at::WIDTH)? == 0x280
        && m.u16(at::HEIGHT)? == 0x1e0
        && (m.u32(at::PLAYERS)? as i32) <= 1;
    let b = brightness(setting, fade, fade_to, faded);
    if m.f32(at::LAST)? != b {
        frame_types::display::set_ramp(ramp(b));
        m.set_f32(at::LAST, b)?;
    }
    Ok(())
}

/// The view begin's body (`0x00012a50`, EAX = the record).
pub async fn view_begin(cx: Cx) -> Result<Ret, EntryError> {
    let record = cx.entry().eax;
    cx.write(|m| begin_words(m, record))?;
    for target in calls::BEFORE {
        cx.call(GuestCall::new(target)).await?;
    }
    let reset = cx.read(|m| Ok::<_, MemFault>(m.u8(at::RESET_LATCH)? != 0 && m.u32(at::PRESENTS)? & 1 != 0))?;
    if reset {
        cx.call(GuestCall::new(calls::RELEASE)).await?;
        cx.call(GuestCall::new(calls::RESET)).await?;
        cx.write(|m| m.set_u8(at::RESET_LATCH, 0))?;
    }
    cx.write(|m| m.set_u32(at::NOISE_LATCH, 0))?;
    cx.call(GuestCall::new(calls::NOISE).args([0])).await?;
    cx.write(update_brightness)?;
    cx.write(|m| {
        for a in at::END_CLEAR {
            m.set_u8(a, 0)?;
        }
        for a in at::END_SET {
            m.set_u8(a, 1)?;
        }
        Ok::<_, MemFault>(())
    })?;
    let cycle = cx.call(GuestCall::new(calls::CYCLE)).await?;
    Ok(Ret::new(cycle.eax, 0))
}

/// The guest words the vblank install zeroes and reads.
mod install_at {
    /// `[[0x411690]]`: the clock the install reads (`0x3314b0`).
    pub const CLOCK: u32 = 0x0041_1690;
    pub const START: u32 = 0x004e_6390;
    pub const ZERO_WORDS: [u32; 3] = [0x004e_6394, 0x004e_6398, 0x004e_639c];
    /// Seven unaligned dwords from `0x4e6402`, step 4.
    pub const RING: u32 = 0x004e_6402;
    pub const RING_WORDS: u32 = 7;
    /// The ring's two words: the index (`0x4e6400`) and `0x4e641e`.
    pub const RING_INDEX: u32 = 0x004e_6400;
    pub const RING_TAIL: u32 = 0x004e_641e;
}

/// The vblank install's body (`0x0012b070`): the clock read into
/// `0x4e6390`, the frame wait's words zeroed, the callback set.
pub async fn vblank_install(cx: Cx) -> Result<Ret, EntryError> {
    cx.write(|m| {
        let clock = m.u32(install_at::CLOCK)?;
        let now = m.u32(clock)?;
        m.set_u32(install_at::START, now)?;
        for a in install_at::ZERO_WORDS {
            m.set_u32(a, 0)?;
        }
        for i in 0..install_at::RING_WORDS {
            m.set_u32(install_at::RING + 4 * i, 0)?;
        }
        m.set_u16(install_at::RING_INDEX, 0)?;
        m.set_u16(install_at::RING_TAIL, 0)?;
        Ok::<_, MemFault>(())
    })?;
    CALLBACK.store(VBLANK_CALLBACK, Ordering::Relaxed);
    Ok(Ret::new(0, 0))
}

/// The display persist (`0x0022376b`, stdcall(mode)): the loading state
/// `0x12bf40`, `0x6cb60` unless `mode` is 4, and the retail
/// `D3DDevice_PersistDisplay` call, which has no display to keep here.
pub async fn display_persist(cx: Cx) -> Result<Ret, EntryError> {
    let mode = cx.arg(0)?;
    cx.call(GuestCall::new(0x0012_bf40)).await?;
    if mode != 4 {
        cx.call(GuestCall::new(0x0006_cb60)).await?;
    }
    Ok(Ret::new(0, 4))
}

/// The feature: both bodies in place of the retail ones.
pub struct Display;

impl Feature for Display {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The view begin with its brightness gamma ramp, and the vblank install and the display persist, without the D3D device")
            .doc("docs/graphics/d3d-device.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The callback and the ramp are host state outside guest RAM: a
        // snapshot keeps them.
        b.snapshot(SnapshotPolicy::section(save, |bytes, _| load(bytes)));
        b.raw().implement_async(&points::RENDER_VIEW_BEGIN, "display view begin", |cx| -> EntryFuture {
            Box::pin(view_begin(cx))
        });
        b.raw().implement_async(&points::VBLANK_INSTALL, "display vblank install", |cx| -> EntryFuture {
            Box::pin(vblank_install(cx))
        });
        b.raw().implement_async(&points::DISPLAY_PERSIST, "display persist", |cx| -> EntryFuture {
            Box::pin(display_persist(cx))
        });
        Ok(())
    }
}

/// The snapshot section: the callback, then 0 or 1 and the ramp.
fn save() -> Vec<u8> {
    let mut out = vblank_callback().to_le_bytes().to_vec();
    match frame_types::display::ramp() {
        Some(ramp) => {
            out.push(1);
            out.extend_from_slice(&ramp);
        }
        None => out.push(0),
    }
    out
}

fn load(bytes: &[u8]) -> Result<(), String> {
    let (callback, rest) = bytes.split_first_chunk::<4>().ok_or("display: no callback word")?;
    let (&present, ramp) = rest.split_first().ok_or("display: no ramp flag")?;
    CALLBACK.store(u32::from_le_bytes(*callback), Ordering::Relaxed);
    match present {
        0 => frame_types::display::clear_ramp(),
        1 => {
            let ramp: [u8; frame_types::display::RAMP_BYTES] =
                ramp.try_into().map_err(|_| "display: the ramp is not 768 bytes".to_string())?;
            frame_types::display::set_ramp(ramp);
        }
        n => return Err(format!("display: ramp flag {n}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{self, Regs};
    use guest_layout::testing::FakeGuest;

    /// The callback and the ramp are process-wide: the tests that touch
    /// them take turns.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    const STACK: u32 = 0x0030_0000;
    const RECORD: u32 = 0x0060_0000;
    const SETTINGS: u32 = 0x0060_1000;

    /// The image's words the begin touches, mapped zeroed.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x1000, 0x2000).map(0x0040_0000, 0x0020_0000).map(RECORD, 0x2000);
        g.put_u32(at::SETTING, SETTINGS);
        g.put_f32(at::DEFAULT_NEAR, 0.0625).put_f32(at::DEFAULT_FAR, 1.0);
        g.put_f32(at::NEAR, 0.5).put_f32(at::FAR, 4.0);
        g.put_f32(at::LAST, 1.0);
        g.put_f32(SETTINGS + at::SETTING_BRIGHTNESS, 1.0);
        g
    }

    #[test]
    fn the_identity_brightness_makes_the_identity_ramp() {
        let r = ramp(1.0);
        assert!(frame_types::display::is_identity(&r));
        for c in 0..3 {
            assert_eq!(r[256 * c], 0);
            assert_eq!(r[256 * c + 255], 255);
        }
    }

    #[test]
    fn a_higher_brightness_darkens_the_middle_and_keeps_the_ends() {
        let r = ramp(2.0);
        assert_eq!((r[0], r[255]), (0, 255));
        // (128/255)^2 * 255 = 64.25
        assert_eq!(r[128], 64);
        assert_eq!(r[0..256], r[256..512]);
        assert_eq!(r[0..256], r[512..768]);
    }

    #[test]
    fn the_brightness_fades_and_resets() {
        // No fade, or a fade on a display it does not cover: the setting.
        assert_eq!(brightness(0.8, 0.5, 2.0, false), 0.8);
        assert_eq!(brightness(0.8, 0.0, 2.0, true), 0.8);
        // Half way to 2.0 from 1.0.
        assert_eq!(brightness(1.0, 0.5, 2.0, true), 1.5);
        // The fade saturates at 1.
        assert_eq!(brightness(1.0, 3.0, 2.0, true), 2.0);
        // Under 0.1 the brightness is 1.
        assert_eq!(brightness(0.05, 0.0, 0.0, false), 1.0);
    }

    #[test]
    fn the_depth_clamp_passes_nan() {
        assert_eq!(clamp_depth(-1.0), 0.0);
        assert_eq!(clamp_depth(2.0e9), DEPTH_MAX);
        assert_eq!(clamp_depth(5.0), 5.0);
        assert!(clamp_depth(f32::NAN).is_nan());
    }

    #[test]
    fn the_begin_copies_the_record_and_sets_the_clip_range() {
        let mut g = guest();
        for i in 0..6 {
            g.put_u32(RECORD + 4 * i, 0x100 + i);
        }
        g.put_f32(SETTINGS + at::SETTING_NEAR, 0.25).put_f32(SETTINGS + at::SETTING_FAR, 0.0);
        begin_words(&mut g, RECORD).unwrap();
        for i in 0..6 {
            assert_eq!(g.u32(at::RECORD_COPY + 4 * i).unwrap(), 0x100 + i);
        }
        // (1 / 4) * 0.5 and (1 / 4) * 4 of 2^24 - 1: the second is the maximum.
        assert_eq!(g.f32(at::RANGE_NEAR).unwrap(), 0.125 * DEPTH_MAX);
        assert_eq!(g.f32(at::RANGE_FAR).unwrap(), DEPTH_MAX);
        // The setting's near is positive and used; its far is not, so the default.
        assert_eq!(g.f32(at::NEAR).unwrap(), 0.25);
        assert_eq!(g.f32(at::FAR).unwrap(), 1.0);
    }

    #[test]
    fn the_begin_runs_its_calls_in_order_and_builds_the_ramp_once() {
        let _turn = serial();
        frame_types::display::clear_ramp();
        let mut g = guest();
        g.put_f32(SETTINGS + at::SETTING_BRIGHTNESS, 2.0);
        g.put_u8(at::RESET_LATCH, 1).put_u32(at::PRESENTS, 1);
        let f = |cx: Cx| -> EntryFuture { Box::pin(view_begin(cx)) };
        let regs = Regs { eax: RECORD, esp: STACK, ..Default::default() };
        let run = entry::testing::run(&f, g, regs, |_, _| Regs { eax: 7, ..Default::default() });
        assert_eq!(run.result, Ok(Ret::new(7, 0)));
        let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            targets,
            [0x206a0, 0x12dd80, 0x12c600, 0x1de50, calls::RELEASE, calls::RESET, calls::NOISE, calls::CYCLE]
        );
        assert_eq!(run.calls[6].args, [0]);
        assert_eq!(run.guest.u8(at::RESET_LATCH).unwrap(), 0);
        assert_eq!(run.guest.f32(at::LAST).unwrap(), 2.0);
        assert_eq!(frame_types::display::gamma(), Some(ramp(2.0)));
        assert_eq!((run.guest.u8(0x0048_5af0).unwrap(), run.guest.u8(0x0048_5a45).unwrap()), (1, 0));
        frame_types::display::clear_ramp();
    }

    #[test]
    fn the_install_zeroes_the_wait_and_sets_the_callback() {
        let _turn = serial();
        let mut g = guest();
        g.put_u32(install_at::CLOCK, 0x0041_2000).put_u32(0x0041_2000, 99);
        for a in install_at::ZERO_WORDS {
            g.put_u32(a, 5);
        }
        g.put_u32(install_at::RING + 8, 5).put_u32(install_at::RING_INDEX, 3);
        let f = |cx: Cx| -> EntryFuture { Box::pin(vblank_install(cx)) };
        let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(run.guest.u32(install_at::START).unwrap(), 99);
        assert_eq!(run.guest.u32(install_at::ZERO_WORDS[1]).unwrap(), 0);
        assert_eq!(run.guest.u32(install_at::RING + 8).unwrap(), 0);
        assert_eq!(run.guest.u16(install_at::RING_INDEX).unwrap(), 0);
        assert_eq!(vblank_callback(), VBLANK_CALLBACK);
    }

    #[test]
    fn the_persist_skips_one_call_for_mode_4() {
        let f = |cx: Cx| -> EntryFuture { Box::pin(display_persist(cx)) };
        for (mode, want) in [(4u32, vec![0x12bf40]), (1, vec![0x12bf40, 0x6cb60])] {
            let mut g = guest();
            g.put_u32(STACK + 4, mode);
            let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs::default());
            assert_eq!(run.result, Ok(Ret::new(0, 4)));
            assert_eq!(run.calls.iter().map(|c| c.target).collect::<Vec<_>>(), want);
        }
    }

    #[test]
    fn a_snapshot_keeps_the_callback_and_the_ramp() {
        let _turn = serial();
        CALLBACK.store(0x1234, Ordering::Relaxed);
        frame_types::display::set_ramp(ramp(2.0));
        let bytes = save();
        CALLBACK.store(0, Ordering::Relaxed);
        frame_types::display::clear_ramp();
        load(&bytes).unwrap();
        assert_eq!(vblank_callback(), 0x1234);
        assert_eq!(frame_types::display::ramp(), Some(ramp(2.0)));
        assert!(load(&bytes[..3]).is_err());
        frame_types::display::clear_ramp();
        CALLBACK.store(0, Ordering::Relaxed);
    }
}
