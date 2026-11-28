//! The aim-assist cone `1a50a0` (`AIM_ASSIST_PARAMS`) as an idiomatic
//! kernel over Rust-owned data (docs/engine/sim-rewrite.md, "Seats and
//! rules (phase 7)"; the guest contract is docs/game/aim-assist.md).
//!
//! `1a50a0 (unit: ecx, out: esi, zoom: stack 0) -> al`, `ret 4`: the
//! cone the input search `1a4900` and the shot aim `1a4c00` bend towards,
//! built from the unit's held weapons. The guest's three callees are part
//! of the kernel: the zoom magnification `101090`, the tracking test
//! `1011d0` and the acquisition test `101160`. Only the magnification's
//! x87 sequence (`max / min`, the CRT `pow`, `* min`) is the caller's, through
//! [`Magnify`], because it runs the image's `pow` on the x87 model.
//!
//! The caller copies every field the call reads into an [`Input`] first:
//! the unit's two hands, each weapon's tag fields and its trigger states.
//! The kernel reads nothing else, so the same code runs on synthetic
//! values in tests. Arithmetic is `sim_core::det::Fp`: one rounding per
//! instruction in the original's order, and [`Answer::Inexact`] when an
//! operand leaves the range where that is provably the guest's result.

use sim_core::det::{self, Fp};

/// A held weapon's zoom range (`weap +0x1fe`, `+0x200`, `+0x204`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoomRange {
    /// The number of zoom levels (i16).
    pub levels: i16,
    /// The least and greatest magnification, f32 bits. A value not above
    /// zero reads as 1.
    pub min: u32,
    pub max: u32,
}

/// One weapon definition's aim-assist fields, f32 bits where they are
/// floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WeaponAim {
    /// `+0x208` `cone_angle`, `+0x20c` `cone_range`, `+0x210` `pull_angle`,
    /// `+0x214` `pull_range`, `+0x218` `bend_angle`.
    pub cone_angle: u32,
    pub cone_range: u32,
    pub pull_angle: u32,
    pub pull_range: u32,
    pub bend_angle: u32,
    /// `+0x294`: the weapon class the flags pick from (1 and 2 matter).
    pub class: u16,
    /// Flags `+0x12c` bit 5: with no zoom level asked (`-1`) the weapon
    /// gives no cone.
    pub needs_zoom: bool,
    pub zoom: ZoomRange,
}

/// One trigger of a held weapon: the weapon object's state byte
/// (`+0x20c`, stride 12) and the definition's trigger kind (`+0x2cc`
/// block, stride `0x40`, word `+6`), which the tracking test reads only
/// for an idle trigger (state 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trigger {
    pub state: u8,
    pub kind: Option<u16>,
}

/// A held weapon: its definition's fields and its triggers' states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Weapon<'a> {
    pub aim: WeaponAim,
    pub triggers: &'a [Trigger],
}

/// What one of the unit's two weapon slots holds (`unit +0x212` and
/// `+0x213`, signed bytes indexing the weapon handles at `+0x218`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand<'a> {
    /// The slot byte is `-1`.
    NoSlot,
    /// The slot names an index whose weapon handle is `-1`.
    Empty {
        index: i16,
    },
    Held(Weapon<'a>),
}

/// One call's inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input<'a> {
    /// The image's 1.0 (`0x45dbc0`).
    pub one: u32,
    /// The zoom level asked for (the stack word's low half, signed).
    pub zoom: i16,
    /// The unit's primary and secondary hands; `None` for no unit
    /// (`ecx` = `-1`).
    pub hands: Option<[Hand<'a>; 2]>,
}

/// The x87 half of the zoom magnification `101090`: `fld max; fdiv min;
/// fld t; call pow; fmul min; fstp m32`. `None` when the sequence leaves
/// the exact model; the call is then inexact.
pub trait Magnify {
    fn magnify(&mut self, max: u32, min: u32, t: u32) -> Option<u32>;
}

impl<F: FnMut(u32, u32, u32) -> Option<u32>> Magnify for F {
    fn magnify(&mut self, max: u32, min: u32, t: u32) -> Option<u32> {
        self(max, min, t)
    }
}

