//! The monitor's HUD (docs/game/forge-firefight-ui.md, "The monitor HUD"):
//! what the Forge monitor shows of its tool, as kit elements the game's own
//! HUD draws.
//!
//! - `forge.prompt`: the prompt line ([`Kind::Prompt`]), the one action A
//!   does ([`prompt_text`]): "Press (A) to pick up Beacon", "Press (X) for
//!   the object menu"; while a held thing is being turned, the value that
//!   is changing ("Heading 45").
//! - `forge.notice`: a notification ([`Kind::Message`]), sent once each,
//!   not published: a refusal, Saved, Loaded.
//! - `forge.teach`: the training box ([`Kind::Training`]): the first time a
//!   state is reached ([`Teach`]).
//! - `forge.budget.pill`, `.pill.end`, `.count`, `.meter`: a pill in the
//!   grenade row's first place with the objects left to place on it and the
//!   battery strip under it with the share used; the four flash the time
//!   warning from [`FLASH_FROM`].
//! - `forge.tool`: the tool's name, capitals, top right.
//! - `forge.rebuild.pill`, `.pill.end`, `.count`: the grenade row's second
//!   place, the edits Rebuild map has still to apply; hidden at 0.
//!
//! The reticle and the axis gizmo are not here: the monitor's reticle is
//! the compositor's `+` until a retail weapon's reticle widget is chosen
//! from the disc census (a text at the screen's centre is in the retail
//! reticle's reserved rectangle), and the gizmo and markers wait for the
//! kit's `Segment` and several navpoints (slices K4, K3, U6).
//!
//! A button is the font's glyph, read from the pad's table: [`Buttons`] is
//! what the game fills from `forge_controls::bindings::BINDINGS`, so no
//! string here names a button ([`Buttons::sample`] is the preview's, and
//! the game's test holds it to the table).
//!
//! [`View`] is what the HUD shows; the game (`game::forge_hud`) makes it
//! from the session each tick, so `forge.status` and the HUD agree.

use crate::model::{Describe, Hud, Pulse, Style, Value, Values};

use super::firefight::{COUNT_MAX, PILL_STEP};

pub const NAME: &str = "forge";

pub const PROMPT: &str = "forge.prompt";
pub const NOTICE: &str = "forge.notice";
pub const TEACH: &str = "forge.teach";
pub const BUDGET_PILL: &str = "forge.budget.pill";
pub const BUDGET_END: &str = "forge.budget.pill.end";
pub const BUDGET_COUNT: &str = "forge.budget.count";
pub const BUDGET_METER: &str = "forge.budget.meter";
pub const TOOL: &str = "forge.tool";
pub const REBUILD_PILL: &str = "forge.rebuild.pill";
pub const REBUILD_END: &str = "forge.rebuild.pill.end";
pub const REBUILD_COUNT: &str = "forge.rebuild.count";

/// The elements' ids, in draw order.
pub const IDS: [&str; 11] = [
    PROMPT,
    NOTICE,
    TEACH,
    BUDGET_PILL,
    BUDGET_END,
    BUDGET_COUNT,
    BUDGET_METER,
    TOOL,
    REBUILD_PILL,
    REBUILD_END,
    REBUILD_COUNT,
];

/// The widest budget count: the objects left, four digits (the LCD font has
/// no slash). More shows as [`BUDGET_COUNT_MAX`].
pub const BUDGET_LONGEST: &str = "9999";
pub const BUDGET_COUNT_MAX: u32 = 9999;
/// The rebuild count's widest: two digits, as a grenade count.
pub const REBUILD_LONGEST: &str = "99";
/// The widest tool name.
pub const TOOL_LONGEST: &str = "VOLUMES";
/// The widest notice.
pub const NOTICE_LONGEST: &str = "Item spawners are full (100) in this map.";

