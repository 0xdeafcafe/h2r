//! The fade `0x00040890` made in Rust (native-only plan, `nor-10c`;
//! docs/graphics/native-menus-overlay.md, "The fade in Rust"). The guest
//! body never runs.
//!
//! | Guest | Rust |
//! |---|---|
//! | the body's mode test (`[0x00485870]` 0: nothing drawn) | the same test, then `ret` |
//! | the two packed colours (`0x00131e50`), the blend and its constant, the combiner words written to the PSDEF at `0x00484f68`, the fan `0x00040d86` | [`read`], [`psdef`] written to `0x00484f68` as the body writes it, and one [`frame_scene::OverlayKind::Fill`] in slot `Fade` ([`plan`]) appended to the scene's overlay |
//!
//! The body's render-state and binding calls are not made: the device and
//! the game's state cache (`0x004b81fc`, D3D's mirror `0x00407498`) both
//! keep their values, so they stay in step. The PSDEF is the game's own
//! word block, which a bar with no image draws with (`0x00052857`), so the
//! cut writes it. A fade the cut cannot make panics, named
//! (`fade: <cause>`).

use engine::memory::GuestMemory;
use frame_scene::{ClipRect, Overlay, OverlayBlend, OverlayItem, Program, ViewSlot};
use hooks::Call;
use runtime::abi::TransferAbi;
use timeline::native::Ui;

/// The fade's mode, 0 (none) to 6.
pub const MODE: u32 = 0x0048_5870;
/// The fraction every channel is multiplied by.
pub const FRACTION: u32 = 0x0048_5874;
/// The colour: alpha, red, green, blue (floats).
pub const COLOUR: u32 = 0x0048_5878;
/// The 1.0 the complement subtracts from (`0x000408f3`).
pub const ONE: u32 = 0x0045_dbc0;
/// The pack's channel scale, 255.0 (`0x00131e53`).
pub const BYTE: u32 = 0x0045_dc38;
/// The view's rectangle, four `i16`: top, left, bottom, right.
pub const WINDOW: u32 = 0x0048_5648;
/// The PSDEF the body fills (`0x00040d11`).
pub const PSDEF: u32 = 0x0048_4f68;
pub const PSDEF_WORDS: usize = 60;
/// The highest mode the jump table `0x00040dc0` names.
pub const MODES: u32 = 6;

/// What one fade call draws, from the words the body reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fade {
    /// 1 to 6.
    pub mode: u32,
    /// The colour times the fraction, packed ARGB (`0x00040940`).
    pub colour: u32,
    /// Alpha as the colour's, each colour channel one minus its own,
    /// times the fraction, packed (`0x0004094c`).
    pub complement: u32,
    /// The view, guest pixels.
    pub rect: ClipRect,
}

/// `0x00131e50`: each channel times `scale` on the x87 (exact in `f64`),
/// stored with `fistp` (to nearest, ties to even; out of range is the
/// integer indefinite) and joined with no mask, alpha at the top.
pub fn pack(channels: [f32; 4], scale: f32) -> u32 {
    let int = |v: f32| {
        let x = (f64::from(v) * f64::from(scale)).round_ties_even();
        if (-2_147_483_648.0..=2_147_483_647.0).contains(&x) {
            x as i32 as u32
        } else {
            0x8000_0000
        }
    };
    let [a, r, g, b] = channels.map(int);
    b | (g << 8) | (r << 16) | (a << 24)
}

