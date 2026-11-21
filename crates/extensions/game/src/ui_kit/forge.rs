//! Compositor pages for the local Forge monitor. The host owns the session,
//! camera, file I/O and world calls. A dialog returns the selected command.

use super::input::{Glyph, Prompt, legend};
use super::spec::ScreenSpec;
use super::text_fit;
use super::view::{Art, Justify, Kind, Layer, Widget};

mod dialogs;
mod pages;
pub use dialogs::{ANGLE_STEPS, Action, Dialogs, HEADINGS};
pub(super) use pages::live;

const HUD: &str = "forge.hud";
const PALETTE: &str = "forge.palette";
const PROPERTIES: &str = "forge.properties";
const MENU: &str = "forge.menu";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Idle,
    Aimed,
    Holding,
    Placing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hud {
    pub mode: Mode,
    pub target: String,
    pub used: usize,
    pub limit: usize,
    pub notice: Option<String>,
}

impl Hud {
    pub fn spec(&self) -> ScreenSpec {
        let mut widgets = vec![
            panel("hud.top", [204, -272, 148, 272]),
            text(
                "title",
                [198, -264, 171, 40],
                Justify::Left,
                "OBJECTS".into(),
            ),
            text(
                "budget",
                [198, 40, 171, 264],
                Justify::Right,
                format!("Budget {} / {}", self.used, self.limit),
            ),
            text("reticle", [9, -12, -16, 12], Justify::Centre, "+".into()),
            panel("hud.bottom", [-177, -272, -211, 272]),
            text(
                "legend",
                [-183, -264, -208, 264],
                Justify::Centre,
                legend(&self.prompts()),
            ),
        ];
        if !self.target.is_empty() {
            widgets.push(text(
                "target",
                [172, -264, 148, 264],
                Justify::Left,
                text_fit::line(&self.target, 510),
            ));
        }
        if let Some(notice) = self.notice.as_deref().filter(|n| !n.is_empty()) {
            widgets.push(panel("hud.message", [-113, -272, -175, 272]));
            widgets.push(text(
                "notice",
                [-118, -264, -174, 264],
                Justify::Left,
                text_fit::paragraph(notice, 490, 2),
            ));
        }
        ScreenSpec::hud(HUD, "OBJECTS", widgets)
    }

    fn prompts(&self) -> Vec<Prompt> {
        let mut p = Vec::new();
        match self.mode {
            Mode::Aimed => {
                p.push(Prompt::new(&[Glyph::A], "Pick up"));
                p.push(Prompt::new(&[Glyph::Y], "Properties"));
                p.push(Prompt::new(&[Glyph::B], "Hold to delete"));
            }
            Mode::Holding | Mode::Placing => {
                p.push(Prompt::new(
                    &[Glyph::A],
                    if self.mode == Mode::Holding {
                        "Drop"
                    } else {
                        "Place"
                    },
                ));
                p.push(Prompt::new(&[Glyph::Black], "Hold to turn"));
                p.push(Prompt::new(&[Glyph::Y], "Properties"));
                p.push(Prompt::new(&[Glyph::B], "Cancel"));
            }
            Mode::Idle => {}
        }
        if matches!(self.mode, Mode::Idle | Mode::Aimed) {
            p.push(Prompt::new(&[Glyph::X], "Palette"));
        }
        if self.mode == Mode::Idle {
            p.push(Prompt::new(&[Glyph::Start], "Forge menu"));
            p.push(Prompt::new(&[Glyph::DpadUp], "Player"));
        }
        p
    }
}

fn panel(id: &str, rect: [i16; 4]) -> Widget {
    Widget {
        id: id.into(),
        rect,
        alpha: 255,
        layer: Layer::Panel,
        kind: Kind::Art {
            art: Art::Solid {
                rgba: [10, 19, 29, 214],
            },
            size: [(rect[3] - rect[1]) as u16, (rect[0] - rect[2]) as u16],
        },
    }
}

fn text(id: &str, rect: [i16; 4], justify: Justify, value: String) -> Widget {
    Widget {
        id: id.into(),
        rect,
        alpha: 255,
        layer: Layer::Content,
        kind: Kind::Text {
            text: value,
            font: 1,
            justify,
            colour: [235, 243, 252],
        },
    }
}

#[cfg(test)]
mod tests;
