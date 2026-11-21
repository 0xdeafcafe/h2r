//! The superbounce HUD on the kit (docs/game/hud-kit.md, "The superbounce
//! HUD on the kit"), made of things Halo 2 already draws:
//!
//! - a pill in the grenade row, the next one after the plasma and frag
//!   pills: a short pill cut from the frag pill's own art (its body and
//!   its right end, without the frag "x" and picture), with a chevron
//!   where a pill's count sits: lit while primed, inactive while not, dim
//!   while the map is scanned, hidden on a map with no strips;
//! - the charge meter under the pill, the energy sword's battery strip
//!   cropped to the pill's width: how far the player is through the
//!   recipe, then how long the charge has left;
//! - the recipe's verbs on the game's prompt line, the bound button drawn
//!   by the game;
//! - the results (primed, lost, each landing on a strip that did not
//!   bounce) and the medals (a bounce's medal glyph and line, as a retail
//!   multi-kill's) in the game's notification list;
//! - a navpoint over the next thing to do, the nearest crouch spot while
//!   not primed and the nearest strip while primed, the game's arrow
//!   pointing down at it and its waypoint marker above, the distance in
//!   whole metres over them;
//! - the tutorial in the game's training box, and its step machine
//!   ([`Tutorial`]).
//!
//! [`View`] is what the HUD shows of `superbounce_mode`'s state; the
//! game's runtime (`game::superbounce_hud`) fills it from `State` each
//! tick. Launch and drop are not on the HUD: they are on the strips'
//! labels in the world and in the agent API.

use crate::model::{
    self, Art, Describe, Element, Font, Hud, Justify, Kind, Origin, PILL_BODY, PillPart, Place,
    Pulse, Style, Value, Values,
};

pub const NAME: &str = "superbounce";

pub const PILL: &str = "sb.pill";
pub const PILL_END: &str = "sb.pill.end";
pub const PILL_ICON: &str = "sb.pill.icon";
pub const CHARGE: &str = "sb.charge";
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
/// The distance the preview's navpoints show.
pub const SAMPLE_DISTANCE_M: f32 = 12.0;

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
/// The fill of each recipe step before the word is -3
/// (docs/game/hud-kit.md, "The charge meter").
pub const FILL_NEAR: f32 = 0.0;
pub const FILL_UNDER: f32 = 1.0 / 3.0;
pub const FILL_HELD: f32 = 2.0 / 3.0;

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
/// (`game::superbounce_ghost`): the button is d-pad up (U+E108), which no
/// Halo 2 layout uses in a game.
pub const PROMPT_SHOW_GHOST: &str = "Press \u{e108} to show ghost";
pub const PROMPT_STOP_GHOST: &str = "Press \u{e108} to stop ghost";
/// The prompt line's widest verb: the most characters.
pub const PROMPT_LONGEST: &str = PROMPT_CHARGE_AGAIN;

/// The notifications.
pub const MESSAGE_PRIMED: &str = "Primed";
pub const MESSAGE_LOST: &str = "Charge lost";
pub const MESSAGE_LOST_STOOD_STILL: &str = "Charge lost: you stood still";
pub const MESSAGE_LOST_CROUCHED: &str = "Charge lost: you crouched";
pub const MESSAGE_NOT_PRIMED: &str = "Not primed.";
pub const MESSAGE_MISSED: &str = "Missed the strip.";
pub const MESSAGE_TOO_SLOW: &str = "Too slow.";
pub const MESSAGE_NO_BOUNCE: &str = "No bounce.";
/// The widest notification: the most characters, all of them plain.
pub const MESSAGE_LONGEST: &str = MESSAGE_LOST_STOOD_STILL;

/// A superbounce medal (docs/game/superbounce.md, "Medals"): the chain's
/// tiers, then Bounced. Which bounce earns which is the chain's rule
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
}

