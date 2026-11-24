//! The monitor's controls: one table, [`BINDINGS`], Halo 3's layout
//! (docs/game/forge-editing.md "Controls"). The pad gate, the keyboard map,
//! the Controls page and the HUD's prompts all read it, so none can
//! disagree: change a row here and they follow.
//!
//! Start is a row that does nothing of ours ([`Does::Game`]): it reaches
//! the game, which opens its own GAME MENU with the Forge rows in it
//! (`crate::forge_screens::game_menu`). Escape is B's key, and reaches the
//! game the same way when there is nothing to cancel.

use std::time::Duration;

use forge::tool::Action;
use input::forge::Key;
use input::host::HostButton;

/// A press the monitor acts on once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Do(Action),
    /// The object menu, or Properties when an object is aimed at or held
    /// (Halo 3's X). In the Links tool it opens the aimed thing's Link
    /// dialog the same way.
    ObjectsOrProperties,
    /// Fine movement, on or off.
    Fine,
}

/// A control read while it is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    Rise,
    Fall,
    /// Flying is faster ([`THRUST`]).
    Thrust,
    /// The sticks move the held object: the left pushes it away, pulls it
    /// in and rolls it; the right turns it.
    Manipulate,
}

/// A stick, or the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Fly,
    Look,
}

/// What a row of the table does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Does {
    Act(Gesture),
    Hold(Held),
    Move(Motion),
    /// Reaches the game: Start's GAME MENU.
    Game,
}

/// A trigger, or a stick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Where on the pad a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pad {
    /// A press of the button. When the same button also has a [`Pad::Hold`]
    /// row, a release before [`HOLD`].
    Tap(HostButton),
    /// The button held for [`HOLD`]; fires once.
    Hold(HostButton),
    /// While the button is down.
    Down(HostButton),
    /// While the trigger is past [`TRIGGER`].
    Trigger(Side),
    Stick(Side),
}

impl Pad {
    pub const fn button(self) -> Option<HostButton> {
        match self {
            Pad::Tap(b) | Pad::Hold(b) | Pad::Down(b) => Some(b),
            Pad::Trigger(_) | Pad::Stick(_) => None,
        }
    }
}

/// One control: what it does, its name on the Controls page, its place on
/// the pad, the keys that send it and how the page names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub does: Does,
    pub name: &'static str,
    pub pad: Pad,
    /// The [`Key`]s that send it (none: the mouse, by `mouse`).
    pub keys: &'static [Key],
    pub mouse: &'static str,
}

/// How long a press is held before it counts as a hold.
pub const HOLD: Duration = Duration::from_millis(450);
/// How far a trigger is pulled before it counts as held.
pub const TRIGGER: f32 = 0.5;
/// What thrust multiplies flying by.
pub const THRUST: f32 = 3.0;
/// What fine movement scales flying, pushing and turning by.
pub const FINE: f32 = 0.2;

const fn row(
    does: Does,
    name: &'static str,
    pad: Pad,
    keys: &'static [Key],
    mouse: &'static str,
) -> Binding {
    Binding {
        does,
        name,
        pad,
        keys,
        mouse,
    }
}

const fn act(a: Action) -> Does {
    Does::Act(Gesture::Do(a))
}

use HostButton as B;

