//! The blur under a dialog (docs/graphics/native-post-hud.md, "The blur
//! under a dialog"), drawn from the overlay as a `Blur` item.
//!
//! `0x00235756` (`points::UI_MODAL_BLUR`, Code, `h2 disasm`) takes the
//! screen's transition fraction in XMM0, multiplies it by `[0x0045de14]`
//! and calls the Kawase blur `0x000211a0` with EAX 0 (target 0) and the
//! words: the steps, scale 1.0, growth 1.0, dest 1, other -1, last 0,
//! combine 1, 0, the strength `[0x0045dc20]`, `y` 0.0; then a bare RET.
//! The blur's call returns to `0x00235794`.
//!
//! At that call's entry ([`read`]) the draws are what `engine::glow::blur`
//! gives for those words and the combiner what `engine::glow::blur_combiner`
//! gives for the strength and `y`. The guest's code runs whole: its target
//! binds, texture stage writes, combiner block and dirty bits stay its own.
//! The host arms the device to leave out its draws and draw the overlay's
//! blur in their place when the wrapper returns ([`Host::modal_blur`],
//! [`Host::finish_modal_blur`]); a draw that does not match the plan, or a
//! pass that does not record, gives the guest's draws back in order. A
//! call that does not read is the game's, counted by reason ([`Refusal`]).
//!
//! [`Host::modal_blur`]: super::Host::modal_blur
//! [`Host::finish_modal_blur`]: super::Host::finish_modal_blur

use std::cell::RefCell;

use engine::glow::{self, BlurArgs, BlurDraw, Psdef};
use engine::memory::GuestMemory;
use engine::points;
use frame_scene::OverlayBlur;
use hooks::Builder;
use timeline::native::Ui;

use super::host;

/// `0x00235756`'s call of `0x000211a0` returns here.
pub const BLUR_RETURN: u32 = 0x0023_5794;
/// The tap's offset over the spread, and its negative: the signs of the
/// quad draw's taps (`native_render::glow::quad`).
pub const TAP: u32 = 0x0045_dbbc;
pub const TAP_NEGATIVE: u32 = 0x0045_dbc4;
/// The blur's stack words above its return address.
const WORDS: u32 = 10;
/// The combiner words that tell its forms apart.
const PSDEF_ALPHA_2: u32 = 0x0048_4f70;
const PSDEF_CONSTANTS: [u32; 3] = [0x0048_4f94, 0x0048_4fa0, 0x0048_4fac];
const PSDEF_COUNT: u32 = 0x0048_503c;
/// Stage 2's alpha inputs of the form without `y`.
const ALPHA_2_PLAIN: u32 = 0x1c20_1d20;
const ONE: u32 = 0x3f80_0000;

/// Why a call of the blur under a dialog is the game's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A word is not the one `0x00235756` pushes, or did not read.
    Arguments,
    /// The step count is past `engine::glow::MAX_BLUR_STEPS`.
    Steps,
    /// The combiner is not one the overlay's blur step draws (the `y`
    /// form, or another stage count).
    Combiner,
    /// A draw names a target other than 0 and 1, or the result is not
    /// target 0.
    Targets,
}

impl Refusal {
    pub fn counter(self) -> Ui {
        match self {
            Refusal::Arguments => Ui::OverlayModalBlurRefusedArguments,
            Refusal::Steps => Ui::OverlayModalBlurRefusedSteps,
            Refusal::Combiner => Ui::OverlayModalBlurRefusedCombiner,
            Refusal::Targets => Ui::OverlayModalBlurRefusedTargets,
        }
    }
}

/// One call of the blur under a dialog, read at its entry.
#[derive(Clone, Debug, PartialEq)]
pub struct ModalBlur {
    /// The wrapper's return slot (the blur's entry ESP plus its return
    /// and ten words): the call the device scopes the replacement to.
    pub wrapper_esp: u32,
    /// The wrapper's return address.
    pub caller: u32,
    /// The guest's draws, in order: each from one of targets 0 and 1
    /// into the other.
    pub draws: Vec<BlurDraw>,
    /// The copy into target 0 after an odd count of draws.
    pub copy: Option<(u32, u32)>,
    /// The combiner block `0x00020f30` writes.
    pub psdef: Psdef,
    /// What the overlay draws in their place.
    pub blur: OverlayBlur,
}

fn word(mem: &dyn GuestMemory, at: u32) -> Option<u32> {
    mem.u32(at).ok()
}