/// Every medal, in [`Medal`]'s order.
pub const MEDALS: [Medal; 7] = [
    Medal::Superbounce,
    Medal::DoubleBounce,
    Medal::TripleBounce,
    Medal::QuadBounce,
    Medal::ChainBounce,
    Medal::Bouncetacular,
    Medal::Bounced,
];

/// Each medal's glyph in the HUD message fonts: new art added at spare
/// private-use code points by the font rewrite (`font::medals`,
/// docs/game/hud-kit.md, "The medals"). New art changes the pictures, not
/// these.
pub const MEDAL_GLYPHS: [(Medal, char); 7] = [
    (Medal::Superbounce, '\u{e2a0}'),
    (Medal::DoubleBounce, '\u{e2a1}'),
    (Medal::TripleBounce, '\u{e2a2}'),
    (Medal::QuadBounce, '\u{e2a3}'),
    (Medal::ChainBounce, '\u{e2a4}'),
    (Medal::Bouncetacular, '\u{e2a5}'),
    (Medal::Bounced, '\u{e2a6}'),
];

/// The widest medal line: the most characters.
pub const MEDAL_LONGEST: &str = "\u{e2a5} Bouncetacular!";

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
    "Find a strip. Its post is the height\nto fall from. No post: just jump.",
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
    /// The nearest strip's stand point (primed).
    Strip,
}

/// How a landing within 0.5 of a strip that did not bounce ended, by its
/// first failed test (docs/game/superbounce.md, "Landings"). A bounce's
/// line is its medal ([`Medal`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Landed {
    NotPrimed,
    Missed,
    TooSlow,
    NoBounce,
}

/// A notification.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Note {
    Primed,
    ChargeLost(Lost),
    Landed(Landed),
}

impl Note {
    pub fn text(self) -> String {
        match self {
            Note::Primed => MESSAGE_PRIMED.into(),
            Note::ChargeLost(Lost::StoodStill) => MESSAGE_LOST_STOOD_STILL.into(),
            Note::ChargeLost(Lost::Crouched) => MESSAGE_LOST_CROUCHED.into(),
            Note::ChargeLost(Lost::Unknown) => MESSAGE_LOST.into(),
            Note::Landed(Landed::NotPrimed) => MESSAGE_NOT_PRIMED.into(),
            Note::Landed(Landed::Missed) => MESSAGE_MISSED.into(),
            Note::Landed(Landed::TooSlow) => MESSAGE_TOO_SLOW.into(),
            Note::Landed(Landed::NoBounce) => MESSAGE_NO_BOUNCE.into(),
        }
    }
}

/// What the HUD shows.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub primed: bool,
    pub scan: Scan,
    /// The charge meter; `None` hides it.
    pub charge: Option<Charge>,
    /// The prompt line's verb; `None` leaves the line to the game.
    pub verb: Option<Verb>,
    /// The navpoint's target and the metres to it; `None` without one.
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
            verb: None,
            nav: None,
            note: None,
            medal: None,
            tutorial: None,
        }
    }
}

/// A distance as the navpoint shows it: whole metres, as the game's
/// counts are whole (the count fonts have no `.`), clamped to
/// [`DISTANCE_LONGEST`].
pub fn distance_text(m: f32) -> String {
    if m.is_nan() {
        return "0".into();
    }
    format!("{:.0}", m.clamp(0.0, 999.0))
}

