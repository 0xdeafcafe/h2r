//! The superbounce HUD on the kit (docs/game/hud-kit.md, "The superbounce
//! HUD on the kit"), made of things Halo 2 already draws:
//!
//! - a pill at the bottom left, right of the motion tracker, in one
//!   block with the arming bar over it and the readout's rows beside it:
//!   a short pill cut from the frag pill's own art (its body and
//!   its right end, without the frag "x" and picture), with a chevron
//!   where a pill's count sits: lit while primed, inactive while not, dim
//!   while the map is scanned, hidden on a map with no strips;
//! - the charge meter under the pill, the energy sword's battery strip
//!   cropped to the pill's width: how long the charge has left;
//! - the arming bar, Assault's own (`ui\hud\scoreboard`'s
//!   `bomb_defusal`, red, over the pill): how far the player is
//!   through the recipe, with the primed ending, the run out when the
//!   player leaves early and the blink when a charge is lost, moved on
//!   wall time (`crate::motion`; [`ArmTrack`]);
//! - the recipe's verbs on the game's prompt line, the bound button drawn
//!   by the game;
//! - the results ("Charging..." as the arming bar comes up, "Primed!",
//!   lost) and the medals (a bounce's medal glyph and line, as a retail
//!   multi-kill's) in the game's notification list;
//! - a navpoint over the next thing to do, the nearest crouch spot while
//!   not primed and the chosen strip while primed, the game's arrow
//!   pointing down at it and its waypoint marker above, a number in whole
//!   metres over them: the distance to a crouch spot, the drop onto a
//!   strip;
//! - the tutorial in the game's training box, and its step machine
//!   ([`Tutorial`]).
//!
//! [`View`] is what the HUD shows of `superbounce_mode`'s state; the
//! game's runtime (`game::superbounce_hud`) fills it from `State` each
//! tick. Launch and drop are not on the HUD: they are on the strips'
//! labels in the world and in the agent API.

use crate::model::{
    self, Anchor, Art, Describe, Element, Font, Hud, HudRef, Justify, Kind, Origin, PILL_BODY,
    PillPart, Place, Pulse, Style, Value, Values,
};
use crate::motion::Motion;

pub const NAME: &str = "superbounce";

pub const PILL: &str = "sb.pill";
pub const PILL_END: &str = "sb.pill.end";
pub const PILL_ICON: &str = "sb.pill.icon";
pub const CHARGE: &str = "sb.charge";
pub const ARM: &str = "sb.arm";
pub const PROMPT: &str = "sb.prompt";
pub const MESSAGE: &str = "sb.message";
pub const MEDAL: &str = "sb.medal";
pub const NAVPOINT: &str = "sb.navpoint";
pub const TUTORIAL: &str = "sb.tutorial";

/// The pill's icon: the HUD font's chevron (`^` in `conduit-13`, the font
/// of the grenade counts; its arrow characters U+2191 and U+25B2 are the
/// missing box, and its d-pad glyph U+E108 is swapped for the bound key's
/// picture on the other input sets, docs/input/button-icons.md).
pub const ICON_TEXT: &str = "^";
/// The widest navpoint number: whole metres under 1000.
pub const DISTANCE_LONGEST: &str = "999";
/// The distance the preview's crouch-spot navpoints show.
pub const SAMPLE_DISTANCE_M: f32 = 12.0;
/// The drop the preview's strip navpoints show.
pub const SAMPLE_DROP_M: f32 = 3.0;

/// The charge meter's width: the strip's last columns, as wide as the
/// pill ([`PILL_BODY`] and [`model::PILL_END`]).
pub const CHARGE_COLUMNS: u8 = (PILL_BODY + model::PILL_END) as u8;
/// The meter's top below the pill's: the pill's 19 rows (V: `grenade_bkd`,
/// 103 by 19) and 2 more.
pub const CHARGE_DOWN: i16 = 21;
/// At this fill or under the charge is about to go: the icon flashes the
/// timer's warning record. Half: 19 still ticks of the 38 a body has
/// before it sleeps, where the prompt line warns ([`PROMPT_KEEP_MOVING`]).
pub const LOW_CHARGE: f32 = 0.5;
/// The arming bar's fill at each recipe step before the word is -3
/// (docs/game/hud-kit.md, "The arming bar").
pub const FILL_NEAR: f32 = 0.0;
pub const FILL_UNDER: f32 = 1.0 / 3.0;
pub const FILL_HELD: f32 = 2.0 / 3.0;

/// The arming bar's art: Assault's bomb arming meter, the retail widget's
/// bitmap (`ui\hud\bitmaps\new_hud\bomb_meter`), red shader
/// (`ui\hud\shaders\bomb_defusal_meter`) and sequences, by its name
/// in the scoreboard `nhdt` (V: `zanzibar.map`).
pub const ARM_HUD: &str = "ui\\hud\\scoreboard";
pub const ARM_WIDGET: &str = "bomb_defusal";
/// The fullscreen HUD frame's height, 36 to 444: a point this far under
/// the top-left anchor is on the bottom-left one.
pub const FRAME_HEIGHT: i16 = 408;
/// The bar's centre fullscreen, from the top-left anchor: the top of the
/// block right of the motion tracker (x 138 to 278, y 355 to 368), over
/// the pill and the readout's rows. The retail widget's own place is
/// [`RETAIL_ARM_FULL`], from the centre: a centre-anchored widget stays
/// mid-screen when the host pins the HUD's corners to a wide window's
/// edges, and the tracker does not
/// (docs/graphics/high-resolution.md, "HUD and UI").
pub const ARM_FULL: (i16, i16) = (160, 325);
/// The retail widget's fullscreen place: centre, 110 down (V).
pub const RETAIL_ARM_FULL: (i16, i16) = (0, 110);
/// The retail widget's split-screen place, 27 down (V): not ours.
pub const RETAIL_ARM_SPLIT: (i16, i16) = (0, 27);
/// Ours in half screen, from the top-left anchor: 35 above the frame's
/// centre (the frame is 544 by 204). Retail's 27 under the centre puts
/// the bar in the prompt line's box (the `retail` check), which our
/// verbs fill while the bar is up, where retail's carrier has no prompt;
/// under it the training box and the reticle take the rest of a 204-high
/// frame. 35 up is clear of the prompt's first line (the frame's top
/// plus 35, a line of font 5) and under the navpoint's arrow at its
/// sample (docs/game/hud-kit.md, "The arming bar").
pub const ARM_SPLIT: (i16, i16) = (272, 67);
/// In a quarter view (272 by 204), [`ARM_SPLIT`]'s height, 7 left of the
/// centre: the quarter frame has no row free between the navpoint's
/// arrow at its sample and the prompt's first line, and at the centre
/// the bar's right end meets the arrow (x 198 to 203, the `overlap`
/// check); 7 left clears it.
pub const ARM_QUARTER: (i16, i16) = (129, 67);
/// The pill's top left fullscreen, from the top-left anchor: screen
/// (138, 376), 8 right of the motion tracker and 8 under the arming bar,
/// its top in line with the readout's speed row
/// (`superbounce_readout::SPEED_UP`). In half and quarter screen the pill
/// is the grenade row's next (`Origin::NextGrenadePill`), and the row's
/// anchor is the top left: the one anchor the widget has.
pub const PILL_FULL: (i16, i16) = (90, 340);
/// The icon's point from the pill's top left: where a grenade pill's
/// count is in its pill (V: the plasma count at (12, -1), its pill at
/// (0, 0)).
pub const ICON_AT: (i16, i16) = (12, -1);
/// The readout's rows start this far right of the pill's left edge: the
/// pill's width and a gap of 6.
pub const PILL_COLUMN: i16 = PILL_BODY + model::PILL_END + 6;
/// The game's word D bit that shows the retail bar: Assault arming (V:
/// the widget's only show bit). Ours hides while it is set.
pub const ASSAULT_ARMING_D: u16 = 0x400;