/// The strip's top below the pill's: the pill's 19 rows and 2 more.
pub const METER_DOWN: i16 = 21;
/// The budget used from which the pill and strip flash the time warning
/// (docs/game/forge-design.md: from 90%).
pub const FLASH_FROM: f32 = 0.9;
/// The widest name a prompt carries; a longer one is left out whole (no
/// text is cut with "...").
pub const NAME_MAX: usize = 24;
/// The widest prompt: the longest verb and a name of [`NAME_MAX`] units.
pub const PROMPT_LONGEST: &str = "Press \u{e100} to start a link from Assault Rifle Ammo Crate";
/// The widest teaching text, two lines.
pub const TEACH_LONGEST: &str =
    "Hold \u{e107} and use the right stick\nto turn what you are holding.";
/// The most pending edits the rebuild pill shows.
pub const REBUILD_COUNT_MAX: u32 = COUNT_MAX as u32;

/// The first step of the pill in the grenade row to the second: the
/// rebuild pill's place, as the Firefight HUD's second pill.
pub const REBUILD_STEP: i16 = PILL_STEP;

/// The tool the monitor holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Objects,
    Links,
}

impl Tool {
    pub const ALL: [Tool; 2] = [Tool::Objects, Tool::Links];

    /// The name in the HUD's top right.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Objects => "OBJECTS",
            Tool::Links => "LINKS",
        }
    }

    /// What the tool's hand can be doing.
    pub fn hands(self) -> &'static [Hand] {
        match self {
            Tool::Objects => &[Hand::Idle, Hand::Aimed, Hand::Placing, Hand::Holding],
            Tool::Links => &[Hand::Idle, Hand::Aimed, Hand::Linking],
        }
    }
}

/// What the monitor's hand is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    /// Nothing aimed at.
    Idle,
    /// A thing aimed at.
    Aimed,
    /// A new object held from the object menu.
    Placing,
    /// A placed thing picked up.
    Holding,
    /// The Links tool has a link's first end.
    Linking,
}

/// One control as the prompt names it: the font's glyph, and whether it is
/// held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cue {
    /// The glyph's code point (docs/input/button-icons.md).
    pub glyph: char,
    /// The control is held, not pressed.
    pub hold: bool,
}

impl Cue {
    /// "Press (A)" or "Hold (Y)".
    pub fn press(self) -> String {
        format!(
            "{} {}",
            if self.hold { "Hold" } else { "Press" },
            self.glyph
        )
    }
}

/// The controls the HUD names, each from the pad's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buttons {
    /// Pick up, drop or place.
    pub accept: Cue,
    /// The object menu or Properties.
    pub objects: Cue,
    /// Delete.
    pub delete: Cue,
    /// Moves a held object: turns it with the right stick.
    pub turn: Cue,
    /// The Forge menu.
    pub menu: Cue,
}

impl Buttons {
    /// Halo 3's layout as the table has it (the preview's; the game's test
    /// holds this to `BINDINGS`).
    pub fn sample() -> Buttons {
        let press = |glyph| Cue { glyph, hold: false };
        Buttons {
            accept: press('\u{e100}'),
            objects: press('\u{e102}'),
            delete: Cue {
                glyph: '\u{e103}',
                hold: true,
            },
            turn: Cue {
                glyph: '\u{e107}',
                hold: true,
            },
            menu: press('\u{e10c}'),
        }
    }
}

/// A first-use hint: the training box, shown the first time its state is
/// reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Teach {
    /// Aimed at a thing: hold Y to delete it.
    Delete,
    /// Holding a thing: hold RT to turn it.
    Turn,
    /// Nothing aimed at: Start's menu.
    Menu,
}

impl Teach {
    pub const ALL: [Teach; 3] = [Teach::Delete, Teach::Turn, Teach::Menu];

    /// The state that shows it: a hand and, for each, the tool's.
    pub fn of(tool: Tool, hand: Hand) -> Option<Teach> {
        match (tool, hand) {
            (Tool::Objects, Hand::Aimed) => Some(Teach::Delete),
            (Tool::Objects, Hand::Placing | Hand::Holding) => Some(Teach::Turn),
            (Tool::Objects, Hand::Idle) => Some(Teach::Menu),
            _ => None,
        }
    }