/// The overlay blur step's constant for `psdef`: 0 for the plain form
/// (three stages), the packed strength over 255 for the leaning form
/// (eight stages, the same grey in the constant of stages 1, 4 and 7).
pub fn pick(psdef: &Psdef) -> Option<f32> {
    let at = |addr: u32| psdef.0[((addr - glow::PSDEF) / 4) as usize];
    if at(PSDEF_ALPHA_2) != ALPHA_2_PLAIN {
        return None;
    }
    match at(PSDEF_COUNT) & 0xff {
        3 => Some(0.0),
        8 => {
            let c = at(PSDEF_CONSTANTS[0]);
            let [_, r, g, b] = c.to_be_bytes();
            (PSDEF_CONSTANTS.iter().all(|&a| at(a) == c) && r == g && g == b)
                .then(|| f32::from(r) / 255.0)
        }
        _ => None,
    }
}

/// The blur under a dialog at the entry of `0x000211a0`: `esp` its entry
/// ESP, `source` EAX. `Ok(None)`: another caller's call, or no draws.
pub fn read(
    mem: &dyn GuestMemory,
    esp: u32,
    source: u32,
    mxcsr: u32,
    x87_control: u16,
) -> Result<Option<ModalBlur>, Refusal> {
    if word(mem, esp) != Some(BLUR_RETURN) {
        return Ok(None);
    }
    let arg = |i: u32| esp.checked_add(4 + 4 * i).and_then(|at| word(mem, at));
    let mut words = [0u32; WORDS as usize];
    for (i, w) in words.iter_mut().enumerate() {
        *w = arg(i as u32).ok_or(Refusal::Arguments)?;
    }
    let [
        steps,
        scale,
        growth,
        dest,
        other,
        last,
        combine,
        seventh,
        strength,
        y,
    ] = words;
    if source != 0
        || scale != ONE
        || growth != ONE
        || dest != 1
        || other != u32::MAX
        || last != 0
        || combine & 0xff == 0
        || seventh != 0
    {
        return Err(Refusal::Arguments);
    }
    let wrapper_esp = esp.checked_add(4 + 4 * WORDS).ok_or(Refusal::Arguments)?;
    let caller = word(mem, wrapper_esp).ok_or(Refusal::Arguments)?;
    let args = BlurArgs {
        source,
        steps,
        scale,
        growth,
        dest,
        other,
        last,
    };
    let blur = match glow::blur(mem, &args, mxcsr) {
        Ok(blur) => blur,
        Err(engine::light_constants::Error::Unbounded) => return Err(Refusal::Steps),
        Err(_) => return Err(Refusal::Arguments),
    };
    if blur.draws.is_empty() {
        return Ok(None);
    }
    if blur.result != 0
        || blur
            .draws
            .iter()
            .any(|d| !matches!((d.source, d.dest), (0, 1) | (1, 0)))
        || blur.copy.is_some_and(|c| c != (1, 0))
    {
        return Err(Refusal::Targets);
    }
    let psdef = glow::blur_combiner(mem, strength, y, mxcsr, x87_control)
        .map_err(|_| Refusal::Arguments)?;
    let pick = pick(&psdef).ok_or(Refusal::Combiner)?;
    let tap = word(mem, TAP).ok_or(Refusal::Arguments)?;
    if word(mem, TAP_NEGATIVE) != Some(tap ^ 0x8000_0000) {
        return Err(Refusal::Arguments);
    }
    let overlay = OverlayBlur {
        tap: f32::from_bits(tap),
        spreads: blur
            .draws
            .iter()
            .map(|d| f32::from_bits(d.spread))
            .collect(),
        pick,
    };
    if !overlay.tap.is_finite() || overlay.spreads.iter().any(|s| !s.is_finite() || *s < 0.0) {
        return Err(Refusal::Arguments);
    }
    Ok(Some(ModalBlur {
        wrapper_esp,
        caller,
        draws: blur.draws,
        copy: blur.copy,
        psdef,
        blur: overlay,
    }))
}

thread_local! {
    /// The armed call: its wrapper's return slot and its blur.
    static ARMED: RefCell<Option<(u32, OverlayBlur)>> = const { RefCell::new(None) };
}