/// The 0x24-byte record the call fills at `out`, f32 bits except
/// `flags`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cone {
    /// `+0`, `+4`: the primary's cone, zoom applied.
    pub cone_angle: u32,
    pub cone_range: u32,
    /// `+8`, `+0xc`: the pull, the larger of both hands' when dual wielding.
    pub pull_angle: u32,
    pub pull_range: u32,
    /// `+0x10`: the bend limit, the larger of `bend_angle` and `cone_angle`
    /// (zoom applied), and of both hands'.
    pub bend: u32,
    /// `+0x14`: 0x20 a tracking class-1 weapon, 8 or 0x10 an acquiring
    /// class-1 or class-2 one.
    pub flags: u32,
    /// `+0x18`, `+0x1c`: the secondary's own `cone_angle` and `cone_range`,
    /// unzoomed, or 0.
    pub second_cone_angle: u32,
    pub second_cone_range: u32,
    /// `+0x20`: `cone_range` again when `flags` is set, else 0.
    pub flagged_range: u32,
}

impl Cone {
    /// The record's nine words in address order.
    pub fn words(&self) -> [u32; 9] {
        [
            self.cone_angle,
            self.cone_range,
            self.pull_angle,
            self.pull_range,
            self.bend,
            self.flags,
            self.second_cone_angle,
            self.second_cone_range,
            self.flagged_range,
        ]
    }
}

/// Why the call gives no cone (`al` 0, nothing written).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unarmed {
    NoUnit,
    NoSlot,
    Empty {
        index: i16,
    },
    /// The zoom level is `-1` and the weapon needs one.
    Unzoomed,
}

/// An XMM register's 128 bits after the call, as the guest's last write
/// to it leaves them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Not written.
    Kept,
    /// The low lane replaced (`cvtsi2ss`), the upper lanes kept.
    Low(u32),
    /// The low lane this value, the upper lanes zero (`movss` from memory,
    /// then scalar arithmetic).
    Whole(u32),
}

/// What the guest's arithmetic leaves in `xmm0`..`xmm3` on an armed call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lanes(pub [Lane; 4]);

/// One call's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Unarmed(Unarmed),
    /// `al` 1 and the record written.
    Armed {
        cone: Cone,
        lanes: Lanes,
    },
    /// An operand left the exact range: the guest answers.
    Inexact,
}

/// Flags `+0x14` values.
pub const TRACKING: u32 = 0x20;
pub const ACQUIRED_CLASS_1: u32 = 8;
pub const ACQUIRED_CLASS_2: u32 = 0x10;

/// `1a50a0` on `input`.
pub fn cone(input: &Input<'_>, magnify: &mut impl Magnify) -> Answer {
    cone_with(&mut Fp::new(), input, magnify)
}

/// [`cone`] under the guest's `mxcsr` ([`Fp::tracked`]; it must pass
/// [`sim_core::det::guest_env`]): the answer and the MXCSR after it, PE
/// raised when an operation was inexact.
pub fn cone_tracked(input: &Input<'_>, mxcsr: u32, magnify: &mut impl Magnify) -> (Answer, u32) {
    let mut fp = Fp::tracked(mxcsr);
    let answer = cone_with(&mut fp, input, magnify);
    (answer, fp.mxcsr().unwrap_or(mxcsr))
}