    /// The box's text: two lines at most.
    pub fn text(self, b: &Buttons) -> String {
        match self {
            Teach::Delete => format!(
                "{} to delete\nthe thing you are aiming at.",
                b.delete.press()
            ),
            Teach::Turn => format!(
                "{} and use the right stick\nto turn what you are holding.",
                b.turn.press()
            ),
            Teach::Menu => format!("{} for the Forge menu.", b.menu.press()),
        }
    }
}

/// What the HUD shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The monitor is up. Off, nothing shows.
    pub monitor: bool,
    pub tool: Tool,
    pub hand: Hand,
    /// What the hand has: the thing aimed at, held or being placed; in the
    /// Links tool, the thing aimed at. Empty with none.
    pub target: String,
    /// The objects placed and the most the map holds.
    pub used: u32,
    pub limit: u32,
    /// Edits that reach the game only through Rebuild map.
    pub rebuild: u32,
    /// The value a held thing is being changed by, as the prompt says it.
    pub changing: Option<String>,
    /// The hint to show now, if the state has one that was not shown yet.
    pub teach: Option<Teach>,
}

impl Default for View {
    fn default() -> View {
        View {
            monitor: false,
            tool: Tool::Objects,
            hand: Hand::Idle,
            target: String::new(),
            used: 0,
            limit: 0,
            rebuild: 0,
            changing: None,
            teach: None,
        }
    }
}

impl View {
    /// The objects left to place.
    pub fn left(&self) -> u32 {
        self.limit.saturating_sub(self.used)
    }

    /// The share of the budget used, 0 to 1; a map with none to give is
    /// full.
    pub fn share(&self) -> f32 {
        if self.limit == 0 {
            1.0
        } else {
            (self.used as f32 / self.limit as f32).clamp(0.0, 1.0)
        }
    }
}

/// `verb` and the name of what it is done to, when there is one the line
/// holds whole.
fn with_name(verb: String, name: &str) -> String {
    let units = name.encode_utf16().count();
    if name.is_empty() || units > NAME_MAX || name.contains(['\n', '\r']) {
        verb
    } else {
        format!("{verb} {name}")
    }
}

/// The prompt line for `v`: the one action A does, or the value being
/// changed. Never empty: every state of every tool has one.
pub fn prompt_text(v: &View, b: &Buttons) -> String {
    let a = b.accept.press();
    if let Some(c) = v
        .changing
        .as_ref()
        .filter(|c| !c.is_empty() && matches!(v.hand, Hand::Placing | Hand::Holding))
    {
        return c.clone();
    }
    match (v.tool, v.hand) {
        (Tool::Objects, Hand::Aimed) => with_name(format!("{a} to pick up"), &v.target),
        (Tool::Objects, Hand::Placing) => with_name(format!("{a} to place"), &v.target),
        (Tool::Objects, Hand::Holding) => with_name(format!("{a} to drop"), &v.target),
        (Tool::Objects, _) => format!("{} for the object menu", b.objects.press()),
        (Tool::Links, Hand::Aimed) => with_name(format!("{a} to start a link from"), &v.target),
        (Tool::Links, Hand::Linking) if v.target.is_empty() => "Aim at the thing to link to".into(),
        (Tool::Links, Hand::Linking) => with_name(format!("{a} to link to"), &v.target),
        (Tool::Links, _) => "Aim at a thing to link".into(),
    }
}

/// The elements' pieces hidden: no monitor.
pub fn hidden() -> Values {
    let mut out = Values::default();
    for id in IDS {
        out.set(id, Value::hidden());
    }
    out
}

/// A count as a pill shows it, at most `most`.
pub fn count_text(n: u32, most: u32) -> String {
    n.min(most).to_string()
}