/// The fade the body would draw: `None` for mode 0, which draws nothing.
/// `Err` names what the cut does not make.
pub fn read(mem: &dyn GuestMemory) -> Result<Option<Fade>, &'static str> {
    let word = |a| mem.u32(a).map_err(|_| "unread");
    let mode = word(MODE)?;
    if mode == 0 {
        return Ok(None);
    }
    if mode > MODES {
        return Err("mode");
    }
    let f = |a| word(a).map(f32::from_bits);
    let (fraction, one, scale) = (f(FRACTION)?, f(ONE)?, f(BYTE)?);
    let c = [f(COLOUR)?, f(COLOUR + 4)?, f(COLOUR + 8)?, f(COLOUR + 12)?];
    let lit = c.map(|v| v * fraction);
    let complement = [
        lit[0],
        (one - c[1]) * fraction,
        (one - c[2]) * fraction,
        (one - c[3]) * fraction,
    ];
    let half = |k: u32| word(WINDOW + 4 * (k / 2)).map(|w| (w >> (16 * (k % 2))) as u16 as i16);
    let [top, left, bottom, right] = [half(0)?, half(1)?, half(2)?, half(3)?];
    // The body's own 16-bit differences (`0x00040d65`, `0x00040d73`).
    let width = right.wrapping_sub(left);
    let height = bottom.wrapping_sub(top);
    if width <= 0 || height <= 0 || left < 0 || top < 0 {
        return Err("rect");
    }
    Ok(Some(Fade {
        mode,
        colour: pack(lit, scale),
        complement: pack(complement, scale),
        rect: ClipRect {
            x: left as u32,
            y: top as u32,
            width: width as u32,
            height: height as u32,
        },
    }))
}

/// The blend each case sets (`D3DBLEND` source and dest, `D3DBLENDOP`),
/// and whether its factors name the constant colour.
fn blend(mode: u32) -> ((u32, u32, u32), bool) {
    match mode {
        1 => ((1, 0x303, 0x8006), false),
        2 => ((1, 1, 0x800b), false),
        3 => ((0x307, 0x8002, 0x8008), true),
        4 => ((0x307, 0x8002, 0x8007), true),
        5 => ((0x307, 0x8002, 0x8006), true),
        _ => ((1, 0x8002, 0x8006), true),
    }
}

impl Fade {
    /// The packed `BLENDCOLOR` (modes 3 to 6) and the combiner's constant:
    /// the colour, the complement for mode 6.
    fn constant(&self) -> u32 {
        if self.mode == 6 {
            self.complement
        } else {
            self.colour
        }
    }
}

/// The PSDEF words the body writes (`0x00040d06`..`0x00040d56`): the block
/// zeroed, then stage 0's alpha and colour inputs, its outputs to spare 0,
/// the constant, one stage and the final combiner reading spare 0.
pub fn psdef(fade: &Fade) -> [u32; PSDEF_WORDS] {
    let (alpha, rgb) = match fade.mode {
        1 | 2 => (0x1120_0000, 0x0120_0000),
        3 => (0, 0x0120_1140),
        4 => (0, 0x0120_1120),
        5 => (0, 0x0141_1120),
        _ => (0, 0x1120_0000),
    };
    let mut w = [0; PSDEF_WORDS];
    w[0] = alpha;
    w[8] = 0xc;
    w[9] = 0x1c00;
    w[10] = fade.constant();
    w[26] = 0xc00;
    w[34] = rgb;
    w[45] = 0xc00;
    w[53] = 1;
    w
}

/// The fade as one overlay item over a target `size`: a `Fill` of the
/// view in slot `Fade`, drawn by the PSDEF's combiner with the case's
/// blend and constant, colour channels written, alpha not (`0x00040971`).
pub fn plan(fade: &Fade, size: [u32; 2]) -> Result<Overlay, &'static str> {
    let ((source, dest, equation), constant) = blend(fade.mode);
    let r = fade.rect;
    let rect = [
        r.x as f32,
        r.y as f32,
        (r.x + r.width) as f32,
        (r.y + r.height) as f32,
    ];
    let mut overlay = Overlay::new(size);
    overlay
        .push_fill(
            r,
            rect,
            [1.0; 4],
            OverlayBlend::On {
                source,
                dest,
                equation,
            },
        )
        .map_err(|_| "overlay")?;
    let item = &mut overlay.items[0];
    item.material.program = Program::Shader { tag: 0 };
    item.color_write = OverlayItem::RGB_WRITE;
    if constant {
        let c = fade.constant();
        item.blend_constant = [16, 8, 0, 24].map(|s| (((c >> s) & 0xff) as f64 / 255.0) as f32);
    }
    item.shader_words = Some(psdef(fade).to_vec().into_boxed_slice());
    overlay.set_slot(ViewSlot::Fade);
    Ok(overlay)
}