fn cone_with(fp: &mut Fp, input: &Input<'_>, magnify: &mut impl Magnify) -> Answer {
    let Some([primary, secondary]) = input.hands else {
        return Answer::Unarmed(Unarmed::NoUnit);
    };
    let first = match primary {
        Hand::NoSlot => return Answer::Unarmed(Unarmed::NoSlot),
        Hand::Empty { index } => return Answer::Unarmed(Unarmed::Empty { index }),
        Hand::Held(w) => w,
    };
    if input.zoom == -1 && first.aim.needs_zoom {
        return Answer::Unarmed(Unarmed::Unzoomed);
    }
    let one = input.one;
    fp.check(one);
    let Some(z1) = zoom(fp, one, input.zoom, &first.aim.zoom, magnify) else {
        return Answer::Inexact;
    };
    let a = first.aim;
    let inv1 = fp.div(one, z1.magnification);
    let mut cone = Cone {
        cone_angle: fp.mul(a.cone_angle, inv1),
        cone_range: fp.mul(a.cone_range, z1.magnification),
        pull_angle: fp.mul(a.pull_angle, inv1),
        pull_range: fp.mul(a.pull_range, z1.magnification),
        ..Cone::default()
    };
    let widest = wider(fp, a.bend_angle, a.cone_angle);
    let bend1 = fp.mul(widest, inv1);
    cone.bend = bend1;
    let flags = if tracking(&first) {
        (a.class == 1).then_some(TRACKING)
    } else if acquiring(&first) {
        match a.class {
            1 => Some(ACQUIRED_CLASS_1),
            2 => Some(ACQUIRED_CLASS_2),
            _ => None,
        }
    } else {
        None
    };
    if let Some(flags) = flags {
        cone.flags = flags;
        cone.flagged_range = cone.cone_range;
    }
    let lanes = match secondary {
        // The record's second cone is the zeroed xmm0's bits.
        Hand::NoSlot | Hand::Empty { .. } => Lanes([
            Lane::Whole(0),
            Lane::Whole(inv1),
            Lane::Whole(cone.pull_range),
            z1.lane.map_or(Lane::Kept, Lane::Low),
        ]),
        Hand::Held(second) => {
            let b = second.aim;
            let Some(z2) = zoom(fp, one, input.zoom, &b.zoom, magnify) else {
                return Answer::Inexact;
            };
            let inv2 = fp.div(one, z2.magnification);
            cone.second_cone_angle = b.cone_angle;
            cone.second_cone_range = b.cone_range;
            let pull1 = cone.pull_angle;
            let pull2 = fp.mul(b.pull_angle, inv2);
            cone.pull_angle = larger(fp, pull1, pull2);
            let range2 = fp.mul(b.pull_range, z2.magnification);
            cone.pull_range = larger(fp, cone.pull_range, range2);
            let widest = wider(fp, b.bend_angle, b.cone_angle);
            let bend2 = fp.mul(widest, inv2);
            cone.bend = larger(fp, bend1, bend2);
            Lanes([
                Lane::Whole(cone.bend),
                Lane::Whole(bend1),
                Lane::Whole(inv2),
                Lane::Whole(pull1),
            ])
        }
    };
    if !fp.exact() {
        return Answer::Inexact;
    }
    Answer::Armed { cone, lanes }
}

/// The magnification `101090` returns, and the `xmm3` low lane it leaves
/// (`levels - 1`, when it divides by it).
struct Zoomed {
    magnification: u32,
    lane: Option<u32>,
}

/// `101090`: 1 outside the weapon's levels, else
/// `min * (max / min) ^ (level / (levels - 1))`.
fn zoom(
    fp: &mut Fp,
    one: u32,
    level: i16,
    z: &ZoomRange,
    magnify: &mut impl Magnify,
) -> Option<Zoomed> {
    if level < 0 || level >= z.levels {
        return Some(Zoomed {
            magnification: one,
            lane: None,
        });
    }
    let (t, lane) = if z.levels > 1 {
        let steps = det::from_int(i32::from(z.levels) - 1);
        (fp.div(det::from_int(i32::from(level)), steps), Some(steps))
    } else {
        (0, None)
    };
    let min = positive_or(fp, z.min, one);
    let max = positive_or(fp, z.max, one);
    let magnification = magnify.magnify(max, min, t)?;
    fp.check(magnification);
    Some(Zoomed {
        magnification,
        lane,
    })
}

/// `comiss v, 0; jbe` takes `or` for a value not above zero.
fn positive_or(fp: &mut Fp, v: u32, or: u32) -> u32 {
    fp.check_compared(v);
    if det::compare(v, 0).is_gt() { v } else { or }
}

/// `comiss bend, cone; jbe cone`: the bend unless the cone is at least as
/// wide.
fn wider(fp: &mut Fp, bend: u32, cone: u32) -> u32 {
    fp.check_compared(bend);
    fp.check_compared(cone);
    if det::compare(bend, cone).is_gt() {
        bend
    } else {
        cone
    }
}

/// `comiss first, second; jbe second`: a tie is the second's.
fn larger(fp: &mut Fp, first: u32, second: u32) -> u32 {
    fp.check_compared(first);
    fp.check_compared(second);
    if det::compare(first, second).is_gt() {
        first
    } else {
        second
    }
}

/// `1011d0`: an idle trigger (state 0) of kind 5.
fn tracking(w: &Weapon<'_>) -> bool {
    w.triggers.iter().any(|t| t.state == 0 && t.kind == Some(5))
}

/// `101160`: a trigger in state 3, 5, 6 or 7, or in state 2 on a weapon of
/// a class other than 0.
fn acquiring(w: &Weapon<'_>) -> bool {
    w.triggers
        .iter()
        .any(|t| matches!(t.state, 3 | 5 | 6 | 7) || (t.state == 2 && w.aim.class != 0))
}

#[cfg(test)]
mod tests;