/// The default crouch button's glyph, the left stick click (U+E110,
/// docs/input/button-icons.md).
pub const CROUCH: &str = "\u{e110}";
/// The prompt line's verbs: the recipe's steps, the warning while primed
/// and still, and the new charge after a loss.
pub const PROMPT_NEAR: &str = "Hold \u{e110} and walk under";
pub const PROMPT_UNDER: &str = "Let go of \u{e110}";
pub const PROMPT_HELD: &str = "Walk out to stand up";
pub const PROMPT_KEEP_MOVING: &str = "Keep moving or lose the charge";
/// After a loss: the loss's reason is its notification
/// ([`Note::ChargeLost`]).
pub const PROMPT_CHARGE_AGAIN: &str = "Find a crouch spot to charge again";
/// Looking at a bounce spot that has a ghost run, and while one plays
/// (`game::superbounce_ghost`): the button is X (U+E102), pressed on the
/// ground (the fall assist reads X in the air); on Bumper Jumper, where X
/// is the flashlight, it is B (U+E101), also on the ground.
pub const PROMPT_SHOW_GHOST: &str = "Press \u{e102} to show ghost";
pub const PROMPT_STOP_GHOST: &str = "Press \u{e102} to stop ghost";
pub const PROMPT_SHOW_GHOST_B: &str = "Press \u{e101} to show ghost";
pub const PROMPT_STOP_GHOST_B: &str = "Press \u{e101} to stop ghost";
/// The prompt line's widest verb: the most characters.
pub const PROMPT_LONGEST: &str = PROMPT_CHARGE_AGAIN;

/// The notifications. Charging and primed are Assault's pair's shape
/// ("Bomb is arming..." and "Bomb armed!", `multiplayer_globals`).
pub const MESSAGE_CHARGING: &str = "Charging...";
pub const MESSAGE_PRIMED: &str = "Primed!";
pub const MESSAGE_LOST: &str = "Charge lost";
pub const MESSAGE_LOST_STOOD_STILL: &str = "Charge lost: you stood still";
pub const MESSAGE_LOST_CROUCHED: &str = "Charge lost: you crouched";
/// The widest notification: the most characters, all of them plain.
pub const MESSAGE_LONGEST: &str = MESSAGE_LOST_STOOD_STILL;

/// A superbounce medal (docs/game/superbounce.md, "Medals"): the chain's
/// tiers, then Bounced, then Monster Bounce. Which bounce earns which is the chain's rule
/// (`game::superbounce_chain::bounce_medals`); this is how each shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Medal {
    /// Every bounce.
    Superbounce,
    /// The second bounce of a chain.
    DoubleBounce,
    /// The third.
    TripleBounce,
    /// The fourth.
    QuadBounce,
    /// The fifth.
    ChainBounce,
    /// The sixth.
    Bouncetacular,
    /// A fall out of the map during a bounce.
    Bounced,
    /// A bounce that launches well above a normal one, given with the
    /// bounce's Superbounce and chain medal. Last, so it is the line a
    /// Monster shows.
    Monster,
}

/// Every medal, in [`Medal`]'s order.
pub const MEDALS: [Medal; 8] = [
    Medal::Superbounce,
    Medal::DoubleBounce,
    Medal::TripleBounce,
    Medal::QuadBounce,
    Medal::ChainBounce,
    Medal::Bouncetacular,
    Medal::Bounced,
    Medal::Monster,
];

/// Each medal's glyph in the HUD message fonts: new art added at spare
/// private-use code points by the font rewrite (`font::medals`,
/// docs/game/hud-kit.md, "The medals"). New art changes the pictures, not
/// these.
pub const MEDAL_GLYPHS: [(Medal, char); 8] = [
    (Medal::Superbounce, '\u{e2a0}'),
    (Medal::DoubleBounce, '\u{e2a1}'),
    (Medal::TripleBounce, '\u{e2a2}'),
    (Medal::QuadBounce, '\u{e2a3}'),
    (Medal::ChainBounce, '\u{e2a4}'),
    (Medal::Bouncetacular, '\u{e2a5}'),
    (Medal::Bounced, '\u{e2a6}'),
    (Medal::Monster, '\u{e2a8}'),
];

/// The widest medal line: the most characters.
pub const MEDAL_LONGEST: &str = "\u{e2a8} Monster bounce!";

impl Medal {
    /// The id the film, Live and the agent API use (the canonical list is
    /// `live_proto::film::events::medals`).
    pub fn id(self) -> &'static str {
        match self {
            Medal::Superbounce => "superbounce",
            Medal::DoubleBounce => "double-bounce",
            Medal::TripleBounce => "triple-bounce",
            Medal::QuadBounce => "quad-bounce",
            Medal::ChainBounce => "chain-bounce",
            Medal::Bouncetacular => "bouncetacular",
            Medal::Bounced => "bounced",
            Medal::Monster => "monster-bounce",
        }
    }

    /// The medal with id `id`.
    pub fn from_id(id: &str) -> Option<Medal> {
        MEDALS.into_iter().find(|m| m.id() == id)
    }

    /// The words of its line, in sentence case with `!`.
    pub fn words(self) -> &'static str {
        match self {
            Medal::Superbounce => "Superbounce!",
            Medal::DoubleBounce => "Double bounce!",
            Medal::TripleBounce => "Triple bounce!",
            Medal::QuadBounce => "Quad bounce!",
            Medal::ChainBounce => "Chain bounce!",
            Medal::Bouncetacular => "Bouncetacular!",
            Medal::Bounced => "Bounced!",
            Medal::Monster => "Monster bounce!",
        }
    }

    /// Its glyph ([`MEDAL_GLYPHS`]).
    pub fn glyph(self) -> char {
        MEDAL_GLYPHS
            .iter()
            .find(|(m, _)| *m == self)
            .map_or('\u{e2a0}', |(_, c)| *c)
    }

    /// Its notification: the glyph, a space and the words, as a retail
    /// multi-kill line is.
    pub fn line(self) -> String {
        format!("{} {}", self.glyph(), self.words())
    }
}

