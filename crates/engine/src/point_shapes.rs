//! Per-point verification: what a hook point expects to find at its
//! address in the user's image, checked when the plan is installed, so a
//! wrong address fails with the point's name instead of hooking the
//! middle of some other function (docs/engine/feature-engine.md,
//! "Point shapes").
//!
//! A shape is a few encoding facts about x86 code, never the image's
//! bytes: "a return or padding comes right before it" (a function start),
//! "it is a call", "it opens a stack frame". The check reads the user's
//! image at run time.
//!
//! By default every function point (any signature but [`Sig::Site`]) must
//! start a function, and a site is not checked. [`DECLARED`] overrides
//! that per point: stronger shapes for sites that are calls, and the
//! function points that do not follow a return or padding in the image
//! (entries reached by falling through or after a call that does not
//! return), with what they do have. `tests/point_shapes.rs` checks every
//! point against the user's XBE.

use crate::points::{self, Point, Sig, ui_render};

/// One fact about the code at a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The byte before is a `ret`, `int3` or `nop`, or the three before
    /// are a `ret imm16`: the address starts a function.
    Boundary,
    /// It opens a stack frame: `push ebp; mov ebp, esp`.
    Frame,
    /// It is a `call rel32`.
    DirectCall,
    /// It is a `call r/m32` (`FF /2`).
    IndirectCall,
}

const RET: u8 = 0xc3;
const RET_IMM16: u8 = 0xc2;
const INT3: u8 = 0xcc;
const NOP: u8 = 0x90;
const PUSH_EBP: u8 = 0x55;
const MOV_R32_RM32: u8 = 0x8b;
/// `mod 11, reg ebp, rm esp`: the ModR/M byte of `mov ebp, esp`.
const MODRM_EBP_ESP: u8 = 0xec;
const CALL_REL32: u8 = 0xe8;
const GROUP_5: u8 = 0xff;

/// Bytes read before and from the address.
pub const BEFORE: usize = 3;
pub const AT: usize = 3;

impl Check {
    pub fn describe(self) -> &'static str {
        match self {
            Check::Boundary => "a function start (a return or padding before it)",
            Check::Frame => "a frame prologue (push ebp; mov ebp, esp)",
            Check::DirectCall => "a call rel32",
            Check::IndirectCall => "an indirect call",
        }
    }

    /// Whether it holds, given the [`BEFORE`] bytes before the address and
    /// the [`AT`] bytes from it.
    pub fn holds(self, before: &[u8; BEFORE], at: &[u8; AT]) -> bool {
        match self {
            Check::Boundary => {
                matches!(before[BEFORE - 1], RET | INT3 | NOP) || before[0] == RET_IMM16
            }
            Check::Frame => *at == [PUSH_EBP, MOV_R32_RM32, MODRM_EBP_ESP],
            Check::DirectCall => at[0] == CALL_REL32,
            Check::IndirectCall => at[0] == GROUP_5 && (at[1] >> 3) & 7 == 2,
        }
    }
}

use Check::{Boundary, DirectCall, Frame, IndirectCall};

/// Points whose shape is not the default. An empty list means unchecked:
/// the point is not where the default says, and nothing stronger holds.
pub static DECLARED: &[(&Point, &[Check])] = &[
    // Function points the image does not reach from a return or padding.
    (&points::CONTROLS_ROW_CHOSEN, &[Frame]),
    (&points::LOBBY_START_STATE, &[Frame]),
    (&points::CAMPAIGN_CHECKPOINT_POLL, &[Frame]),
    (&points::AI_DIALOGUE_ENQUEUE, &[]),
    (&points::AI_SPAWN_AT_SQUAD_START, &[]),
    (&points::LIGHT_EXTRA_CASTER, &[]),
    (&points::CRT_FMOD, &[]),
    (&points::MAP_SCREEN_FREE, &[]),
    (&points::PLAYER_PICKUP_SCAN, &[]),
    (&points::LOBBY_MODE_SET, &[]),
    // Sites that are calls.
    (&points::OBJECT_RECORD_APPEND, &[DirectCall]),
    (&points::TRANSPARENT_GEOMETRY_ENTRY, &[DirectCall]),
    (&ui_render::BACKDROP_TINT, &[DirectCall]),
    (&ui_render::BACKDROP_COMPOSITE, &[DirectCall]),
    (&ui_render::MODEL_VIEW_CALL, &[DirectCall]),
    (&ui_render::MODEL_GEOMETRY_CALL, &[DirectCall]),
    (&ui_render::LOADING_SPINNER_READY, &[DirectCall]),
    (&ui_render::LOADING_BITMAP_READY, &[DirectCall]),
    (&ui_render::FADE_FAN_READY, &[DirectCall]),
    (&ui_render::ITEM_CALL, &[IndirectCall]),
];

/// The checks `point` must pass at its address.
pub fn expected(point: &Point) -> &'static [Check] {
    if let Some((_, checks)) = DECLARED
        .iter()
        .find(|(p, _)| p.name == point.name && p.base == point.base)
    {
        return checks;
    }
    match point.sig {
        Sig::Site => &[],
        Sig::Opaque | Sig::Fn { .. } => &[Boundary],
    }
}

