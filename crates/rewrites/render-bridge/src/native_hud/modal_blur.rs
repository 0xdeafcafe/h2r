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
//! [`wrapper`] builds the call's words from the fraction and the globals
//! and gives what `engine::glow::blur` gives for them and what
//! `engine::glow::blur_combiner` gives for the strength and `y`. The
//! wrapper is implemented here ([`place`]): the guest's blur body never
//! runs, the host places the pass ([`Host::modal_blur`],
//! [`Host::finish_modal_blur`]) and the overlay's blur is listed. A call
//! the plan cannot take stops by name ([`Refusal`]).
//!
//! [`Host::modal_blur`]: super::Host::modal_blur
//! [`Host::finish_modal_blur`]: super::Host::finish_modal_blur

use engine::glow::{self, BlurArgs, BlurDraw, Psdef};
use guest_layout::memory::GuestMemory;
use guest_layout::points;
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
/// The wrapper's multiplier of the fraction (`mulss`) and the strength it
/// pushes.
const STEPS_FACTOR: u32 = 0x0045_de14;
const STRENGTH: u32 = 0x0045_dc20;
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
/// (The words as the guest pushed them; the replacement builds them with
/// [`wrapper`].)
#[cfg(test)]
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
    let wrapper_esp = esp.checked_add(4 + 4 * WORDS).ok_or(Refusal::Arguments)?;
    let caller = word(mem, wrapper_esp).ok_or(Refusal::Arguments)?;
    build(mem, words, source, wrapper_esp, caller, mxcsr, x87_control)
}

/// The blur `0x00235756` makes, from its argument: `fraction` the screen's
/// transition fraction (XMM0), `esp` the wrapper's return slot. The words
/// are the ones the wrapper pushes for `0x000211a0` (steps, scale 1.0,
/// growth 1.0, dest 1, other -1, last 0, combine 1, 0, the strength
/// `[0x0045dc20]`, `y` 0.0). `Ok(None)`: no draws.
pub fn wrapper(
    mem: &dyn GuestMemory,
    fraction: f32,
    esp: u32,
    mxcsr: u32,
    x87_control: u16,
) -> Result<Option<ModalBlur>, Refusal> {
    let factor = f32::from_bits(word(mem, STEPS_FACTOR).ok_or(Refusal::Arguments)?);
    let steps = (fraction * factor).to_bits();
    let strength = word(mem, STRENGTH).ok_or(Refusal::Arguments)?;
    let words = [steps, ONE, ONE, 1, u32::MAX, 0, 1, 0, strength, 0];
    let caller = word(mem, esp).ok_or(Refusal::Arguments)?;
    build(mem, words, 0, esp, caller, mxcsr, x87_control)
}

/// The blur for `words` (the ten the blur takes) from `source`.
fn build(
    mem: &dyn GuestMemory,
    words: [u32; WORDS as usize],
    source: u32,
    wrapper_esp: u32,
    caller: u32,
    mxcsr: u32,
    x87_control: u16,
) -> Result<Option<ModalBlur>, Refusal> {
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

/// Lists `blur` in the scene's overlay over the UI's canvas.
fn record(blur: OverlayBlur) -> Option<bool> {
    let size = scene_extract::ui::CANVAS;
    let mut part = frame_scene::Overlay::new(size);
    part.push_blur(frame_scene::ClipRect::full(size), blur)
        .ok()?;
    crate::scene_step::with_overlay(|o| o.append(&part).is_ok())
}

/// `0x00235756` in Rust: the blur the wrapper asks `0x000211a0` for is
/// placed over the bound target and in the scene's overlay, and the
/// wrapper returns. The guest's blur body (`0x000211a0` and its combiner,
/// target binds, quads and copy) never runs.
fn replace(
    cpu: &mut runtime::CpuAbi,
    ctx: &mut runtime::ExecCtx,
    _addr: u32,
) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let rt = std::sync::Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let esp = cpu.esp;
    let ret = mem.u32(esp)?;
    let fraction = f32::from_le_bytes([cpu.xmm[0][0], cpu.xmm[0][1], cpu.xmm[0][2], cpu.xmm[0][3]]);
    let planned = wrapper(&*mem, fraction, esp, cpu.mxcsr, cpu.x87.control)
        .unwrap_or_else(|why| unimplemented!("dialog blur: {why:?}"));
    if let Some(call) = planned {
        let host = host().unwrap_or_else(|| unimplemented!("dialog blur: no host"));
        if !host.modal_blur(&mut *mem, &call) || !host.finish_modal_blur(&mut *mem, esp) {
            unimplemented!("dialog blur: the device did not take the pass");
        }
        Ui::OverlayModalBlurs.count();
        if record(call.blur) != Some(true) {
            unimplemented!("dialog blur: the scene's overlay refused it");
        }
    }
    // The wrapper's bare RET pops its return slot.
    cpu.esp = esp.wrapping_add(4);
    Ok(runtime::TransferAbi::ret(ret))
}

pub(super) fn place(b: &mut Builder<'_>) {
    b.raw()
        .implement(&points::UI_MODAL_BLUR, "native_ui_modal_blur", replace)
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

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

    /// The wrapper's own words (the fraction times its factor, the fixed
    /// ones, the strength global) give the blur the guest's stack gave.
    #[test]
    fn the_wrapper_builds_the_blur_the_guest_pushed() {
        let mut mem = fixture(3.5, 0.25);
        mem.put_f32(STEPS_FACTOR, 7.0).put_f32(STRENGTH, 0.25);
        // The wrapper's return slot holds its caller.
        mem.put_u32(ESP + 44, 0x0023_512d);
        let from_stack = read(&mem, ESP, 0, MXCSR, X87).unwrap().unwrap();
        let from_wrapper = wrapper(&mem, 0.5, ESP + 44, MXCSR, X87).unwrap().unwrap();
        assert_eq!(from_wrapper.blur, from_stack.blur);
        assert_eq!(from_wrapper.draws.len(), from_stack.draws.len());
        assert_eq!(from_wrapper.copy, from_stack.copy);
        assert_eq!(wrapper(&mem, 0.0, ESP + 44, MXCSR, X87), Ok(None));
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