/// The four tutorial steps, two lines each. The landing is the player's
/// own: the last step names the opt-in fall assist once, by its Controls
/// row, and no step asks for a button. The third is the widest.
pub const TUTORIAL_TEXT: [&str; 4] = [
    "Crouch, walk in under a lip,\nlet go, then walk out.",
    "You are primed.\nDo not crouch again.",
    "Follow the navpoint. Jump from the\nJump mark onto the Land mark.",
    "Jump or fall onto the strip.\nFall Assist (Controls) can steer.",
];

/// The scan of the map for strips.
#[derive(Clone, Debug, PartialEq)]
pub enum Scan {
    Scanning,
    Ready,
    /// Done, and the map has none.
    NoStrips,
    Failed,
}

/// Where the player is in the recipe, then the charge (docs/game/hud-kit.md,
/// "The charge meter").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Crouched within 1.5 units of a crouch spot, the standing capsule
    /// fits.
    Near,
    /// Crouched, the standing capsule does not fit, crouch held.
    Under,
    /// Crouch let go and the request still set: the forced crouch.
    Held,
    /// The word is -3.
    Primed,
    /// The word left -3 (shown at fill 0 for a second, then hidden).
    Lost,
}

/// The charge meter's phase and its fill, 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Charge {
    pub phase: Phase,
    pub fill: f32,
}

/// A verb on the prompt line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    HoldAndWalkUnder,
    LetGoOfCrouch,
    WalkOutToStand,
    /// Primed and still for half the time a body has before it sleeps.
    KeepMoving,
    /// A few seconds after a loss, until a recipe step begins, with why.
    ChargeAgain(Lost),
    /// Looking at a spot with a ghost run.
    ShowGhost,
    /// A ghost run is playing.
    StopGhost,
    /// [`Verb::ShowGhost`] with B's glyph (the Bumper Jumper layout).
    ShowGhostB,
    /// [`Verb::StopGhost`] with B's glyph (the Bumper Jumper layout).
    StopGhostB,
}

impl Verb {
    pub fn text(self) -> &'static str {
        match self {
            Verb::HoldAndWalkUnder => PROMPT_NEAR,
            Verb::LetGoOfCrouch => PROMPT_UNDER,
            Verb::WalkOutToStand => PROMPT_HELD,
            Verb::KeepMoving => PROMPT_KEEP_MOVING,
            Verb::ChargeAgain(_) => PROMPT_CHARGE_AGAIN,
            Verb::ShowGhost => PROMPT_SHOW_GHOST,
            Verb::StopGhost => PROMPT_STOP_GHOST,
            Verb::ShowGhostB => PROMPT_SHOW_GHOST_B,
            Verb::StopGhostB => PROMPT_STOP_GHOST_B,
        }
    }
}

/// Why the charge went (docs/game/superbounce.md, "Charge").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lost {
    /// The body stood still until it slept.
    StoodStill,
    /// A crouch while primed.
    Crouched,
    /// The word left -3 for another reason.
    Unknown,
}

/// What the navpoint is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The nearest crouch spot's stand point (not primed).
    Crouch,
    /// The chosen strip's stand point (primed): its number is the drop.
    Strip,
}

/// A notification. A landing has none: a bounce's line is its medal
/// ([`Medal`]), and why another did not bounce is the readout's line
/// ([`super::superbounce_readout::Miss`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Note {
    /// The arming bar came up: a recipe step began.
    Charging,
    Primed,
    ChargeLost(Lost),
}

impl Note {
    pub fn text(self) -> String {
        match self {
            Note::Charging => MESSAGE_CHARGING.into(),
            Note::Primed => MESSAGE_PRIMED.into(),
            Note::ChargeLost(Lost::StoodStill) => MESSAGE_LOST_STOOD_STILL.into(),
            Note::ChargeLost(Lost::Crouched) => MESSAGE_LOST_CROUCHED.into(),
            Note::ChargeLost(Lost::Unknown) => MESSAGE_LOST.into(),
        }
    }
}

/// The arming bar's target and how it gets there: a fill and its
/// [`Motion`], from [`arm_for`] or [`ArmTrack`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arm {
    pub fill: f32,
    pub motion: Motion,
}

/// The arming bar for a charge phase, one tick on its own: each recipe
/// step's fill, eased to; primed, the ending; lost, the blink; no charge,
/// none. [`ArmTrack`] adds the run out, which needs the tick before.
pub fn arm_for(phase: Option<Phase>) -> Option<Arm> {
    let arm = |fill, motion| Some(Arm { fill, motion });
    match phase? {
        Phase::Near => arm(FILL_NEAR, Motion::Ease),
        Phase::Under => arm(FILL_UNDER, Motion::Ease),
        Phase::Held => arm(FILL_HELD, Motion::Ease),
        Phase::Primed => arm(1.0, Motion::Ending),
        Phase::Lost => arm(0.0, Motion::Blink),
    }
}

/// The arming bar from tick to tick. A phase gives [`arm_for`]'s bar; no
/// charge after a recipe step is leaving early, the run out; no charge
/// after the ending, the blink or the run out keeps that bar, which hides
/// itself on its curve. The bar's motion is the runtime's to play on wall
/// time; this is per tick only.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ArmTrack {
    last: Option<Arm>,
}

impl ArmTrack {
    /// The bar for this tick's phase.
    pub fn update(&mut self, phase: Option<Phase>) -> Option<Arm> {
        let arm = match (arm_for(phase), self.last) {
            (Some(a), _) => Some(a),
            (
                None,
                Some(Arm {
                    motion: Motion::Ease | Motion::None,
                    ..
                }),
            ) => Some(Arm {
                fill: 0.0,
                motion: Motion::RunOut,
            }),
            (None, last) => last,
        };
        self.last = arm;
        arm
    }

    /// The mode went off: nothing to run out.
    pub fn clear(&mut self) {
        self.last = None;
    }
}