/// Halo 3's layout, with B as cancel where Halo 3 has hold to drop. The
/// D-pad's other three directions and the stick click are ours. The host
/// pad's bumpers are the game's Black and White buttons.
pub const BINDINGS: &[Binding] = &[
    row(
        act(Action::Toggle),
        "Monitor or player",
        Pad::Tap(B::DpadUp),
        &[Key::Toggle],
        "Tab",
    ),
    row(
        Does::Move(Motion::Fly),
        "Fly",
        Pad::Stick(Side::Left),
        &[
            Key::Forward,
            Key::Backward,
            Key::StrafeLeft,
            Key::StrafeRight,
        ],
        "W A S D",
    ),
    row(
        Does::Move(Motion::Look),
        "Look",
        Pad::Stick(Side::Right),
        &[],
        "the mouse",
    ),
    row(
        Does::Hold(Held::Rise),
        "Up",
        Pad::Down(B::RightBumper),
        &[Key::Rise],
        "E",
    ),
    row(
        Does::Hold(Held::Fall),
        "Down",
        Pad::Down(B::LeftBumper),
        &[Key::Fall],
        "Q",
    ),
    row(
        Does::Hold(Held::Thrust),
        "Thrust",
        Pad::Trigger(Side::Left),
        &[Key::Thrust],
        "hold Shift",
    ),
    row(
        Does::Act(Gesture::Fine),
        "Fine movement, on or off",
        Pad::Tap(B::LeftStick),
        &[Key::Fine],
        "Left Ctrl",
    ),
    row(
        act(Action::Accept),
        "Pick up, drop or place",
        Pad::Tap(B::South),
        &[Key::Accept],
        "the left mouse button",
    ),
    row(
        Does::Hold(Held::Manipulate),
        "Move a held object",
        Pad::Trigger(Side::Right),
        &[],
        "hold the right mouse button",
    ),
    row(
        Does::Act(Gesture::ObjectsOrProperties),
        "Objects or properties",
        Pad::Tap(B::West),
        &[Key::Objects],
        "F",
    ),
    row(
        act(Action::Delete),
        "Delete",
        Pad::Hold(B::North),
        &[Key::Delete],
        "Delete",
    ),
    row(
        act(Action::Back),
        "Cancel",
        Pad::Tap(B::East),
        &[Key::Back],
        "Escape",
    ),
    row(
        act(Action::Tools),
        "Tools",
        Pad::Tap(B::DpadRight),
        &[Key::Tools],
        "T",
    ),
    row(
        act(Action::Settings),
        "Tool settings",
        Pad::Tap(B::DpadDown),
        &[Key::Settings],
        "G",
    ),
    row(
        act(Action::Overlays),
        "Overlays",
        Pad::Tap(B::Back),
        &[Key::Overlays],
        "H",
    ),
    row(
        act(Action::Undo),
        "Undo",
        Pad::Tap(B::DpadLeft),
        &[Key::Undo],
        "Cmd+Z",
    ),
    row(
        act(Action::Redo),
        "Redo",
        Pad::Hold(B::DpadLeft),
        &[Key::Redo],
        "Shift+Cmd+Z",
    ),
    row(
        Does::Game,
        "Forge menu",
        Pad::Tap(B::Start),
        &[],
        "Escape with nothing held",
    ),
];

/// A button's name as the Controls page and the HUD's prompts write it.
pub const fn button_name(b: HostButton) -> &'static str {
    match b {
        B::South => "A",
        B::East => "B",
        B::West => "X",
        B::North => "Y",
        B::LeftBumper => "LB",
        B::RightBumper => "RB",
        B::Back => "Back",
        B::Start => "Start",
        B::LeftStick => "Click the left stick",
        B::RightStick => "Click the right stick",
        B::DpadUp => "D-pad Up",
        B::DpadDown => "D-pad Down",
        B::DpadLeft => "D-pad Left",
        B::DpadRight => "D-pad Right",
    }
}

/// Where a row is on the pad, as the Controls page writes it.
pub fn pad_name(p: Pad) -> String {
    match p {
        Pad::Tap(b) | Pad::Down(b) => button_name(b).into(),
        Pad::Hold(b) => format!("Hold {}", button_name(b)),
        Pad::Trigger(Side::Left) => "Hold LT".into(),
        Pad::Trigger(Side::Right) => "Hold RT".into(),
        Pad::Stick(Side::Left) => "Left stick".into(),
        Pad::Stick(Side::Right) => "Right stick".into(),
    }
}

/// Every control as the Controls page lists it, `(what, pad, keys)`, in
/// the table's order.
pub fn controls() -> Vec<(String, String, String)> {
    BINDINGS
        .iter()
        .map(|b| (b.name.to_string(), pad_name(b.pad), b.mouse.to_string()))
        .collect()
}

/// Pairs of controls the Controls page shows on one row: up and down, and
/// the tool settings and the overlays. Undo and redo keep a row each; their
/// pad names together are wider than the column.
pub const PAIRS: &[(Does, Does)] = &[
    (Does::Hold(Held::Rise), Does::Hold(Held::Fall)),
    (act(Action::Settings), act(Action::Overlays)),
];