/// What the image has at a point.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to check.
    Unchecked,
    Holds,
    /// The bytes did not read (not an image, or not mapped readable).
    Unreadable,
    /// A check failed: the point's name, its address, what was expected
    /// and what is there.
    Mismatch(String),
}

/// Checks `point` at `addr` (its address in this image) with `read`,
/// which fills a buffer from guest memory and says whether it could.
pub fn verify(point: &Point, addr: u32, read: &dyn Fn(u32, &mut [u8]) -> bool) -> Verdict {
    let checks = expected(point);
    if checks.is_empty() {
        return Verdict::Unchecked;
    }
    let mut before = [0u8; BEFORE];
    let mut at = [0u8; AT];
    if !read(addr.wrapping_sub(BEFORE as u32), &mut before) || !read(addr, &mut at) {
        return Verdict::Unreadable;
    }
    let failed: Vec<&str> = checks
        .iter()
        .filter(|c| !c.holds(&before, &at))
        .map(|c| c.describe())
        .collect();
    if failed.is_empty() {
        return Verdict::Holds;
    }
    Verdict::Mismatch(format!(
        "{} ({addr:#010x}) is not {}: the image has {before:02x?} before it and {at:02x?} at it",
        point.name,
        failed.join(" or ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic image: `code` at `base`, nothing else.
    fn reader(base: u32, code: &'static [u8]) -> impl Fn(u32, &mut [u8]) -> bool {
        move |addr: u32, buf: &mut [u8]| {
            let Some(off) = addr.checked_sub(base).map(|o| o as usize) else {
                return false;
            };
            match code.get(off..off + buf.len()) {
                Some(b) => {
                    buf.copy_from_slice(b);
                    true
                }
                None => false,
            }
        }
    }

    const FN_POINT: Point = Point::new("TEST_FN", 0x1003, Sig::Opaque, "docs/x.md");
    const SITE: Point = Point::new("TEST_SITE", 0x1003, Sig::Site, "docs/x.md");

    #[test]
    fn a_function_point_must_follow_a_return_or_padding() {
        // ret; push ebp; mov ebp, esp
        let image = reader(0x1000, &[0x00, 0x00, 0xc3, 0x55, 0x8b, 0xec]);
        assert_eq!(verify(&FN_POINT, 0x1003, &image), Verdict::Holds);
        // int3 padding
        let image = reader(0x1000, &[0xcc, 0xcc, 0xcc, 0x53, 0x56, 0x57]);
        assert_eq!(verify(&FN_POINT, 0x1003, &image), Verdict::Holds);
        // ret 8, then the function
        let image = reader(0x1000, &[0xc2, 0x08, 0x00, 0x53, 0x56, 0x57]);
        assert_eq!(verify(&FN_POINT, 0x1003, &image), Verdict::Holds);
        // The middle of an instruction stream: named, with what is there.
        let image = reader(0x1000, &[0x8b, 0x45, 0x08, 0x50, 0xe8, 0x00]);
        let Verdict::Mismatch(why) = verify(&FN_POINT, 0x1003, &image) else {
            panic!("a mid-function address passed");
        };
        assert!(why.starts_with("TEST_FN (0x00001003) is not a function start"), "{why}");
    }

    #[test]
    fn sites_are_unchecked_unless_declared_and_unreadable_code_is_not_a_mismatch() {
        let image = reader(0x1000, &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(verify(&SITE, 0x1003, &image), Verdict::Unchecked);
        let nothing = reader(0x9000, &[]);
        assert_eq!(verify(&FN_POINT, 0x1003, &nothing), Verdict::Unreadable);
    }

    #[test]
    fn call_shapes_read_the_opcode() {
        let at = |b: [u8; 3]| b;
        assert!(Check::DirectCall.holds(&[0; 3], &at([0xe8, 0x10, 0x00])));
        assert!(!Check::DirectCall.holds(&[0; 3], &at([0xff, 0x50, 0x10])));
        // call [eax+0x10] is FF /2; jmp [eax+0x10] is FF /4.
        assert!(Check::IndirectCall.holds(&[0; 3], &at([0xff, 0x50, 0x10])));
        assert!(!Check::IndirectCall.holds(&[0; 3], &at([0xff, 0x60, 0x10])));
        assert!(Check::Frame.holds(&[0; 3], &at([0x55, 0x8b, 0xec])));
    }

    #[test]
    fn declared_points_override_the_default() {
        assert_eq!(expected(&points::CRT_FMOD), &[] as &[Check]);
        assert_eq!(expected(&points::CONTROLS_ROW_CHOSEN), &[Frame]);
        assert_eq!(expected(&points::GAME_TICK), &[Boundary]);
        assert_eq!(expected(&ui_render::ITEM_CALL), &[IndirectCall]);
        assert_eq!(expected(&ui_render::ITEM_RETURN), &[] as &[Check]);
        for (p, _) in DECLARED {
            assert!(
                DECLARED.iter().filter(|(q, _)| q.name == p.name).count() == 1,
                "{} declared twice",
                p.name
            );
        }
    }
}