/// What the HUD shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub primed: bool,
    pub scan: Scan,
    /// The charge: the strip under the pill shows it primed and lost;
    /// `None` hides it.
    pub charge: Option<Charge>,
    /// The arming bar; `None` hides it.
    pub arm: Option<Arm>,
    /// The prompt line's verb; `None` leaves the line to the game.
    pub verb: Option<Verb>,
    /// The navpoint's target and its number in metres: the distance to a
    /// crouch spot, the drop onto a strip; `None` without one.
    pub nav: Option<(Target, f32)>,
    /// The latest notification and the tick it was raised at: a new tick
    /// queues it once.
    pub note: Option<(Note, u32)>,
    /// The latest medal line and the tick it was raised at, as `note`.
    pub medal: Option<(Medal, u32)>,
    /// The tutorial step shown, 1 to 4.
    pub tutorial: Option<u8>,
}

impl View {
    /// Not primed, the map scanned, nothing else.
    pub fn idle() -> Self {
        View {
            primed: false,
            scan: Scan::Ready,
            charge: None,
            arm: None,
            verb: None,
            nav: None,
            note: None,
            medal: None,
            tutorial: None,
        }
    }
}

/// A distance or drop as the navpoint shows it: whole metres, as the game's
/// counts are whole (the count fonts have no `.`), clamped to
/// [`DISTANCE_LONGEST`].
pub fn distance_text(m: f32) -> String {
    if m.is_nan() {
        return "0".into();
    }
    format!("{:.0}", m.clamp(0.0, 999.0))
}

/// A place of the pill's block: fullscreen `by` from the pill's top left
/// beside the motion tracker ([`PILL_FULL`]); in half and quarter screen
/// `split` from `origin` in the grenade row.
const fn pill_place(by: (i16, i16), split: (i16, i16), origin: Origin) -> Place {
    Place::new(Anchor::TopLeft, (PILL_FULL.0 + by.0, PILL_FULL.1 + by.1))
        .split(split, split)
        .split_from(origin)
}

/// The elements, in draw order.
pub fn hud() -> Hud {
    Hud {
        name: NAME,
        elements: vec![
            Element::icon(
                PILL,
                Art::GrenadePill(PillPart::Body),
                pill_place((0, 0), (0, 0), Origin::NextGrenadePill),
            ),
            // The right end, against the body's right edge.
            Element::icon(
                PILL_END,
                Art::GrenadePill(PillPart::End),
                pill_place((PILL_BODY, 0), (PILL_BODY, 0), Origin::NextGrenadePill),
            ),
            // Under the pill, as wide as it, in every mode the pill has.
            Element::meter(
                CHARGE,
                Art::StripMeterEnd(CHARGE_COLUMNS),
                pill_place((0, CHARGE_DOWN), (0, CHARGE_DOWN), Origin::NextGrenadePill),
            ),
            // Assault's arming bar, over the pill fullscreen and where
            // split screen has room; it gives way to retail's own in an
            // Assault game.
            Element::meter(
                ARM,
                Art::Widget {
                    hud: HudRef::Named(ARM_HUD),
                    name: ARM_WIDGET,
                },
                Place::new(Anchor::TopLeft, ARM_FULL)
                    .split(ARM_SPLIT, ARM_QUARTER)
                    .registered([0.5, 0.5]),
            )
            .hidden_on_d(ASSAULT_ARMING_D),
            // Centred where a pill's count is, as the counts are.
            Element::text(
                PILL_ICON,
                Font::Hud,
                Justify::Centre,
                ICON_TEXT,
                pill_place(ICON_AT, (0, 0), Origin::NextGrenadeCount),
            )
            .over(PILL),
            Element::channel(
                PROMPT,
                Kind::Prompt {
                    longest: PROMPT_LONGEST,
                },
            ),
            Element::channel(
                MESSAGE,
                Kind::Message {
                    longest: MESSAGE_LONGEST,
                },
            ),
            Element::channel(
                MEDAL,
                Kind::Message {
                    longest: MEDAL_LONGEST,
                },
            ),
            Element {
                at: Place::GAME.split((0, 0), (0, 0)),
                ..Element::channel(
                    NAVPOINT,
                    Kind::Navpoint {
                        longest: DISTANCE_LONGEST,
                    },
                )
            },
            Element::channel(
                TUTORIAL,
                Kind::Training {
                    longest: TUTORIAL_TEXT[2],
                },
            ),
        ],
    }
}

/// The values for `v`: every element of [`hud`], once.
pub fn values(v: &View) -> Values {
    let mut out = Values::default();
    let ready = v.scan == Scan::Ready;
    let pill = match v.scan {
        Scan::NoStrips | Scan::Failed => None,
        Scan::Scanning => Some(Style::Dim),
        Scan::Ready if v.primed => Some(Style::Normal),
        Scan::Ready => Some(Style::Inactive),
    };
    let charge = v.charge.filter(|_| ready);
    // The strip under the pill is the charge's time left: primed, and the
    // empty strip of a loss. The recipe is the arming bar's.
    let strip = charge.filter(|c| matches!(c.phase, Phase::Primed | Phase::Lost));
    // The icon flashes while the charge is about to go, and in the held
    // step (the next is to walk out).
    let icon_style = |style: Style| match charge {
        Some(c) if v.primed && c.phase == Phase::Primed && c.fill <= LOW_CHARGE => {
            Style::Flash(Pulse::TimeWarning)
        }
        Some(c) if !v.primed && c.phase == Phase::Held => Style::Flash(Pulse::Help),
        _ => style,
    };
    let (p, icon) = match pill {
        Some(style) => (
            Value::shown().styled(style),
            Value::text(ICON_TEXT).styled(icon_style(style)),
        ),
        None => (Value::hidden(), Value::hidden()),
    };
    out.set(PILL, p.clone());
    out.set(PILL_END, p);
    out.set(
        CHARGE,
        match strip {
            Some(c) => Value::fill(c.fill.clamp(0.0, 1.0)),
            None => Value::hidden(),
        },
    );
    out.set(
        ARM,
        match v.arm.filter(|_| ready) {
            Some(a) => Value::fill(a.fill.clamp(0.0, 1.0)).moving(a.motion),
            None => Value::hidden(),
        },
    );
    out.set(PILL_ICON, icon);
    out.set(
        PROMPT,
        match v.verb.filter(|_| ready) {
            Some(verb) => Value::text(verb.text()),
            None => Value::hidden(),
        },
    );
    out.set(
        MESSAGE,
        match v.note {
            Some((note, tick)) => Value {
                since_tick: tick,
                ..Value::text(note.text())
            },
            None => Value::hidden(),
        },
    );
    out.set(
        MEDAL,
        match v.medal {
            Some((medal, tick)) => Value {
                since_tick: tick,
                ..Value::text(medal.line())
            },
            None => Value::hidden(),
        },
    );
    out.set(
        NAVPOINT,
        match v.nav.filter(|_| ready) {
            Some((_, m)) => Value::text(distance_text(m)),
            None => Value::hidden(),
        },
    );
    out.set(
        TUTORIAL,
        match v.tutorial {
            Some(step @ 1..=4) => Value::text(TUTORIAL_TEXT[usize::from(step - 1)]),
            _ => Value::hidden(),
        },
    );
    out
}