/// The Controls page's rows, `(what, pad, keys)`: the table in its order,
/// each of [`PAIRS`] on its first control's row ("Up; down", "RB; LB",
/// "E; Q").
pub fn page() -> Vec<(String, String, String)> {
    let mut v = Vec::new();
    for b in BINDINGS {
        if PAIRS.iter().any(|(_, second)| *second == b.does) {
            continue;
        }
        let mut row = (b.name.to_string(), pad_name(b.pad), b.mouse.to_string());
        if let Some((_, second)) = PAIRS.iter().find(|(first, _)| *first == b.does) {
            let o = binding(*second);
            let lower = |s: &str| {
                let mut c = s.chars();
                c.next()
                    .map(|f| f.to_lowercase().chain(c).collect::<String>())
                    .unwrap_or_default()
            };
            row.0 = format!("{}; {}", row.0, lower(o.name));
            row.1 = format!("{}; {}", row.1, pad_name(o.pad));
            row.2 = format!("{}; {}", row.2, o.mouse);
        }
        v.push(row);
    }
    v
}

/// The row that does `does`: each has exactly one
/// (`every_action_has_one_binding`).
pub fn binding(does: Does) -> &'static Binding {
    BINDINGS
        .iter()
        .find(|b| b.does == does)
        .expect("every control has a row in BINDINGS")
}

/// The pad's name for an action the HUD prompts for: "Press (X) for the
/// object menu" reads X from here, never from its string.
pub fn prompt_button(does: Does) -> String {
    pad_name(binding(does).pad)
}

/// What a key sends, from the table.
pub fn of_key(key: Key) -> Option<&'static Binding> {
    BINDINGS.iter().find(|b| b.keys.contains(&key))
}

/// The action a gesture fires: X's choice follows what the editor
/// published (`x_properties`: something is aimed at or held). `None` for
/// the gestures the bridge keeps (fine movement).
pub fn resolve(g: Gesture, x_properties: bool) -> Option<Action> {
    match g {
        Gesture::Do(a) => Some(a),
        Gesture::ObjectsOrProperties if x_properties => Some(Action::Properties),
        Gesture::ObjectsOrProperties => Some(Action::Palette),
        Gesture::Fine => None,
    }
}