/// At the blur's entry: arms the host's replacement for a call of the
/// blur under a dialog.
fn enter(mem: &mut dyn GuestMemory, esp: u32, source: u32, mxcsr: u32, x87_control: u16) {
    if word(mem, esp) != Some(BLUR_RETURN) {
        return;
    }
    // A call left armed by an unwind is not this one.
    ARMED.with(|a| a.borrow_mut().take());
    let call = match read(mem, esp, source, mxcsr, x87_control) {
        Ok(Some(call)) => call,
        Ok(None) => return,
        Err(why) => {
            why.counter().count();
            return;
        }
    };
    let Some(host) = host() else { return };
    if host.modal_blur(mem, &call) {
        ARMED.with(|a| *a.borrow_mut() = Some((call.wrapper_esp, call.blur)));
    } else {
        Ui::OverlayModalBlurRefusedPass.count();
    }
}

/// At the wrapper's return (`esp` its return slot): the host commits the
/// replacement, and the blur joins the scene's overlay.
fn leave(mem: &mut dyn GuestMemory, esp: u32) {
    let Some((armed, blur)) = ARMED.with(|a| a.borrow_mut().take()) else {
        return;
    };
    if armed != esp {
        return;
    }
    let Some(host) = host() else { return };
    if !host.finish_modal_blur(mem, esp) {
        Ui::OverlayModalBlurReplayed.count();
        return;
    }
    Ui::OverlayModalBlurs.count();
    if record(blur) == Some(false) {
        eprintln!("ui overlay: the scene's overlay refused a dialog's blur");
    }
}

/// Lists `blur` in the scene's overlay over the UI's canvas.
fn record(blur: OverlayBlur) -> Option<bool> {
    let size = scene_extract::ui::CANVAS;
    let mut part = frame_scene::Overlay::new(size);
    part.push_blur(frame_scene::ClipRect::full(size), blur)
        .ok()?;
    crate::scene_step::with_overlay(|o| o.append(&part).is_ok())
}