/// The preview's states, by name, as [`View`]s.
pub fn views() -> Vec<(&'static str, View)> {
    let crouch = |m| Some((Target::Crouch, m));
    let strip = Some((Target::Strip, SAMPLE_DROP_M));
    let not_primed = View {
        nav: crouch(SAMPLE_DISTANCE_M),
        ..View::idle()
    };
    let step = |phase, fill, verb| View {
        charge: Some(Charge { phase, fill }),
        arm: arm_for(Some(phase)),
        verb: Some(verb),
        nav: crouch(1.0),
        ..View::idle()
    };
    let primed = |fill| View {
        primed: true,
        charge: Some(Charge {
            phase: Phase::Primed,
            fill,
        }),
        nav: strip,
        ..View::idle()
    };
    let full = View {
        note: Some((Note::Primed, 1)),
        ..primed(1.0)
    };
    let lost = |why, tick| View {
        charge: Some(Charge {
            phase: Phase::Lost,
            fill: 0.0,
        }),
        arm: arm_for(Some(Phase::Lost)),
        verb: Some(Verb::ChargeAgain(why)),
        note: Some((Note::ChargeLost(why), tick)),
        ..not_primed.clone()
    };
    let medal = |m, tick| View {
        medal: Some((m, tick)),
        ..primed(1.0)
    };
    let scanning = View {
        scan: Scan::Scanning,
        ..View::idle()
    };
    let tutorial = |v: &View, n| View {
        tutorial: Some(n),
        ..v.clone()
    };
    vec![
        ("not-primed", not_primed.clone()),
        (
            "near",
            View {
                note: Some((Note::Charging, 1)),
                ..step(Phase::Near, FILL_NEAR, Verb::HoldAndWalkUnder)
            },
        ),
        ("under", step(Phase::Under, FILL_UNDER, Verb::LetGoOfCrouch)),
        ("held", step(Phase::Held, FILL_HELD, Verb::WalkOutToStand)),
        (
            "primed-ending",
            View {
                arm: arm_for(Some(Phase::Primed)),
                ..full.clone()
            },
        ),
        (
            "left-early",
            View {
                arm: Some(Arm {
                    fill: 0.0,
                    motion: Motion::RunOut,
                }),
                ..not_primed.clone()
            },
        ),
        ("primed-full", full.clone()),
        ("primed-draining", primed(0.25)),
        (
            "primed-still-warning",
            View {
                verb: Some(Verb::KeepMoving),
                ..primed(LOW_CHARGE)
            },
        ),
        ("lost", lost(Lost::Unknown, 2)),
        ("lost-stood-still", lost(Lost::StoodStill, 5)),
        ("lost-crouched", lost(Lost::Crouched, 6)),
        ("bounced", medal(Medal::Superbounce, 3)),
        ("medal-double", medal(Medal::DoubleBounce, 7)),
        ("medal-triple", medal(Medal::TripleBounce, 8)),
        ("medal-quad", medal(Medal::QuadBounce, 9)),
        ("medal-chain", medal(Medal::ChainBounce, 10)),
        ("medal-bouncetacular", medal(Medal::Bouncetacular, 11)),
        (
            "medal-bounced",
            View {
                medal: Some((Medal::Bounced, 12)),
                ..not_primed.clone()
            },
        ),
        ("medal-monster", medal(Medal::Monster, 13)),
        ("scanning", scanning),
        (
            "no-strips",
            View {
                scan: Scan::NoStrips,
                ..View::idle()
            },
        ),
        ("tutorial-1", tutorial(&not_primed, 1)),
        ("tutorial-2", tutorial(&full, 2)),
        ("tutorial-3", tutorial(&full, 3)),
        ("tutorial-4", tutorial(&full, 4)),
    ]
}

/// The superbounce HUD's description.
pub struct Superbounce;

impl Describe for Superbounce {
    fn hud(&self) -> Hud {
        hud()
    }

    /// The recipe's steps, the charge, the landings, the scan, and the
    /// four tutorial steps.
    fn states(&self) -> Vec<(&'static str, Values)> {
        views().into_iter().map(|(n, v)| (n, values(&v))).collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("superbounce is a multiplayer game type")
    }
}

// ---- the tutorial ----

/// Tutorial steps.
pub const STEPS: u8 = 4;
/// A strip this close (metres) shows step 3.
pub const NEAR_M: f32 = 8.0;
/// Step 3 gives way to step 4 after this long without a bounce.
pub const STEP3_SECS: f64 = 20.0;
/// Step 4 closes after this long without a bounce.
pub const STEP4_SECS: f64 = 15.0;

/// What the tutorial watches, one tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Watch {
    pub primed: bool,
    /// Metres to the nearest strip.
    pub strip_m: Option<f32>,
    /// A bounce landed this tick.
    pub bounced: bool,
}

/// The tutorial's step machine: which step shows, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct Tutorial {
    /// Still to show this run.
    pending: bool,
    /// The step showing, 1..=4.
    step: Option<u8>,
    /// When the step showing came up, seconds on the caller's clock.
    since: f64,
}

/// What [`Tutorial::update`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// Now showing this step.
    Step(u8),
    /// The last step closed: the tutorial is done.
    Done,
}

impl Tutorial {
    /// `show`: the steps are to be shown the first time the mode is on.
    pub fn new(show: bool) -> Self {
        Self {
            pending: show,
            step: None,
            since: 0.0,
        }
    }

    /// The step showing.
    pub fn step(&self) -> Option<u8> {
        self.step
    }

    fn go(&mut self, step: u8, now: f64) -> Change {
        if step > STEPS {
            self.step = None;
            self.pending = false;
            return Change::Done;
        }
        self.step = Some(step);
        self.since = now;
        Change::Step(step)
    }