/// The values for `v`. The notice is hidden: a notification is sent, not
/// published.
pub fn values(v: &View, b: &Buttons) -> Values {
    let mut out = hidden();
    if !v.monitor {
        return out;
    }
    out.set(PROMPT, Value::text(prompt_text(v, b)));
    if let Some(t) = v.teach {
        out.set(TEACH, Value::text(t.text(b)));
    }
    out.set(TOOL, Value::text(v.tool.name()));
    let flash = if v.share() >= FLASH_FROM {
        Style::Flash(Pulse::TimeWarning)
    } else {
        Style::Normal
    };
    out.set(BUDGET_PILL, Value::shown().styled(flash));
    out.set(BUDGET_END, Value::shown().styled(flash));
    out.set(
        BUDGET_COUNT,
        Value::text(count_text(v.left(), BUDGET_COUNT_MAX)).styled(flash),
    );
    out.set(BUDGET_METER, Value::fill(v.share()).styled(flash));
    if v.rebuild > 0 {
        out.set(REBUILD_PILL, Value::shown());
        out.set(REBUILD_END, Value::shown());
        out.set(
            REBUILD_COUNT,
            Value::text(count_text(v.rebuild, REBUILD_COUNT_MAX)),
        );
    }
    out
}

/// The elements, in draw order ([`IDS`]).
pub fn hud() -> Hud {
    crate::data::builtin(NAME)
}

/// The named states: each tool's hands, the value being changed, each hint,
/// the budget near and at its end, rebuilds pending, the widest, and the
/// monitor off.
pub fn views() -> Vec<(&'static str, View)> {
    let base = View {
        monitor: true,
        tool: Tool::Objects,
        hand: Hand::Idle,
        target: String::new(),
        used: 12,
        limit: 80,
        rebuild: 0,
        changing: None,
        teach: None,
    };
    let with = |f: &dyn Fn(&mut View)| {
        let mut v = base.clone();
        f(&mut v);
        v
    };
    let named = |v: &mut View, hand: Hand| {
        v.hand = hand;
        v.target = "Beacon".into();
    };
    vec![
        ("idle", base.clone()),
        ("aimed", with(&|v| named(v, Hand::Aimed))),
        ("placing", with(&|v| named(v, Hand::Placing))),
        ("holding", with(&|v| named(v, Hand::Holding))),
        (
            "turning",
            with(&|v| {
                named(v, Hand::Holding);
                v.changing = Some("Heading 45".into());
            }),
        ),
        (
            "links-idle",
            with(&|v| {
                v.tool = Tool::Links;
            }),
        ),
        (
            "links-aimed",
            with(&|v| {
                v.tool = Tool::Links;
                named(v, Hand::Aimed);
                v.target = "Teleporter sender 2".into();
            }),
        ),
        (
            "linking",
            with(&|v| {
                v.tool = Tool::Links;
                v.hand = Hand::Linking;
            }),
        ),
        (
            "linking-target",
            with(&|v| {
                v.tool = Tool::Links;
                v.hand = Hand::Linking;
                v.target = "Teleporter receiver 5".into();
            }),
        ),
        (
            "teach-delete",
            with(&|v| {
                named(v, Hand::Aimed);
                v.teach = Teach::of(v.tool, v.hand);
            }),
        ),
        (
            "teach-turn",
            with(&|v| {
                named(v, Hand::Holding);
                v.teach = Teach::of(v.tool, v.hand);
            }),
        ),
        ("teach-menu", with(&|v| v.teach = Teach::of(v.tool, v.hand))),
        ("refused", base.clone()),
        ("budget-low", with(&|v| v.used = 72)),
        ("budget-full", with(&|v| v.used = 80)),
        ("rebuild", with(&|v| v.rebuild = 2)),
        (
            "widest",
            with(&|v| {
                named(v, Hand::Aimed);
                v.target = "Assault Rifle Ammo Crate".into();
                v.used = 1;
                v.limit = u32::MAX;
                v.rebuild = u32::MAX;
            }),
        ),
        (
            "links-widest",
            with(&|v| {
                v.tool = Tool::Links;
                named(v, Hand::Aimed);
                v.target = "Assault Rifle Ammo Crate".into();
            }),
        ),
        ("off", View::default()),
    ]
}

/// The sample notice a state's preview shows: a refusal.
fn sample_notice(state: &str) -> Option<&'static str> {
    match state {
        "refused" | "widest" => Some(NOTICE_LONGEST),
        "budget-full" => Some("The object budget is full."),
        _ => None,
    }
}