/// The elements, in draw order.
pub fn hud() -> Hud {
    Hud {
        name: NAME,
        elements: vec![
            Element::icon(
                PILL,
                Art::GrenadePill(PillPart::Body),
                Place::in_grenade_row(Origin::NextGrenadePill),
            ),
            // The right end, against the body's right edge.
            Element::icon(
                PILL_END,
                Art::GrenadePill(PillPart::End),
                Place {
                    full: (PILL_BODY, 0),
                    half: Some((PILL_BODY, 0)),
                    quarter: Some((PILL_BODY, 0)),
                    ..Place::in_grenade_row(Origin::NextGrenadePill)
                },
            ),
            // Under the pill, as wide as it, in every mode the pill has.
            Element::meter(
                CHARGE,
                Art::StripMeterEnd(CHARGE_COLUMNS),
                Place {
                    full: (0, CHARGE_DOWN),
                    half: Some((0, CHARGE_DOWN)),
                    quarter: Some((0, CHARGE_DOWN)),
                    ..Place::in_grenade_row(Origin::NextGrenadePill)
                },
            ),
            // Centred where a pill's count is, as the counts are.
            Element::text(
                PILL_ICON,
                Font::Hud,
                Justify::Centre,
                ICON_TEXT,
                Place::in_grenade_row(Origin::NextGrenadeCount),
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
        match charge {
            Some(c) => Value::fill(c.fill.clamp(0.0, 1.0)),
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
    let strip = Some((Target::Strip, SAMPLE_DISTANCE_M));
    let not_primed = View {
        nav: crouch(SAMPLE_DISTANCE_M),
        ..View::idle()
    };
    let step = |phase, fill, verb| View {
        charge: Some(Charge { phase, fill }),
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
        ("near", step(Phase::Near, FILL_NEAR, Verb::HoldAndWalkUnder)),
        ("under", step(Phase::Under, FILL_UNDER, Verb::LetGoOfCrouch)),
        ("held", step(Phase::Held, FILL_HELD, Verb::WalkOutToStand)),
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
        (
            "missed",
            View {
                note: Some((Note::Landed(Landed::Missed), 4)),
                ..primed(0.8)
            },
        ),
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
        assert_eq!(state("primed-full").get(NAVPOINT).unwrap().text, "12");
        assert_eq!(state("under").get(NAVPOINT).unwrap().text, "1");
        assert_eq!(state("primed-full").get(PILL_ICON).unwrap().text, ICON_TEXT);
    }

    /// The meter's fill by phase (docs/game/hud-kit.md, "The charge
    /// meter"), the icon's two flashes, and the verbs on the prompt line.
    #[test]
    fn the_charge_meter_and_the_prompts_follow_the_recipe() {
        let fill = |s: &str| {
            let v = state(s);
            let c = v.get(CHARGE).unwrap();
            c.shown.then_some(c.fill)
        };
        assert_eq!(fill("not-primed"), None);
        assert_eq!(fill("near"), Some(0.0));
        assert_eq!(fill("under"), Some(1.0 / 3.0));
        assert_eq!(fill("held"), Some(2.0 / 3.0));
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
        assert_eq!(style("missed", PILL_ICON), Some(Style::Normal));
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
    fn the_landings_and_the_charge_are_notified() {
        let msg = |s: &str| {
            let v = state(s);
            let m = v.get(MESSAGE).unwrap();
            m.shown.then(|| (m.text.clone(), m.since_tick))
        };
        assert_eq!(msg("primed-full"), Some((MESSAGE_PRIMED.into(), 1)));
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
        for m in [MESSAGE_MISSED, MESSAGE_NOT_PRIMED] {
            assert!(m.len() <= MESSAGE_LONGEST.len(), "{m}");
        }
        assert_eq!(msg("bounced"), None);
        assert_eq!(msg("missed"), Some((MESSAGE_MISSED.into(), 4)));
        assert_eq!(msg("near"), None);
        for (n, t) in [
            (Landed::NotPrimed, MESSAGE_NOT_PRIMED),
            (Landed::TooSlow, MESSAGE_TOO_SLOW),
            (Landed::NoBounce, MESSAGE_NO_BOUNCE),
        ] {
            assert_eq!(Note::Landed(n).text(), t);
        }
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
        ] {
            assert_eq!(medal(s).map(|x| x.0), Some(m.line()), "{s}");
        }
        assert_eq!(medal("missed"), None);
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
        assert_eq!(MEDAL_LONGEST, Medal::Bouncetacular.line());
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
                PILL, PILL_END, CHARGE, PILL_ICON, PROMPT, MESSAGE, MEDAL, NAVPOINT, TUTORIAL
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
                "missed",
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
