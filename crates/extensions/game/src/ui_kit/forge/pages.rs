//! Named preview states run the same dialog transitions as the shell.

use super::dialogs::Effect;
use super::{Dialogs, HUD, Hud, LINK, LOOK, MENU, Mode, PALETTE, PROPERTIES};
use crate::ui_kit::{
    input::Input,
    page::{Driven, Live, Page, focus_line},
    spec::ScreenSpec,
};

pub(super) fn sample_hud(mode: Mode) -> Hud {
    Hud {
        mode,
        links: mode == Mode::Linking,
        target: match mode {
            Mode::Idle => String::new(),
            Mode::Linking => "Teleporter sender 2 to Teleporter receiver 5".into(),
            _ => "Beacon".into(),
        },
        used: 12,
        limit: 80,
        rebuild: 0,
        notice: None,
    }
}

pub(super) struct HudPage;

impl Page for HudPage {
    type State = Hud;
    type Event = Input;
    type Effect = ();

    // The HUD has no focus. The camera and tool consume input in the session.
    fn step(&self, _: &mut Hud, _: Input) {}
    fn handled(_: &()) -> bool {
        false
    }

    fn spec(&self, state: &Hud) -> ScreenSpec {
        state.spec()
    }

    fn states(&self) -> Vec<(String, Hud)> {
        let mut states: Vec<_> = [
            Mode::Idle,
            Mode::Aimed,
            Mode::Placing,
            Mode::Holding,
            Mode::Linking,
        ]
        .into_iter()
        .flat_map(|mode| {
            let name = format!("{mode:?}").to_lowercase();
            let hud = sample_hud(mode);
            [
                (name.clone(), hud.clone()),
                (
                    format!("{name}-refused"),
                    Hud {
                        notice: Some("The object budget is full.".into()),
                        ..hud
                    },
                ),
            ]
        })
        .collect();
        for mode in [Mode::Idle, Mode::Aimed] {
            let name = format!("links-{mode:?}").to_lowercase();
            let hud = Hud {
                links: true,
                target: if mode == Mode::Aimed {
                    "Teleporter receiver 5, channel 2".into()
                } else {
                    String::new()
                },
                ..sample_hud(mode)
            };
            states.push((name.clone(), hud.clone()));
            states.push((format!("{name}-rebuild"), Hud { rebuild: 2, ..hud }));
        }
        states
    }

    fn summary(&self, s: &Hud) -> String {
        if s.links {
            format!("Links {:?}", s.mode)
        } else {
            format!("{:?}", s.mode)
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DialogPage {
    Palette,
    Properties,
    Menu,
    Link,
    Look,
}

impl Page for DialogPage {
    type State = Dialogs;
    type Event = Input;
    type Effect = Effect;

    fn step(&self, s: &mut Dialogs, input: Input) -> Effect {
        s.step(input)
    }
    fn handled(e: &Effect) -> bool {
        *e != Effect::Ignored
    }
    fn spec(&self, s: &Dialogs) -> ScreenSpec {
        s.spec(&sample_hud(Mode::Aimed))
    }

    fn states(&self) -> Vec<(String, Dialogs)> {
        match self {
            Self::Palette => {
                let items = [
                    ("Devices", "Gate"),
                    ("Scenery", "Beacon"),
                    ("Devices", "Button"),
                    ("Scenery", "Lantern"),
                    ("Scenery", "Marker"),
                    ("Scenery", "Panel"),
                    ("Scenery", "Pillar"),
                    ("Scenery", "Ramp"),
                    ("Scenery", "Tray"),
                ]
                .map(|(group, name)| (group.into(), name.into()))
                .to_vec();
                let groups = Dialogs::grouped_palette(items.clone(), 0);
                let mut objects = groups.clone();
                objects.press(Input::A);
                let mut scrolled = Dialogs::grouped_palette(items, 8);
                scrolled.press(Input::A);
                vec![
                    ("open".into(), groups),
                    ("objects".into(), objects),
                    ("scrolled".into(), scrolled),
                    ("empty".into(), Dialogs::grouped_palette(vec![], 0)),
                ]
            }
            Self::Properties => {
                let s = Dialogs::properties("Beacon".into(), 2, 17.5_f32.to_radians());
                let mut heading = s.clone();
                heading.press(Input::A);
                let mut rotation = s.clone();
                rotation.click(1);
                vec![
                    ("open".into(), s),
                    ("heading".into(), heading),
                    ("rotation".into(), rotation),
                ]
            }
            Self::Menu => {
                let menu = Dialogs::menu(vec!["Courtyard".into(), "Stage".into()]);
                let load = |mut s: Dialogs| {
                    s.click(2);
                    s
                };
                vec![
                    ("open".into(), menu.clone()),
                    ("load".into(), load(menu)),
                    ("no-saves".into(), load(Dialogs::menu(vec![]))),
                ]
            }
            Self::Look => {
                let catalogue = |key: &str, colours: bool| forge::look::census::Entry {
                    key: key.into(),
                    name: "Sentinel".into(),
                    source: "05a_deltaapproach".into(),
                    sha256: "abc123".into(),
                    variants: vec!["default".into(), "major".into()],
                    change_colours: colours,
                    light: true,
                    nodes: 20,
                    triangles: 900,
                };
                let colours: Vec<String> = (0..18).map(|n| format!("Colour {n}")).collect();
                let mut model = forge::look::MonitorLook::default_for(3);
                model.model = "sentinel".into();
                model.variant = None;
                model.tint = 1;
                model.light = 1;
                let mut lockless = Dialogs::look(model.clone(), &[catalogue("sentinel", true)], &colours);
                lockless.press(Input::A);
                let no_colours = Dialogs::look(model.clone(), &[catalogue("sentinel", false)], &colours);
                let marker = Dialogs::look(forge::look::MonitorLook::default_for(2), &[], &colours);
                vec![
                    ("open".into(), Dialogs::look(model, &[catalogue("sentinel", true)], &colours)),
                    ("picker".into(), lockless),
                    ("locked".into(), no_colours),
                    ("marker".into(), marker),
                ]
            }
            Self::Link => {
                let row = |label: &str, value: &str| (label.to_string(), value.to_string());
                vec![
                    (
                        "teleporter".into(),
                        Dialogs::link(vec![
                            row("From", "Teleporter sender 2"),
                            row("To", "Teleporter receiver 5"),
                            row("Channel", "3"),
                        ]),
                    ),
                    (
                        "device".into(),
                        Dialogs::link(vec![
                            row("From", "Gate"),
                            row("To", "Lift"),
                            row("Device group", "Rebuild map to apply"),
                        ]),
                    ),
                ]
            }
        }
    }

    fn summary(&self, s: &Dialogs) -> String {
        let spec = self.spec(s);
        format!("{} | {}", spec.title, focus_line(&spec))
    }
}

pub(in crate::ui_kit) fn live(name: &str) -> Option<Box<dyn Live>> {
    let page = match name {
        HUD => return Some(Box::new(Driven::new(HudPage))),
        MENU => DialogPage::Menu,
        PROPERTIES => DialogPage::Properties,
        PALETTE => DialogPage::Palette,
        LINK => DialogPage::Link,
        LOOK => DialogPage::Look,
        _ => return None,
    };
    Some(Box::new(Driven::new(page)))
}