/// The HUD's description.
pub struct Forge;

impl Describe for Forge {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        let b = Buttons::sample();
        views()
            .into_iter()
            .map(|(name, v)| {
                let mut out = values(&v, &b);
                if let Some(t) = sample_notice(name) {
                    out.set(NOTICE, Value::text(t));
                }
                (name, out)
            })
            .collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("Forge runs on multiplayer maps; a campaign player HUD may have no grenade row")
    }

    /// A monitor is no biped and no Firefight: never with the superbounce
    /// HUDs or the Firefight HUD. It shows with the Forge readout, whose
    /// rows are clear of its places.
    fn never_with(&self) -> &'static [&'static str] {
        &[
            super::superbounce::NAME,
            super::superbounce_readout::NAME,
            super::firefight::NAME,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Origin, banner_fault, message_fault};

    fn units(s: &str) -> usize {
        s.encode_utf16().count()
    }

    fn shown(v: &Values, id: &str) -> Option<String> {
        v.get(id).filter(|x| x.shown).map(|x| x.text.clone())
    }

    /// Every state of every tool has exactly one prompt, within the
    /// element's longest, with the button as the glyph and none written in
    /// the string.
    #[test]
    fn every_tool_state_has_one_prompt() {
        let b = Buttons::sample();
        for tool in Tool::ALL {
            for &hand in tool.hands() {
                for target in ["", "Beacon", "Assault Rifle Ammo Crate"] {
                    let v = View {
                        monitor: true,
                        tool,
                        hand,
                        target: target.into(),
                        used: 3,
                        limit: 10,
                        ..View::default()
                    };
                    let text = prompt_text(&v, &b);
                    assert!(!text.is_empty(), "{tool:?} {hand:?} {target:?}");
                    assert!(!text.contains('\n'), "{text}");
                    assert!(units(&text) <= units(PROMPT_LONGEST), "{text}");
                    // One prompt: the one element, shown.
                    let out = values(&v, &b);
                    assert_eq!(shown(&out, PROMPT).as_deref(), Some(text.as_str()));
                    // The button is a glyph from the table, not a letter.
                    if text.starts_with("Press") || text.starts_with("Hold") {
                        let g = text.chars().find(|c| *c >= '\u{e000}').unwrap();
                        assert!(('\u{e100}'..='\u{e111}').contains(&g), "{text}");
                    }
                }
            }
        }
    }

    /// The copy, state by state, as the design has it.
    #[test]
    fn the_prompts_read_as_the_game_does() {
        let b = Buttons::sample();
        let of = |tool, hand, target: &str, changing: Option<&str>| {
            prompt_text(
                &View {
                    monitor: true,
                    tool,
                    hand,
                    target: target.into(),
                    changing: changing.map(Into::into),
                    ..View::default()
                },
                &b,
            )
        };
        use Hand::*;
        use Tool::*;
        assert_eq!(
            of(Objects, Idle, "", None),
            "Press \u{e102} for the object menu"
        );
        assert_eq!(
            of(Objects, Aimed, "Beacon", None),
            "Press \u{e100} to pick up Beacon"
        );
        assert_eq!(
            of(Objects, Placing, "Beacon", None),
            "Press \u{e100} to place Beacon"
        );
        assert_eq!(
            of(Objects, Holding, "Beacon", None),
            "Press \u{e100} to drop Beacon"
        );
        // A hold being turned says the value that changes.
        assert_eq!(
            of(Objects, Holding, "Beacon", Some("Heading 45")),
            "Heading 45"
        );
        // A value changing with nothing in the hand is not the prompt.
        assert_eq!(
            of(Objects, Aimed, "Beacon", Some("Heading 45")),
            "Press \u{e100} to pick up Beacon"
        );
        // A name too long for the line is left out whole.
        let long = "Assault Rifle Ammo Cache Crate";
        assert_eq!(of(Objects, Aimed, long, None), "Press \u{e100} to pick up");
        assert_eq!(
            of(Links, Aimed, "Gate", None),
            "Press \u{e100} to start a link from Gate"
        );
        assert_eq!(of(Links, Linking, "", None), "Aim at the thing to link to");
        assert_eq!(
            of(Links, Linking, "Lift", None),
            "Press \u{e100} to link to Lift"
        );
        assert_eq!(of(Links, Idle, "", None), "Aim at a thing to link");
    }

    /// A cue names its glyph and whether it is held.
    #[test]
    fn a_cue_says_press_or_hold() {
        let b = Buttons::sample();
        assert_eq!(b.accept.press(), "Press \u{e100}");
        assert_eq!(b.delete.press(), "Hold \u{e103}");
        assert_eq!(b.turn.press(), "Hold \u{e107}");
    }

    /// The pill counts what is left, the strip what is used, both flash
    /// from 90%, and the rebuild pill shows only with edits pending.
    #[test]
    fn the_budget_and_the_rebuilds_follow_the_view() {
        let b = Buttons::sample();
        let mut v = View {
            monitor: true,
            used: 12,
            limit: 80,
            ..View::default()
        };
        let out = values(&v, &b);
        assert_eq!(shown(&out, BUDGET_COUNT).as_deref(), Some("68"));
        assert_eq!(out.get(BUDGET_METER).unwrap().fill, 12.0 / 80.0);
        for id in [BUDGET_PILL, BUDGET_END, BUDGET_COUNT, BUDGET_METER] {
            assert_eq!(out.get(id).unwrap().style, Style::Normal, "{id}");
        }
        for id in [REBUILD_PILL, REBUILD_END, REBUILD_COUNT] {
            assert!(!out.get(id).unwrap().shown, "{id}: nothing pending");
        }
        v.used = 72;
        let low = values(&v, &b);
        for id in [BUDGET_PILL, BUDGET_END, BUDGET_COUNT, BUDGET_METER] {
            assert_eq!(
                low.get(id).unwrap().style,
                Style::Flash(Pulse::TimeWarning),
                "{id}"
            );
        }
        v.used = 71;
        assert_eq!(
            values(&v, &b).get(BUDGET_METER).unwrap().style,
            Style::Normal,
            "71 of 80 is under 90%"
        );
        v.used = 90;
        let over = values(&v, &b);
        assert_eq!(shown(&over, BUDGET_COUNT).as_deref(), Some("0"));
        assert_eq!(over.get(BUDGET_METER).unwrap().fill, 1.0);
        v.limit = 0;
        v.used = 0;
        assert_eq!(values(&v, &b).get(BUDGET_METER).unwrap().fill, 1.0);
        v.rebuild = 2;
        let pending = values(&v, &b);
        assert_eq!(shown(&pending, REBUILD_COUNT).as_deref(), Some("2"));
        assert!(pending.get(REBUILD_PILL).unwrap().shown);
        v.rebuild = 500;
        assert_eq!(shown(&values(&v, &b), REBUILD_COUNT).as_deref(), Some("99"));
        v.limit = u32::MAX;
        assert_eq!(
            shown(&values(&v, &b), BUDGET_COUNT).as_deref(),
            Some("9999")
        );
    }

    /// With the monitor off nothing shows and the layout is the same.
    #[test]
    fn the_monitor_off_shows_nothing() {
        let b = Buttons::sample();
        let off = values(&View::default(), &b);
        for (id, v) in &off.0 {
            assert!(!v.shown, "{id}");
        }
        assert_eq!(off, hidden());
        assert!(!off.get(TOOL).unwrap().shown);
    }

    /// Each hint is two lines at most, its lines within the widest, and one
    /// state has one.
    #[test]
    fn each_hint_fits_the_training_box() {
        let b = Buttons::sample();
        let widest = TEACH_LONGEST.lines().map(units).max().unwrap();
        for t in Teach::ALL {
            let text = t.text(&b);
            assert!(text.lines().count() <= 2, "{text}");
            for line in text.lines() {
                assert!(units(line) <= widest, "{line}");
            }
            // The state that has it is the one that says it.
            assert!(
                Tool::ALL
                    .iter()
                    .flat_map(|t| t.hands().iter().map(|h| (*t, *h)))
                    .any(|(tool, hand)| Teach::of(tool, hand) == Some(t)),
                "{t:?}"
            );
        }
        assert_eq!(Teach::of(Tool::Links, Hand::Aimed), None);
    }

    /// The notices and the banner-sized words the sheet shows are lines the
    /// game keeps whole.
    #[test]
    fn the_notices_are_one_line() {
        for t in [
            NOTICE_LONGEST,
            "The object budget is full.",
            "Fine movement on",
            "Saved Courtyard.",
        ] {
            assert_eq!(message_fault(t), None, "{t}");
        }
        assert_eq!(banner_fault("OBJECTS"), None);
        assert!(units(TOOL_LONGEST) >= units(Tool::Objects.name()));
        assert!(units(TOOL_LONGEST) >= units(Tool::Links.name()));
    }

    /// The layout is one in every state, the YAML's, and the pills are the
    /// grenade row's first place and the one after, a kit pill's width and
    /// the gap apart.
    #[test]
    fn the_pills_are_the_grenade_rows_first_places() {
        let h = hud();
        assert_eq!(h.problems(), Vec::<String>::new());
        assert_eq!(h.elements.iter().map(|e| e.id).collect::<Vec<_>>(), IDS);
        assert!(h.uses_grenade_row() && h.has_text());
        let at = |id: &str| {
            let p = h.element(id).unwrap().at;
            assert_eq!(p.full, p.half.unwrap(), "{id}");
            assert_eq!(p.full, p.quarter.unwrap(), "{id}");
            (p.origin, p.full)
        };
        let (pill, count) = (Origin::NextGrenadePill, Origin::NextGrenadeCount);
        assert_eq!(at(BUDGET_PILL), (pill, (0, 0)));
        assert_eq!(at(BUDGET_END), (pill, (20, 0)));
        assert_eq!(at(BUDGET_COUNT), (count, (0, 0)));
        assert_eq!(at(BUDGET_METER), (pill, (0, METER_DOWN)));
        assert_eq!(at(REBUILD_PILL), (pill, (REBUILD_STEP, 0)));
        assert_eq!(at(REBUILD_END), (pill, (REBUILD_STEP + 20, 0)));
        assert_eq!(at(REBUILD_COUNT), (count, (REBUILD_STEP, 0)));
        assert_eq!(h.element(BUDGET_COUNT).unwrap().over, Some(BUDGET_PILL));
        assert_eq!(h.element(REBUILD_COUNT).unwrap().over, Some(REBUILD_PILL));
        for id in [PROMPT, NOTICE, TEACH] {
            assert!(h.element(id).unwrap().kind.is_channel(), "{id}");
        }
        for (name, v) in Forge.states() {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
        }
        assert_eq!(hidden().layout_problems(&h), Vec::<String>::new());
        assert!(Forge.multiplayer_only().is_some());
        for other in [
            super::super::superbounce::NAME,
            super::super::firefight::NAME,
        ] {
            assert!(Forge.never_with().contains(&other), "{other}");
        }
        assert!(
            !Forge
                .never_with()
                .contains(&super::super::forge_readout::NAME),
            "the monitor shows the readout"
        );
    }

    /// Every named state's prompt, in every state, is within the longest;
    /// the widest state is the widest.
    #[test]
    fn every_named_state_fits_its_longest() {
        let b = Buttons::sample();
        for (name, v) in views() {
            let out = values(&v, &b);
            if let Some(p) = shown(&out, PROMPT) {
                assert!(units(&p) <= units(PROMPT_LONGEST), "{name}: {p}");
            } else {
                assert!(!v.monitor, "{name}");
            }
            if let Some(t) = shown(&out, TEACH) {
                for line in t.lines() {
                    assert!(
                        units(line) <= TEACH_LONGEST.lines().map(units).max().unwrap(),
                        "{name}"
                    );
                }
            }
        }
    }
}