/// The target the cut draws over: the scene overlay's, else target 0's
/// (`0x0001dd60`, as the frame tail reads it).
fn target(mem: &dyn GuestMemory) -> [u32; 2] {
    let Some(size) = crate::scene_step::with_overlay(|o| o.size) else {
        unimplemented!("fade: no scene being built");
    };
    if size != [0, 0] {
        return size;
    }
    match crate::frame_tail::target0_size(mem) {
        Ok((w, h)) if w != 0 && h != 0 => [w, h],
        _ => unimplemented!("fade: target 0's size"),
    }
}

/// The hook at the fade's entry, before its scope opens: the cut's `ret`.
pub(super) fn enter(c: &mut Call<'_>) -> TransferAbi {
    let rt = std::sync::Arc::clone(c.ctx.runtime());
    // The memory guard is dropped before `ret` locks it again: a guard in
    // the match scrutinee lives to the end of the match and deadlocks.
    let read = read(&*rt.memory());
    let fade = match read {
        // The body tests its mode and returns.
        Ok(None) => return c.ret(0).unwrap_or_else(|_| unimplemented!("fade: return")),
        Ok(Some(fade)) => fade,
        Err(cause) => unimplemented!("fade: {cause}"),
    };
    let size = target(&*rt.memory());
    let planned = plan(&fade, size).unwrap_or_else(|cause| unimplemented!("fade: {cause}"));
    let out = c.ret(0).unwrap_or_else(|_| unimplemented!("fade: return"));
    {
        let mut mem = rt.memory();
        for (k, w) in psdef(&fade).iter().enumerate() {
            if mem.set_u32(PSDEF + 4 * k as u32, *w).is_err() {
                unimplemented!("fade: PSDEF unwritable");
            }
        }
    }
    if crate::scene_step::with_overlay(|o| o.append(&planned).is_ok()) != Some(true) {
        unimplemented!("fade: append");
    }
    Ui::OverlayFadeQuads.count();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;
    use frame_scene::OverlayKind;

    fn mem(mode: u32, colour: [f32; 4], fraction: f32) -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        m.set_u32(MODE, mode).unwrap();
        m.set_f32(FRACTION, fraction).unwrap();
        for (k, v) in colour.into_iter().enumerate() {
            m.set_f32(COLOUR + 4 * k as u32, v).unwrap();
        }
        m.set_f32(ONE, 1.0).unwrap();
        m.set_f32(BYTE, 255.0).unwrap();
        // top 0, left 0, bottom 480, right 640.
        m.set_u32(WINDOW, 0).unwrap();
        m.set_u32(WINDOW + 4, 480 | 640 << 16).unwrap();
        m
    }

    fn fade(mode: u32) -> Fade {
        read(&mem(mode, [1.0, 0.25, 0.5, 1.0], 0.5))
            .unwrap()
            .unwrap()
    }

    /// The cut's item follows the HUD's and comes before the UI's, in
    /// slot `Fade`, a `Fill` drawn by the body's combiner words.
    #[test]
    fn the_fade_draws_between_the_hud_and_the_ui() {
        let size = [640, 480];
        let mut scene = Overlay::new(size);
        let mut hud = Overlay::new(size);
        hud.push_fill(
            ClipRect::full(size),
            [0.0, 0.0, 8.0, 8.0],
            [1.0; 4],
            OverlayBlend::ALPHA,
        )
        .unwrap();
        hud.set_slot(ViewSlot::Hud);
        scene.append(&hud).unwrap();
        let f = fade(1);
        scene.append(&plan(&f, size).unwrap()).unwrap();
        let mut ui = Overlay::new(size);
        ui.push_fill(
            ClipRect::full(size),
            [0.0, 0.0, 8.0, 8.0],
            [1.0; 4],
            OverlayBlend::ALPHA,
        )
        .unwrap();
        scene.append(&ui).unwrap();
        let slots: Vec<_> = scene.items.iter().map(|i| i.slot).collect();
        assert_eq!(slots, [ViewSlot::Hud, ViewSlot::Fade, ViewSlot::Ui]);
        let item = &scene.items[1];
        assert_eq!(item.kind, OverlayKind::Fill);
        assert_eq!(item.material.program, Program::Shader { tag: 0 });
        assert_eq!(item.clip, ClipRect::full(size));
        assert_eq!(item.shader_words.as_deref(), Some(&psdef(&f)[..]));
        assert_eq!(scene.in_slot(ViewSlot::Fade).count(), 1);
    }

    /// `0x00131e50` rounds to nearest, ties to even, alpha on top.
    #[test]
    fn the_pack_rounds_as_fistp_does() {
        assert_eq!(
            pack([1.0, 0.5, 0.0, 1.0], 255.0),
            0xff80_00ff,
            "127.5 rounds to 128"
        );
        assert_eq!(
            pack([0.0, 0.5, 1.5, 2.5], 1.0),
            0x0000_0202,
            "0.5 and 2.5 round down, 1.5 up"
        );
    }

    /// The complement keeps the colour's alpha and inverts each colour
    /// channel before the fraction, as `0x00040909`..`0x00040929` do.
    #[test]
    fn the_complement_keeps_alpha() {
        let f = fade(6);
        assert_eq!(f.colour, pack([0.5, 0.125, 0.25, 0.5], 255.0));
        assert_eq!(f.complement, pack([0.5, 0.375, 0.25, 0.0], 255.0));
        assert_eq!(
            psdef(&f)[10],
            f.complement,
            "mode 6 combines with the complement"
        );
        assert_eq!(psdef(&fade(3))[10], f.colour);
    }

    /// Each case's blend, its constant and its stage 0 inputs.
    #[test]
    fn each_mode_has_the_bodys_blend_and_combiner() {
        let cases = [
            (1, (1, 0x303, 0x8006), false, 0x1120_0000, 0x0120_0000),
            (2, (1, 1, 0x800b), false, 0x1120_0000, 0x0120_0000),
            (3, (0x307, 0x8002, 0x8008), true, 0, 0x0120_1140),
            (4, (0x307, 0x8002, 0x8007), true, 0, 0x0120_1120),
            (5, (0x307, 0x8002, 0x8006), true, 0, 0x0141_1120),
            (6, (1, 0x8002, 0x8006), true, 0, 0x1120_0000),
        ];
        for (mode, (source, dest, equation), constant, alpha, rgb) in cases {
            let f = fade(mode);
            let o = plan(&f, [640, 480]).unwrap();
            let item = &o.items[0];
            assert_eq!(
                item.material.blend,
                OverlayBlend::On {
                    source,
                    dest,
                    equation
                },
                "mode {mode}"
            );
            assert_eq!(item.blend_constant != [0.0; 4], constant, "mode {mode}");
            let w = psdef(&f);
            assert_eq!(
                (w[0], w[34], w[8], w[9], w[26], w[45], w[53]),
                (alpha, rgb, 0xc, 0x1c00, 0xc00, 0xc00, 1)
            );
        }
    }

    /// Mode 0 draws nothing; a mode past the table, or an empty view, is
    /// not made.
    #[test]
    fn mode_zero_draws_nothing_and_a_bad_mode_is_named() {
        assert_eq!(read(&mem(0, [1.0; 4], 1.0)), Ok(None));
        assert_eq!(read(&mem(7, [1.0; 4], 1.0)), Err("mode"));
        let mut m = mem(1, [1.0; 4], 1.0);
        m.set_u32(WINDOW + 4, 0).unwrap();
        assert_eq!(read(&m), Err("rect"));
    }
}