    /// One tick with the mode on (`watch`) or off (`None`), at `now`
    /// seconds. At most one change a call, so each step shows at least
    /// once.
    pub fn update(&mut self, watch: Option<Watch>, now: f64) -> Change {
        let Some(w) = watch else {
            return Change::None;
        };
        if !self.pending {
            return Change::None;
        }
        let age = now - self.since;
        match self.step {
            None => self.go(1, now),
            Some(1) if w.primed => self.go(2, now),
            Some(2) if w.strip_m.is_some_and(|m| m <= NEAR_M) => self.go(3, now),
            Some(3) if w.bounced || age >= STEP3_SECS => self.go(4, now),
            Some(4) if w.bounced || age >= STEP4_SECS => self.go(5, now),
            _ => Change::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(name: &str) -> Values {
        Superbounce
            .states()
            .into_iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1
    }

    fn style(s: &str, id: &str) -> Option<Style> {
        let v = state(s);
        let x = v.get(id).unwrap();
        x.shown.then_some(x.style)
    }

    #[test]
    fn the_distance_is_whole_metres() {
        assert_eq!(distance_text(12.4), "12");
        assert_eq!(distance_text(12.6), "13");
        assert_eq!(distance_text(4000.0), "999");
        assert_eq!(distance_text(f32::NAN), "0");
        assert!(!distance_text(3.4).contains('.'));
    }

    /// The pill is lit primed, inactive not primed, dim scanning, hidden
    /// with no strips; the navpoint shows over either target, hidden
    /// while scanning and without one.
    #[test]
    fn the_pill_and_the_navpoint_follow_the_state() {
        assert_eq!(style("primed-full", PILL), Some(Style::Normal));
        assert_eq!(style("not-primed", PILL_END), Some(Style::Inactive));
        assert_eq!(style("no-strips", PILL_END), None);
        assert_eq!(style("primed-full", PILL_ICON), Some(Style::Normal));
        assert_eq!(style("not-primed", PILL), Some(Style::Inactive));
        assert_eq!(style("scanning", PILL), Some(Style::Dim));
        assert_eq!(style("no-strips", PILL), None);
        assert_eq!(style("no-strips", PILL_ICON), None);
        assert_eq!(style("primed-full", NAVPOINT), Some(Style::Normal));
        assert_eq!(style("not-primed", NAVPOINT), Some(Style::Normal));
        assert_eq!(style("scanning", NAVPOINT), None);
        assert_eq!(style("no-strips", NAVPOINT), None);
        assert_eq!(state("primed-full").get(NAVPOINT).unwrap().text, "3");
        assert_eq!(state("under").get(NAVPOINT).unwrap().text, "1");
        assert_eq!(state("primed-full").get(PILL_ICON).unwrap().text, ICON_TEXT);
    }

    /// The strip's fill by phase (docs/game/hud-kit.md, "The charge
    /// meter"): primed and lost only, the recipe being the arming bar's;
    /// the icon's two flashes, and the verbs on the prompt line.
    #[test]
    fn the_charge_meter_and_the_prompts_follow_the_recipe() {
        let fill = |s: &str| {
            let v = state(s);
            let c = v.get(CHARGE).unwrap();
            c.shown.then_some(c.fill)
        };
        assert_eq!(fill("not-primed"), None);
        assert_eq!(fill("near"), None);
        assert_eq!(fill("under"), None);
        assert_eq!(fill("held"), None);
        assert_eq!(fill("primed-full"), Some(1.0));
        assert_eq!(fill("primed-draining"), Some(0.25));
        assert_eq!(fill("lost"), Some(0.0));
        assert_eq!(fill("lost-stood-still"), Some(0.0));
        assert_eq!(fill("primed-still-warning"), Some(LOW_CHARGE));
        assert_eq!(fill("scanning"), None);
        assert_eq!(
            style("primed-draining", PILL_ICON),
            Some(Style::Flash(Pulse::TimeWarning))
        );
        // The warning and the flash start together, at half.
        assert_eq!(
            style("primed-still-warning", PILL_ICON),
            Some(Style::Flash(Pulse::TimeWarning))
        );
        assert_eq!(style("held", PILL_ICON), Some(Style::Flash(Pulse::Help)));
        assert_eq!(style("under", PILL_ICON), Some(Style::Inactive));
        assert_eq!(style("primed-draining", PILL), Some(Style::Normal));
        let prompt = |s: &str| {
            let v = state(s);
            let p = v.get(PROMPT).unwrap();
            p.shown.then(|| p.text.clone())
        };
        assert_eq!(prompt("near").as_deref(), Some(PROMPT_NEAR));
        assert_eq!(prompt("under").as_deref(), Some(PROMPT_UNDER));
        assert_eq!(prompt("held").as_deref(), Some(PROMPT_HELD));
        assert_eq!(prompt("primed-full"), None);
        assert_eq!(prompt("primed-draining"), None);
        assert_eq!(
            prompt("primed-still-warning").as_deref(),
            Some(PROMPT_KEEP_MOVING)
        );
        // The hint alone: the reason is the loss's notification.
        for s in ["lost", "lost-stood-still", "lost-crouched"] {
            assert_eq!(prompt(s).as_deref(), Some(PROMPT_CHARGE_AGAIN), "{s}");
        }
        assert!(PROMPT_NEAR.contains(CROUCH) && PROMPT_UNDER.contains(CROUCH));
    }

    /// Each notification once, by its tick; a bounce's line is its medal.
    #[test]
    fn the_charge_is_notified() {
        let msg = |s: &str| {
            let v = state(s);
            let m = v.get(MESSAGE).unwrap();
            m.shown.then(|| (m.text.clone(), m.since_tick))
        };
        assert_eq!(msg("primed-full"), Some((MESSAGE_PRIMED.into(), 1)));
        assert_eq!(msg("near"), Some((MESSAGE_CHARGING.into(), 1)));
        assert_eq!(Note::Charging.text(), "Charging...");
        assert_eq!(Note::Primed.text(), "Primed!");
        assert_eq!(msg("lost"), Some((MESSAGE_LOST.into(), 2)));
        assert_eq!(
            msg("lost-stood-still"),
            Some((MESSAGE_LOST_STOOD_STILL.into(), 5))
        );
        assert_eq!(
            msg("lost-crouched"),
            Some((MESSAGE_LOST_CROUCHED.into(), 6))
        );
        assert_eq!(msg("primed-still-warning"), None);
        for m in [MESSAGE_LOST_STOOD_STILL, MESSAGE_LOST_CROUCHED] {
            assert!(m.starts_with(MESSAGE_LOST) && m.len() <= MESSAGE_LONGEST.len());
        }
        assert_eq!(msg("bounced"), None);
        assert_eq!(msg("under"), None);
        for m in [MESSAGE_CHARGING, MESSAGE_PRIMED] {
            assert!(crate::model::message_fault(m).is_none(), "{m}");
            assert!(m.len() <= MESSAGE_LONGEST.len(), "{m}");
        }
    }

    /// The ghost prompts, either button, fit the prompt line's one line of
    /// 63 units and differ only by the glyph.
    #[test]
    fn the_ghost_prompts_fit_the_line_with_either_button() {
        for (x, b) in [
            (PROMPT_SHOW_GHOST, PROMPT_SHOW_GHOST_B),
            (PROMPT_STOP_GHOST, PROMPT_STOP_GHOST_B),
        ] {
            assert_eq!(x.replace('\u{e102}', "\u{e101}"), b);
            assert!(!b.contains('\n'));
            assert!(b.encode_utf16().count() <= crate::model::MESSAGE_UNITS);
            assert!(b.chars().count() <= PROMPT_LONGEST.chars().count());
        }
        assert_eq!(Verb::ShowGhostB.text(), PROMPT_SHOW_GHOST_B);
        assert_eq!(Verb::StopGhostB.text(), PROMPT_STOP_GHOST_B);
    }

    /// A medal is its glyph, a space and its words, each under the game's
    /// 63 characters, in the states that show it and no other.
    #[test]
    fn a_medal_line_is_its_glyph_and_its_words() {
        let medal = |s: &str| {
            let v = state(s);
            let m = v.get(MEDAL).unwrap();
            m.shown.then(|| (m.text.clone(), m.since_tick))
        };
        assert_eq!(medal("bounced"), Some(("\u{e2a0} Superbounce!".into(), 3)));
        for (s, m) in [
            ("medal-double", Medal::DoubleBounce),
            ("medal-triple", Medal::TripleBounce),
            ("medal-quad", Medal::QuadBounce),
            ("medal-chain", Medal::ChainBounce),
            ("medal-bouncetacular", Medal::Bouncetacular),
            ("medal-bounced", Medal::Bounced),
            ("medal-monster", Medal::Monster),
        ] {
            assert_eq!(medal(s).map(|x| x.0), Some(m.line()), "{s}");
        }
        assert_eq!(medal("primed-full"), None);
        let glyphs: std::collections::HashSet<char> =
            MEDAL_GLYPHS.iter().map(|(_, c)| *c).collect();
        assert_eq!(glyphs.len(), MEDALS.len());
        for (i, m) in MEDALS.into_iter().enumerate() {
            assert_eq!(MEDAL_GLYPHS[i].0, m);
            let line = m.line();
            assert!(line.starts_with(m.glyph()) && line.ends_with('!'), "{line}");
            assert!(line.encode_utf16().count() <= crate::model::MESSAGE_UNITS);
            assert!(
                line.chars().count() <= MEDAL_LONGEST.chars().count(),
                "{line}"
            );
            assert_eq!(Medal::from_id(m.id()), Some(m));
        }
        assert_eq!(MEDAL_LONGEST, Medal::Monster.line());
    }

    /// No state carries a launch or drop number; the X button is only in
    /// the tutorial, the crouch button only on the prompt line.
    #[test]
    fn the_hud_has_no_numbers_but_the_distances() {
        let h = hud();
        let ids: Vec<_> = h.elements.iter().map(|e| e.id).collect();
        assert_eq!(
            ids,
            [
                PILL, PILL_END, CHARGE, ARM, PILL_ICON, PROMPT, MESSAGE, MEDAL, NAVPOINT, TUTORIAL
            ]
        );
        for (name, v) in Superbounce.states() {
            for (id, val) in &v.0 {
                if *id != TUTORIAL {
                    assert!(!val.text.contains('\u{e102}'), "{name} {id}");
                }
                if *id != PROMPT {
                    assert!(!val.text.contains(CROUCH), "{name} {id}");
                }
                if ![TUTORIAL, MESSAGE, MEDAL].contains(id) {
                    assert!(!val.text.contains('.'), "{name} {id}");
                }
            }
        }
        // The tutorial asks for no button: the fall assist is opt in.
        assert!(TUTORIAL_TEXT.iter().all(|t| !t.contains('\u{e102}')));
        assert!(TUTORIAL_TEXT[3].contains("Fall Assist"));
        // The layout is sized by the third step's text.
        let widest = |t: &str| t.lines().map(str::len).max().unwrap_or(0);
        assert!(
            TUTORIAL_TEXT
                .iter()
                .all(|t| widest(t) <= widest(TUTORIAL_TEXT[2]))
        );
    }

    #[test]
    fn every_state_is_one_layout_and_the_tutorial_shows_in_its_steps() {
        let h = hud();
        let states = Superbounce.states();
        let names: Vec<_> = states.iter().map(|s| s.0).collect();
        assert_eq!(
            names,
            [
                "not-primed",
                "near",
                "under",
                "held",
                "primed-ending",
                "left-early",
                "primed-full",
                "primed-draining",
                "primed-still-warning",
                "lost",
                "lost-stood-still",
                "lost-crouched",
                "bounced",
                "medal-double",
                "medal-triple",
                "medal-quad",
                "medal-chain",
                "medal-bouncetacular",
                "medal-bounced",
                "medal-monster",
                "scanning",
                "no-strips",
                "tutorial-1",
                "tutorial-2",
                "tutorial-3",
                "tutorial-4"
            ]
        );
        for (name, v) in &states {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
            let t = v.get(TUTORIAL).unwrap();
            assert_eq!(t.shown, name.starts_with("tutorial"), "{name}");
        }
        assert!(Superbounce.multiplayer_only().is_some());
    }

    /// Fullscreen the pill, its end, the strip and the icon are one block
    /// beside the motion tracker, under the arming bar, placed from the
    /// top-left anchor at the bottom left's points; in split screen they
    /// are the grenade row's next pill, as its count.
    #[test]
    fn the_pill_is_beside_the_tracker_fullscreen_and_in_the_grenade_row_in_split_screen() {
        let h = hud();
        let at = |id: &str| {
            let p = h.element(id).unwrap().at;
            assert_eq!((p.anchor, p.origin), (Anchor::TopLeft, Origin::Anchor), "{id}");
            assert_eq!(p.half, p.quarter, "{id}");
            // From the bottom-left corner, as the readout's rows are.
            ((p.full.0, p.full.1 - FRAME_HEIGHT), p.half.unwrap(), p.split_origin)
        };
        let pill = Some(Origin::NextGrenadePill);
        assert_eq!(at(PILL), ((90, -68), (0, 0), pill));
        assert_eq!(at(PILL_END), ((110, -68), (20, 0), pill));
        assert_eq!(at(CHARGE), ((90, -47), (0, 21), pill));
        assert_eq!(
            at(PILL_ICON),
            ((102, -69), (0, 0), Some(Origin::NextGrenadeCount))
        );
        assert!(h.uses_grenade_row());
        let p = h.element(PILL).unwrap().at;
        assert_eq!(p.origin_in(crate::Mode::Full), Origin::Anchor);
        assert_eq!(p.origin_in(crate::Mode::Half), Origin::NextGrenadePill);
        assert_eq!(p.origin_in(crate::Mode::Quarter), Origin::NextGrenadePill);
        // The bar's bottom (its centre plus half of 13) is above the
        // pill's top, and its left edge the pill's.
        let arm = h.element(ARM).unwrap().at;
        assert_eq!(arm.split_origin, None);
        assert!(arm.full.1 + 7 < PILL_FULL.1);
        assert_eq!(arm.full.0 - 70, PILL_FULL.0);
        assert_eq!(PILL_COLUMN, 33);
    }

    /// The arming bar: Assault's widget where retail draws it, giving way
    /// to retail's; the recipe's fills eased to, the ending primed, the
    /// run out on leaving, the blink on a loss; hidden elsewhere.
    #[test]
    fn the_arming_bar_shows_the_recipe() {
        let h = hud();
        let e = h.element(ARM).unwrap();
        assert_eq!(
            e.kind,
            Kind::Meter {
                art: Art::Widget {
                    hud: HudRef::Named("ui\\hud\\scoreboard"),
                    name: "bomb_defusal",
                },
            }
        );
        // The top-left anchor: fullscreen the block's point at the
        // bottom left; in split screen 35 above the frames' centres
        // ((320, 120) of 48..592 by 18..222, (160, 120) of 24..296), 7
        // left in a quarter.
        assert_eq!(e.at.anchor, Anchor::TopLeft);
        assert_eq!(
            (e.at.full, e.at.half, e.at.quarter),
            ((160, 325), Some((272, 67)), Some((129, 67)))
        );
        assert_eq!((48 + 160, 36 + 325), (208, 361));
        assert_eq!((48 + 272, 18 + 67), (320, 120 - 35));
        assert_eq!((24 + 129, 18 + 67), (160 - 7, 120 - 35));
        assert_eq!(e.at.registration, [0.5, 0.5]);
        assert_eq!(e.hide_when_d, 0x400);
        let arm = |s: &str| {
            let v = state(s);
            let a = v.get(ARM).unwrap();
            a.shown.then_some((a.fill, a.motion))
        };
        assert_eq!(arm("near"), Some((0.0, Motion::Ease)));
        assert_eq!(arm("under"), Some((1.0 / 3.0, Motion::Ease)));
        assert_eq!(arm("held"), Some((2.0 / 3.0, Motion::Ease)));
        assert_eq!(arm("primed-ending"), Some((1.0, Motion::Ending)));
        assert_eq!(arm("left-early"), Some((0.0, Motion::RunOut)));
        for s in ["lost", "lost-stood-still", "lost-crouched"] {
            assert_eq!(arm(s), Some((0.0, Motion::Blink)), "{s}");
        }
        for s in [
            "not-primed",
            "primed-full",
            "scanning",
            "no-strips",
            "tutorial-2",
        ] {
            assert_eq!(arm(s), None, "{s}");
        }
    }

    /// Tick to tick: a step eases, leaving a step runs out, the ending and
    /// the blink keep their bar (they hide themselves), a new charge starts
    /// again.
    #[test]
    fn the_arm_track_runs_out_only_after_a_step() {
        let mut t = ArmTrack::default();
        let m = |a: Option<Arm>| a.map(|a| (a.fill, a.motion));
        assert_eq!(m(t.update(None)), None);
        assert_eq!(m(t.update(Some(Phase::Near))), Some((0.0, Motion::Ease)));
        assert_eq!(
            m(t.update(Some(Phase::Under))),
            Some((FILL_UNDER, Motion::Ease))
        );
        assert_eq!(m(t.update(None)), Some((0.0, Motion::RunOut)), "left early");
        assert_eq!(
            m(t.update(None)),
            Some((0.0, Motion::RunOut)),
            "the same run out"
        );
        assert_eq!(
            m(t.update(Some(Phase::Held))),
            Some((FILL_HELD, Motion::Ease))
        );
        assert_eq!(
            m(t.update(Some(Phase::Primed))),
            Some((1.0, Motion::Ending))
        );
        assert_eq!(m(t.update(None)), Some((1.0, Motion::Ending)), "no run out");
        assert_eq!(m(t.update(Some(Phase::Lost))), Some((0.0, Motion::Blink)));
        assert_eq!(
            m(t.update(None)),
            Some((0.0, Motion::Blink)),
            "the loss's blink"
        );
        assert_eq!(m(t.update(Some(Phase::Near))), Some((0.0, Motion::Ease)));
        t.clear();
        assert_eq!(m(t.update(None)), None);
    }

    fn watch(primed: bool, strip_m: Option<f32>, bounced: bool) -> Option<Watch> {
        Some(Watch {
            primed,
            strip_m,
            bounced,
        })
    }

    #[test]
    fn the_tutorial_follows_what_the_player_does() {
        let mut t = Tutorial::new(true);
        assert_eq!(t.update(None, 0.0), Change::None, "waits for the mode");
        assert_eq!(t.update(watch(false, None, false), 1.0), Change::Step(1));
        assert_eq!(t.update(watch(false, None, false), 2.0), Change::None);
        assert_eq!(t.update(watch(true, None, false), 3.0), Change::Step(2));
        // Far from a strip: step 2 stays. Near one: step 3.
        assert_eq!(t.update(watch(true, Some(30.0), false), 4.0), Change::None);
        assert_eq!(
            t.update(watch(true, Some(6.0), false), 5.0),
            Change::Step(3)
        );
        // A bounce shows step 4; another closes it.
        assert_eq!(t.update(watch(true, Some(6.0), true), 6.0), Change::Step(4));
        assert_eq!(t.update(watch(true, Some(6.0), false), 7.0), Change::None);
        assert_eq!(t.update(watch(true, Some(6.0), true), 8.0), Change::Done);
        assert_eq!(t.step(), None);
        // Done stays done.
        assert_eq!(t.update(watch(true, Some(6.0), true), 9.0), Change::None);
    }

    #[test]
    fn steps_three_and_four_time_out_and_an_unshown_tutorial_never_shows() {
        let mut t = Tutorial::new(true);
        let near = watch(true, Some(1.0), false);
        assert_eq!(t.update(near, 0.0), Change::Step(1));
        assert_eq!(t.update(near, 0.1), Change::Step(2));
        assert_eq!(t.update(near, 0.2), Change::Step(3));
        assert_eq!(t.update(near, 10.0), Change::None);
        let four = 0.2 + STEP3_SECS + 0.01;
        assert_eq!(t.update(near, four), Change::Step(4));
        assert_eq!(t.update(near, four + 1.0), Change::None);
        assert_eq!(t.update(near, four + STEP4_SECS + 0.01), Change::Done);
        let mut off = Tutorial::new(false);
        assert_eq!(off.update(near, 0.0), Change::None);
        assert_eq!(off.step(), None);
    }
}
