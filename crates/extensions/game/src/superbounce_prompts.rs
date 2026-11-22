//! The superbounce prompts drawn in the world (docs/game/superbounce.md,
//! "The superbounce game type", "Hold X: the fall assist"): `Hold X` (the
//! X button's glyph) growing on the strip a falling player can be steered
//! onto, `Guiding` once the assist has it, and on the ground `Jump` /
//! `Jump now` at the spot a standing jump takes off from and `Drop 3 m`
//! on strips that need a fall. They are in sentence case, in the HUD's
//! colour and font ([`Look`]), as the HUD writes its prompts.
//!
//! The rules are pure: [`prompt_for`] picks what a strip says from a
//! [`Cue`], [`label_scale`] and [`takeoff`] size and place it, and
//! [`callouts`] builds the lines and labels for one frame from the player's
//! [`Body`], the scan's spots and the assist's pick. Only what is drawn
//! allocates.

use gfx::world_lines::{LineStyle, Pulse};
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_assist::{Body, Reach, drift_unit};
use crate::superbounce_font::Look;
use crate::superbounce_mode::METRES_PER_UNIT;

/// Distance (world units) at and beyond which a prompt is at its base size.
pub const SCALE_FAR: f32 = 6.0;
/// Distance at and inside which a prompt is at its largest.
pub const SCALE_NEAR: f32 = 0.5;
/// The largest [`label_scale`].
pub const SCALE_MAX: f32 = 3.0;
/// A standing jump's strip is within this horizontal distance of the player.
pub const JUMP_RANGE: f32 = 1.5;
/// A strip whose fall is at most this is a standing jump ("about 0").
pub const FLAT_FALL: f32 = 0.05;
/// A strip whose fall is more than this needs a drop.
pub const DROP_FALL: f32 = 0.3;
/// A drop prompt shows within this horizontal distance of the strip.
pub const DROP_RANGE: f32 = 3.0;
/// The take-off spot is this far back from the stand point, against the
/// drift.
pub const TAKEOFF_BACK: f32 = 0.2;
/// Within this horizontal distance of the take-off spot it is time to jump.
pub const NOW_RADIUS: f32 = 0.1;
/// Only strips within this height of the player count on the ground.
pub const SAME_FLOOR: f32 = 1.0;
/// A label floats this far above the point it names.
pub const LABEL_RAISE: f32 = 0.3;
/// The floor ring at the take-off spot.
pub const RING_RADIUS: f32 = 0.15;
const RING_SEGMENTS: usize = 12;

/// `units` world units in whole metres, at least 1: how the labels write
/// a height.
pub fn whole_metres(units: f32) -> i32 {
    ((units * METRES_PER_UNIT).round() as i32).max(1)
}

/// What a strip says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    None,
    /// Falling toward it: hold X.
    HoldX,
    /// X held and the assist has it.
    Guiding,
    Jump,
    /// On the take-off spot.
    JumpNow,
    /// Needs a fall onto it.
    Drop,
}

/// What decides a [`Prompt`] for one strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cue {
    pub primed: bool,
    pub airborne: bool,
    /// The assist's pick (airborne only).
    pub targeted: bool,
    /// The assist is steering now.
    pub engaged: bool,
    /// The player has turned the fall assist on (`superbounce.fall_assist`).
    pub assist: bool,
    /// The strip's fall, world units.
    pub fall: f32,
    /// Horizontal distance from the player to the strip's stand point.
    pub stand_distance: f32,
    /// Horizontal distance from the player to the take-off spot.
    pub takeoff_distance: f32,
}

/// What a strip says for `cue`. Nothing unless the player is primed; in
/// the air nothing unless the fall assist is on, as the landing is the
/// player's own.
pub fn prompt_for(cue: &Cue) -> Prompt {
    if !cue.primed {
        return Prompt::None;
    }
    if cue.airborne {
        return match (cue.assist && cue.targeted, cue.engaged) {
            (false, _) => Prompt::None,
            (true, false) => Prompt::HoldX,
            (true, true) => Prompt::Guiding,
        };
    }
    if cue.fall <= FLAT_FALL && cue.stand_distance <= JUMP_RANGE {
        if cue.takeoff_distance <= NOW_RADIUS {
            Prompt::JumpNow
        } else {
            Prompt::Jump
        }
    } else if cue.fall > DROP_FALL && cue.stand_distance <= DROP_RANGE {
        Prompt::Drop
    } else {
        Prompt::None
    }
}