pub(super) fn place(b: &mut Builder<'_>) {
    b.raw()
        .observe_before(
            &points::SCREEN_CONVOLUTION_BLUR,
            "native_ui_modal_blur",
            |o| {
                let mut mem = o.guest();
                enter(
                    &mut *mem,
                    o.cpu.esp,
                    o.cpu.eax,
                    o.cpu.mxcsr,
                    o.cpu.x87.control,
                );
            },
        )
        .context_only()
        .optional();
    b.raw()
        .observe_after(&points::UI_MODAL_BLUR, "native_ui_modal_blur_end", |o| {
            let mut mem = o.guest();
            // The wrapper's bare RET pops its return slot.
            leave(&mut *mem, o.cpu.esp.wrapping_sub(4));
        })
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    const ESP: u32 = 0x0010_0000;
    const MXCSR: u32 = 0x1f80;
    const X87: u16 = 0x027f;

    /// The constants the blur reads, invented: one, a spread factor of 3,
    /// a tap of 0.375 and its negative, a byte's scale of 255.
    fn constants(mem: &mut FakeGuest) {
        mem.put_f32(glow::constant::ONE, 1.0)
            .put_f32(glow::constant::SPREAD, 3.0)
            .put_f32(TAP, 0.375)
            .put_f32(TAP_NEGATIVE, -0.375)
            .put_f32(glow::constant::PACK_SCALE, 255.0);
    }

    /// The words `0x00235756` pushes for `steps` (the fraction times its
    /// constant) with strength `strength`, and its caller's return above
    /// them.
    fn call(mem: &mut FakeGuest, steps: f32, strength: f32) {
        mem.put_u32(ESP, BLUR_RETURN);
        let words = [
            steps.to_bits(),
            ONE,
            ONE,
            1,
            u32::MAX,
            0,
            1,
            0,
            strength.to_bits(),
            0.0f32.to_bits(),
        ];
        for (i, w) in words.into_iter().enumerate() {
            mem.put_u32(ESP + 4 + 4 * i as u32, w);
        }
        mem.put_u32(ESP + 44, 0x0023_512d);
    }

    fn fixture(steps: f32, strength: f32) -> FakeGuest {
        let mut mem = FakeGuest::new();
        constants(&mut mem);
        call(&mut mem, steps, strength);
        mem
    }

    /// 3.5 steps draw four times, 0 to 1 and back, the scale growing by
    /// the whole spread factor (growth 1), the last step at its fraction:
    /// the overlay's spreads are those.
    #[test]
    fn four_steps_ping_pong_and_grow_by_the_spread_factor() {
        let mem = fixture(3.5, 0.0);
        let call = read(&mem, ESP, 0, MXCSR, X87).unwrap().unwrap();
        assert_eq!(call.wrapper_esp, ESP + 44);
        assert_eq!(call.caller, 0x0023_512d);
        let ends: Vec<_> = call.draws.iter().map(|d| (d.source, d.dest)).collect();
        assert_eq!(ends, [(0, 1), (1, 0), (0, 1), (1, 0)]);
        assert_eq!(call.copy, None, "an even count ends in target 0");
        assert_eq!(call.blur.spreads, [1.0, 3.0, 9.0, 13.5]);
        assert_eq!((call.blur.tap, call.blur.pick), (0.375, 0.0));
    }

    /// Mid-transition an odd count ends in target 1 and the guest copies
    /// it back: the overlay draws the same steps into target 0.
    #[test]
    fn an_odd_count_copies_back_to_target_0() {
        let mem = fixture(2.5, 0.0);
        let call = read(&mem, ESP, 0, MXCSR, X87).unwrap().unwrap();
        assert_eq!(call.draws.len(), 3);
        assert_eq!(call.copy, Some((1, 0)));
        assert_eq!(call.blur.spreads, [1.0, 3.0, 4.5], "the last step at half");
    }

    /// The strength the guest pushes picks the leaning combiner: its
    /// constant, packed to a byte, is the overlay's pick.
    #[test]
    fn the_strength_gives_the_leaning_combiner_and_its_constant() {
        let mem = fixture(3.5, 0.25);
        let call = read(&mem, ESP, 0, MXCSR, X87).unwrap().unwrap();
        // 0.25 * 255 = 63.75, rounded to nearest: 64.
        assert_eq!(call.blur.pick, 64.0 / 255.0);
        let count = call.psdef.0[((PSDEF_COUNT - glow::PSDEF) / 4) as usize];
        assert_eq!(count & 0xff, 8);
        let plain = read(&fixture(3.5, 0.0), ESP, 0, MXCSR, X87)
            .unwrap()
            .unwrap();
        assert_eq!(
            plain.psdef.0[((PSDEF_COUNT - glow::PSDEF) / 4) as usize] & 0xff,
            3
        );
    }

    /// Another caller's call, and a fraction of 0, are not read; a word
    /// the wrapper does not push, the `y` form and endless steps are
    /// refused by name.
    #[test]
    fn other_calls_are_left_alone_and_odd_words_refused_by_name() {
        let mut other = fixture(3.5, 0.0);
        other.put_u32(ESP, 0x0002_22c7);
        assert_eq!(
            read(&other, ESP, 0, MXCSR, X87),
            Ok(None),
            "the shadow filter's call"
        );
        assert_eq!(
            read(&fixture(0.0, 0.0), ESP, 0, MXCSR, X87),
            Ok(None),
            "no steps, no draws"
        );
        assert_eq!(
            read(&fixture(3.5, 0.0), ESP, 7, MXCSR, X87),
            Err(Refusal::Arguments),
            "another source"
        );
        for (i, bad) in [(1, 0x3fa0_0000), (3, 2), (5, u32::MAX), (6, 0), (7, 1)] {
            let mut mem = fixture(3.5, 0.0);
            mem.put_u32(ESP + 4 + 4 * i, bad);
            assert_eq!(
                read(&mem, ESP, 0, MXCSR, X87),
                Err(Refusal::Arguments),
                "word {i}"
            );
        }
        let mut y = fixture(3.5, 0.0);
        y.put_f32(ESP + 4 + 4 * 9, 0.5);
        assert_eq!(
            read(&y, ESP, 0, MXCSR, X87),
            Err(Refusal::Combiner),
            "the y form"
        );
        let endless = fixture(f32::INFINITY, 0.0);
        assert_eq!(read(&endless, ESP, 0, MXCSR, X87), Err(Refusal::Steps));
        let mut signs = fixture(3.5, 0.0);
        signs.put_f32(TAP_NEGATIVE, 0.375);
        assert_eq!(
            read(&signs, ESP, 0, MXCSR, X87),
            Err(Refusal::Arguments),
            "taps not a square"
        );
    }

    /// Each refusal has its own counter.
    #[test]
    fn every_refusal_names_its_counter() {
        let names: Vec<_> = [
            Refusal::Arguments,
            Refusal::Steps,
            Refusal::Combiner,
            Refusal::Targets,
        ]
        .map(|r| format!("{:?}", r.counter()))
        .into();
        let mut unique = names.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), names.len());
    }
}