/// Whether a gesture works in the player's body: only the swap does.
pub fn in_body(g: Gesture) -> bool {
    g == Gesture::Do(Action::Toggle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every gesture the tool takes by name from the agent API
    /// (`Action::named`), to be bound in the table.
    const NAMED: &[Action] = &[
        Action::Toggle,
        Action::Accept,
        Action::Back,
        Action::Delete,
        Action::Undo,
        Action::Redo,
        Action::Tools,
        Action::Settings,
        Action::Overlays,
    ];

    #[test]
    fn x_opens_objects_or_properties_by_aim() {
        let x = binding(Does::Act(Gesture::ObjectsOrProperties));
        assert_eq!(x.pad, Pad::Tap(B::West));
        let Does::Act(g) = x.does else { unreachable!() };
        assert_eq!(
            resolve(g, false),
            Some(Action::Palette),
            "nothing aimed: the object menu"
        );
        assert_eq!(
            resolve(g, true),
            Some(Action::Properties),
            "aimed or held: Properties"
        );
    }

    /// Halo 3's layout, the owner's choice: hold Y deletes, B cancels, A
    /// picks up, D-pad Up swaps, RB and LB rise and fall, RT held
    /// manipulates, LT held thrusts, D-pad Right and Down are the Tools and
    /// their settings, Start is the game's.
    #[test]
    fn the_default_is_halo_3s_layout() {
        let pad = |d: Does| binding(d).pad;
        assert_eq!(pad(act(Action::Delete)), Pad::Hold(B::North));
        assert_eq!(pad(act(Action::Back)), Pad::Tap(B::East));
        assert_eq!(pad(act(Action::Accept)), Pad::Tap(B::South));
        assert_eq!(pad(act(Action::Toggle)), Pad::Tap(B::DpadUp));
        assert_eq!(pad(Does::Hold(Held::Rise)), Pad::Down(B::RightBumper));
        assert_eq!(pad(Does::Hold(Held::Fall)), Pad::Down(B::LeftBumper));
        assert_eq!(pad(Does::Hold(Held::Manipulate)), Pad::Trigger(Side::Right));
        assert_eq!(pad(Does::Hold(Held::Thrust)), Pad::Trigger(Side::Left));
        assert_eq!(pad(act(Action::Tools)), Pad::Tap(B::DpadRight));
        assert_eq!(pad(act(Action::Settings)), Pad::Tap(B::DpadDown));
        assert_eq!(pad(act(Action::Overlays)), Pad::Tap(B::Back));
        assert_eq!(pad(act(Action::Undo)), Pad::Tap(B::DpadLeft));
        assert_eq!(pad(act(Action::Redo)), Pad::Hold(B::DpadLeft));
        assert_eq!(pad(Does::Game), Pad::Tap(B::Start));
        assert!(
            BINDINGS.iter().all(|b| b.pad != Pad::Tap(B::North)),
            "a tap of Y does nothing"
        );
        assert!(
            BINDINGS.iter().all(|b| b.pad != Pad::Hold(B::East)),
            "B never deletes"
        );
    }

    /// Every action has one row, every place on the pad and every key one
    /// action, and every action the agent names is in the table.
    #[test]
    fn every_action_has_one_binding() {
        for (i, a) in BINDINGS.iter().enumerate() {
            let rest = &BINDINGS[i + 1..];
            assert!(rest.iter().all(|b| b.does != a.does), "{:?} twice", a.does);
            assert!(rest.iter().all(|b| b.pad != a.pad), "{:?} twice", a.pad);
            for k in a.keys {
                assert!(rest.iter().all(|b| !b.keys.contains(k)), "{k:?} twice");
            }
            assert!(!a.name.is_empty() && !a.mouse.is_empty(), "{:?}", a.does);
            // A button is a tap and a hold, or held, never both.
            if let Pad::Down(b) = a.pad {
                assert!(
                    BINDINGS
                        .iter()
                        .all(|x| x.pad == a.pad || x.pad.button() != Some(b)),
                    "{b:?} is held and pressed"
                );
            }
        }
        for a in NAMED {
            assert!(
                BINDINGS.iter().any(|b| b.does == act(*a)),
                "{a:?} has no binding"
            );
        }
        for g in [Gesture::ObjectsOrProperties, Gesture::Fine] {
            assert!(BINDINGS.iter().any(|b| b.does == Does::Act(g)), "{g:?}");
        }
        for h in [Held::Rise, Held::Fall, Held::Thrust, Held::Manipulate] {
            assert!(BINDINGS.iter().any(|b| b.does == Does::Hold(h)), "{h:?}");
        }
        for key in [
            Key::Toggle,
            Key::Forward,
            Key::Backward,
            Key::StrafeLeft,
            Key::StrafeRight,
            Key::Rise,
            Key::Fall,
            Key::Thrust,
            Key::Fine,
            Key::Objects,
            Key::Delete,
            Key::Back,
            Key::Accept,
            Key::Tools,
            Key::Settings,
            Key::Overlays,
            Key::Undo,
            Key::Redo,
        ] {
            assert!(of_key(key).is_some(), "{key:?} sends nothing");
        }
    }

    /// The Controls page lists every row of the table, the pairs on one
    /// row each, and fits the in-game list's lines.
    #[test]
    fn the_controls_page_lists_the_bindings_table() {
        let page = page();
        assert_eq!(page.len(), BINDINGS.len() - PAIRS.len());
        assert!(page.len() <= (crate::list_class::LINES_PER_CLASS / 2) as usize);
        for b in BINDINGS {
            let pad = pad_name(b.pad);
            assert!(
                page.iter().any(|(_, p, k)| p.split("; ").any(|x| x == pad)
                    && k.split("; ").any(|x| x == b.mouse)),
                "{:?} is not on the page",
                b.does
            );
        }
        assert!(page.contains(&("Up; down".into(), "RB; LB".into(), "E; Q".into())));
        assert!(page.contains(&(
            "Tool settings; overlays".into(),
            "D-pad Down; Back".into(),
            "G; H".into()
        )));
        assert!(page.contains(&("Undo".into(), "D-pad Left".into(), "Cmd+Z".into())));
        assert!(page.contains(&("Redo".into(), "Hold D-pad Left".into(), "Shift+Cmd+Z".into())));
    }

    #[test]
    fn the_names_and_prompts_come_from_the_table() {
        let c = controls();
        assert_eq!(c.len(), BINDINGS.len());
        assert!(c.contains(&("Delete".into(), "Hold Y".into(), "Delete".into())));
        assert!(c.contains(&("Up".into(), "RB".into(), "E".into())));
        assert!(c.contains(&(
            "Forge menu".into(),
            "Start".into(),
            "Escape with nothing held".into()
        )));
        assert_eq!(prompt_button(Does::Act(Gesture::ObjectsOrProperties)), "X");
        assert_eq!(prompt_button(act(Action::Delete)), "Hold Y");
        assert!(
            c.iter()
                .all(|(a, b, k)| !a.is_empty() && !b.is_empty() && !k.is_empty())
        );
    }
}
