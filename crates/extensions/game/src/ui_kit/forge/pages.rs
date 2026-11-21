//! Named preview states run the same dialog transitions as the shell.

use super::dialogs::Effect;
use super::{Dialogs, HUD, Hud, MENU, Mode, PALETTE, PROPERTIES};
use crate::ui_kit::{
    input::Input,
    page::{Driven, Live, Page, focus_line},
    spec::ScreenSpec,
};

pub(super) fn sample_hud(mode: Mode) -> Hud {
    Hud {
        mode,
        target: if mode == Mode::Idle {
            String::new()
        } else {
            "Beacon".into()
        },
        used: 12,
        limit: 80,
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
        [Mode::Idle, Mode::Aimed, Mode::Placing, Mode::Holding]
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
            .collect()
    }

    fn summary(&self, s: &Hud) -> String {
        format!("{:?}", s.mode)
    }
}

#[derive(Clone, Copy)]
pub(super) enum DialogPage {
    Palette,
    Properties,
    Menu,
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
        _ => return None,
    };
    Some(Box::new(Driven::new(page)))
}