/// The size of a falling player's `HOLD X` at `distance` world units from
/// the strip: 1 at [`SCALE_FAR`] and beyond, [`SCALE_MAX`] at
/// [`SCALE_NEAR`] and inside, linear between. Not a number is the base.
pub fn label_scale(distance: f32) -> f32 {
    if distance.is_nan() {
        return 1.0;
    }
    let t = ((SCALE_FAR - distance) / (SCALE_FAR - SCALE_NEAR)).clamp(0.0, 1.0);
    1.0 + (SCALE_MAX - 1.0) * t
}

/// Where to jump from for a standing jump onto `spot`: its stand point,
/// [`TAKEOFF_BACK`] back against the drift (the stand point itself without
/// a drift).
pub fn takeoff(spot: &Spot) -> Vec3 {
    let d = drift_unit(spot.drift);
    [
        spot.stand[0] - TAKEOFF_BACK * d[0],
        spot.stand[1] - TAKEOFF_BACK * d[1],
        spot.stand[2],
    ]
}

fn horizontal(a: Vec3, b: Vec3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn distance3(a: Vec3, b: Vec3) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// One prompt: lines (a ring or a guide) and a label, drawn with a style.
#[derive(Clone, Debug, PartialEq)]
pub struct Callout {
    pub edges: Vec<[Vec3; 2]>,
    pub anchor: Vec3,
    pub color: [f32; 3],
    pub label: Option<String>,
    pub style: LineStyle,
}

fn ring(centre: Vec3, radius: f32) -> Vec<[Vec3; 2]> {
    let point = |i: usize| {
        let a = std::f32::consts::TAU * (i % RING_SEGMENTS) as f32 / RING_SEGMENTS as f32;
        [
            centre[0] + radius * a.cos(),
            centre[1] + radius * a.sin(),
            centre[2] + 0.02,
        ]
    };
    (0..RING_SEGMENTS)
        .map(|i| [point(i), point(i + 1)])
        .collect()
}

/// The style of a prompt's label and lines: big centred text in the HUD
/// font, pulsing when `pulse` says so.
fn style(look: &Look, scale: f32, width: f32, pulse: Option<Pulse>) -> LineStyle {
    LineStyle {
        width,
        glow: Some(gfx::world_lines::Glow {
            width: 4.0,
            alpha: 0.4,
        }),
        pulse,
        label_scale: scale,
        label_centered: true,
        label_face: look.face,
        ..LineStyle::default()
    }
}

fn raised(p: Vec3) -> Vec3 {
    [p[0], p[1], p[2] + LABEL_RAISE]
}

/// The prompts for one frame. The mode draws none of them
/// (docs/game/superbounce.md, "In the air"): in the air the screen is the
/// step chain's and the tracker's, and on the ground the floor mark says
/// where to land. This is the rule table (`prompt_for`) and its labels,
/// kept with their tests. `target` is the assist's pick (the strip a
/// falling player can be steered onto, and where the player would land
/// otherwise); `engaged` is whether the assist is steering, and `assist`
/// whether the player has it on. Airborne there is at most the one for the
/// target, and only with the assist on; on the ground, at most the nearest
/// standing-jump strip and the nearest strip that needs a drop. All of
/// them are in `look`'s colour and font.
pub fn callouts(
    spots: &[Spot],
    body: &Body,
    primed: bool,
    target: Option<(&Spot, &Reach)>,
    engaged: bool,
    assist: bool,
    look: &Look,
) -> Vec<Callout> {
    let mut out = Vec::new();
    if !primed {
        return out;
    }
    if body.airborne {
        let Some((spot, reach)) = target else {
            return out;
        };
        let cue = Cue {
            primed,
            airborne: true,
            targeted: true,
            engaged,
            assist,
            fall: spot.fall,
            stand_distance: horizontal(body.position, spot.stand),
            takeoff_distance: f32::INFINITY,
        };
        let text = match prompt_for(&cue) {
            Prompt::HoldX => format!("Hold {}", look.x_button()),
            Prompt::Guiding => "Guiding".into(),
            _ => return out,
        };
        let scale = label_scale(distance3(body.position, spot.stand));
        let land = [reach.landing[0], reach.landing[1], spot.stand[2]];
        out.push(Callout {
            // The way to go on the floor; the landing itself is the
            // world's lock-on (superbounce_world, "Lock-on").
            edges: vec![[land, spot.stand]],
            anchor: raised(spot.stand),
            color: look.colour,
            label: Some(text),
            style: style(
                look,
                scale,
                2.0,
                (!engaged).then_some(Pulse {
                    period: 0.6,
                    depth: 0.5,
                }),
            ),
        });
        return out;
    }
    let mut jump: Option<(f32, &Spot)> = None;
    let mut drop: Option<(f32, &Spot)> = None;
    for s in spots.iter().filter(|s| s.kind == Kind::Bounce) {
        if (body.position[2] - s.stand[2]).abs() > SAME_FLOOR {
            continue;
        }
        let cue = Cue {
            primed,
            airborne: false,
            targeted: false,
            engaged,
            assist,
            fall: s.fall,
            stand_distance: horizontal(body.position, s.stand),
            takeoff_distance: horizontal(body.position, takeoff(s)),
        };
        let slot = match prompt_for(&cue) {
            Prompt::Jump | Prompt::JumpNow => &mut jump,
            Prompt::Drop => &mut drop,
            _ => continue,
        };
        if slot.is_none_or(|(d, _)| cue.stand_distance < d) {
            *slot = Some((cue.stand_distance, s));
        }
    }
    if let Some((_, s)) = jump {
        let at = takeoff(s);
        let now = horizontal(body.position, at) <= NOW_RADIUS;
        let (text, scale, pulse) = if now {
            (
                "Jump now",
                SCALE_MAX,
                Pulse {
                    period: 0.25,
                    depth: 0.7,
                },
            )
        } else {
            (
                "Jump",
                2.5,
                Pulse {
                    period: 0.8,
                    depth: 0.4,
                },
            )
        };
        out.push(Callout {
            edges: ring(at, if now { RING_RADIUS * 1.4 } else { RING_RADIUS }),
            anchor: raised(at),
            color: look.colour,
            label: Some(text.into()),
            style: style(look, scale, 2.5, Some(pulse)),
        });
    }
    if let Some((_, s)) = drop {
        out.push(Callout {
            edges: Vec::new(),
            anchor: raised(s.stand),
            color: look.colour,
            label: Some(format!("Drop {} m", whole_metres(s.fall))),
            style: style(look, 1.5, 1.0, None),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue() -> Cue {
        Cue {
            primed: true,
            airborne: false,
            targeted: false,
            engaged: false,
            assist: true,
            fall: 0.0,
            stand_distance: 1.0,
            takeoff_distance: 0.8,
        }
    }

    fn spot(stand: Vec3, fall: f32, drift: [f32; 2]) -> Spot {
        Spot {
            kind: Kind::Bounce,
            edge: [stand, stand],
            stand,
            fall,
            launch: 17.9,
            drift,
            score: 1.0,
            ..Spot::default()
        }
    }

    const LOOK: Look = Look {
        colour: [0.3, 0.6, 0.9],
        face: gfx::world_lines::LabelFace::Hud,
        glyphs: true,
    };

    fn body(position: Vec3, airborne: bool) -> Body {
        Body {
            position,
            velocity: [0.0, 0.0, if airborne { -2.0 } else { 0.0 }],
            yaw: 0.0,
            airborne,
        }
    }

    #[test]
    fn the_label_grows_from_base_at_six_units_to_three_times_at_half_a_unit() {
        assert_eq!(label_scale(20.0), 1.0);
        assert_eq!(label_scale(6.0), 1.0);
        assert_eq!(label_scale(0.5), 3.0);
        assert_eq!(label_scale(0.0), 3.0);
        assert_eq!(label_scale(f32::NAN), 1.0);
        // Halfway is halfway, and it only ever grows as the player nears.
        assert!((label_scale(3.25) - 2.0).abs() < 1e-6);
        let mut last = 0.0;
        for i in (0..=70).rev() {
            let s = label_scale(i as f32 * 0.1);
            assert!(s >= last);
            last = s;
        }
    }

    #[test]
    fn nothing_is_said_unless_primed() {
        for airborne in [false, true] {
            let c = Cue {
                primed: false,
                airborne,
                targeted: true,
                ..cue()
            };
            assert_eq!(prompt_for(&c), Prompt::None);
        }
    }

    #[test]
    fn airborne_it_is_hold_x_then_guiding_and_only_for_the_target() {
        let c = Cue {
            airborne: true,
            targeted: true,
            ..cue()
        };
        assert_eq!(prompt_for(&c), Prompt::HoldX);
        assert_eq!(prompt_for(&Cue { engaged: true, ..c }), Prompt::Guiding);
        assert_eq!(
            prompt_for(&Cue {
                targeted: false,
                ..c
            }),
            Prompt::None
        );
        // A fall or a standing jump does not matter in the air.
        assert_eq!(prompt_for(&Cue { fall: 5.0, ..c }), Prompt::HoldX);
    }

    #[test]
    fn without_the_fall_assist_nothing_is_said_in_the_air() {
        let c = Cue {
            airborne: true,
            targeted: true,
            assist: false,
            ..cue()
        };
        assert_eq!(prompt_for(&c), Prompt::None);
        assert_eq!(prompt_for(&Cue { engaged: true, ..c }), Prompt::None);
        // On the ground the bounce's own prompts stay.
        assert_eq!(
            prompt_for(&Cue {
                assist: false,
                ..cue()
            }),
            Prompt::Jump
        );
    }

    #[test]
    fn grounded_a_flat_strip_says_jump_and_on_the_spot_jump_now() {
        assert_eq!(prompt_for(&cue()), Prompt::Jump);
        let on = Cue {
            takeoff_distance: 0.1,
            ..cue()
        };
        assert_eq!(prompt_for(&on), Prompt::JumpNow);
        assert_eq!(
            prompt_for(&Cue {
                takeoff_distance: 0.11,
                ..cue()
            }),
            Prompt::Jump
        );
        assert_eq!(
            prompt_for(&Cue {
                stand_distance: 1.5,
                ..cue()
            }),
            Prompt::Jump
        );
        assert_eq!(
            prompt_for(&Cue {
                stand_distance: 1.6,
                ..cue()
            }),
            Prompt::None
        );
    }

    #[test]
    fn grounded_a_strip_that_needs_a_fall_says_drop() {
        let c = Cue {
            fall: 1.0,
            stand_distance: 2.5,
            ..cue()
        };
        assert_eq!(prompt_for(&c), Prompt::Drop);
        assert_eq!(
            prompt_for(&Cue {
                stand_distance: 3.1,
                ..c
            }),
            Prompt::None
        );
        // Between a standing jump and a drop it says nothing.
        assert_eq!(prompt_for(&Cue { fall: 0.2, ..c }), Prompt::None);
        assert_eq!(prompt_for(&Cue { fall: 0.3, ..c }), Prompt::None);
        assert_eq!(prompt_for(&Cue { fall: 0.31, ..c }), Prompt::Drop);
    }

    #[test]
    fn the_takeoff_spot_is_back_against_the_drift() {
        let s = spot([1.0, 2.0, 3.0], 0.0, [0.0, 4.0]);
        let t = takeoff(&s);
        assert!((t[0] - 1.0).abs() < 1e-6 && (t[1] - 1.8).abs() < 1e-6 && t[2] == 3.0);
        // The drift's length does not matter, and no drift is the stand point.
        let t = takeoff(&spot([1.0, 2.0, 3.0], 0.0, [-0.1, 0.0]));
        assert!((t[0] - 1.2).abs() < 1e-6);
        assert_eq!(
            takeoff(&spot([1.0, 2.0, 3.0], 0.0, [0.0, 0.0])),
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn heights_are_written_in_whole_metres_and_never_zero() {
        assert_eq!(whole_metres(1.0), 3);
        assert_eq!(whole_metres(0.5), 2);
        assert_eq!(whole_metres(0.01), 1);
    }

    #[test]
    fn a_falling_player_gets_the_hold_x_label_and_a_guide() {
        let s = spot([0.0, 0.0, 0.0], 0.0, [0.0, 1.0]);
        let reach = Reach {
            time: 0.5,
            aim: [0.0, -0.1],
            arrive: [0.0, 0.8],
            landing: [0.4, 0.2],
            miss: 0.5,
        };
        let b = body([0.4, 0.2, 1.0], true);
        let out = callouts(&[s], &b, true, Some((&s, &reach)), false, true, &LOOK);
        assert_eq!(out.len(), 1);
        let c = &out[0];
        // The X button's glyph, in the HUD's colour and font.
        assert_eq!(c.label.as_deref(), Some("Hold \u{e102}"));
        assert_eq!(c.color, LOOK.colour);
        assert_eq!(c.style.label_face, gfx::world_lines::LabelFace::Hud);
        assert!(c.style.pulse.is_some() && c.style.label_scale > 1.0);
        assert_eq!(c.edges, [[[0.4, 0.2, 0.0], [0.0, 0.0, 0.0]]]);
        // Without the HUD font the button is its letter.
        let letters = Look {
            glyphs: false,
            ..LOOK
        };
        let l = &callouts(&[s], &b, true, Some((&s, &reach)), false, true, &letters)[0];
        assert_eq!(l.label.as_deref(), Some("Hold X"));
        // Engaged: steady.
        let g = &callouts(&[s], &b, true, Some((&s, &reach)), true, true, &LOOK)[0];
        assert_eq!(g.label.as_deref(), Some("Guiding"));
        assert_eq!(g.color, LOOK.colour);
        assert!(g.style.pulse.is_none());
        // The assist off: no Hold X, no Guiding.
        assert!(callouts(&[s], &b, true, Some((&s, &reach)), false, false, &LOOK).is_empty());
        assert!(callouts(&[s], &b, true, Some((&s, &reach)), true, false, &LOOK).is_empty());
        // No target, or not primed: nothing.
        assert!(callouts(&[s], &b, true, None, false, true, &LOOK).is_empty());
        assert!(callouts(&[s], &b, false, Some((&s, &reach)), false, true, &LOOK).is_empty());
    }

    #[test]
    fn on_the_ground_the_nearest_jump_and_drop_strips_get_a_prompt() {
        let flat = spot([1.0, 0.0, 0.0], 0.0, [1.0, 0.0]);
        let far_flat = spot([1.4, 0.0, 0.0], 0.0, [1.0, 0.0]);
        let drop = spot([0.0, 2.0, 0.0], 1.0, [0.0, 1.0]);
        let other_floor = spot([0.5, 0.0, 9.0], 0.0, [1.0, 0.0]);
        let spots = [far_flat, flat, drop, other_floor];
        let out = callouts(
            &spots,
            &body([0.0, 0.0, 0.0], false),
            true,
            None,
            false,
            true,
            &LOOK,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label.as_deref(), Some("Jump"));
        // The nearer flat strip's take-off spot (0.2 back from its stand).
        assert_eq!(out[0].anchor, [0.8, 0.0, LABEL_RAISE]);
        assert_eq!(out[0].edges.len(), RING_SEGMENTS);
        assert_eq!(out[1].label.as_deref(), Some("Drop 3 m"));
        assert!(out.iter().all(|c| c.color == LOOK.colour));
        // The same with the assist off.
        let off = callouts(
            &spots,
            &body([0.0, 0.0, 0.0], false),
            true,
            None,
            false,
            false,
            &LOOK,
        );
        assert_eq!(off, out);
        // On the take-off spot it flashes.
        let on = callouts(
            &spots,
            &body([0.8, 0.0, 0.0], false),
            true,
            None,
            false,
            true,
            &LOOK,
        );
        assert_eq!(on[0].label.as_deref(), Some("Jump now"));
        assert_eq!(on[0].style.label_scale, SCALE_MAX);
        // Not primed, or in the air with no target: none.
        assert!(
            callouts(
                &spots,
                &body([0.0; 3], false),
                false,
                None,
                false,
                true,
                &LOOK
            )
            .is_empty()
        );
        assert!(
            callouts(
                &spots,
                &body([0.0, 0.0, 2.0], true),
                true,
                None,
                false,
                true,
                &LOOK
            )
            .is_empty()
        );
    }
}
