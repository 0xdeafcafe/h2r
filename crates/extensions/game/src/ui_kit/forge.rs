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
const LINK: &str = "forge.link";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Idle,
    Aimed,
    Holding,
    Placing,
    /// The Links tool has a link's first end.
    Linking,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hud {
    pub mode: Mode,
    /// The Links tool is selected instead of the Objects tool.
    pub links: bool,
    pub target: String,
    pub used: usize,
    pub limit: usize,
    /// Edits that reach the game only through **Rebuild map**.
    pub rebuild: usize,
    pub notice: Option<String>,
}

impl Hud {
    pub fn spec(&self) -> ScreenSpec {
        let tool = if self.links { "LINKS" } else { "OBJECTS" };
        let title = match self.rebuild {
            0 => tool.to_string(),
            n => format!("{tool}   REBUILD {n}"),
        };
        let mut widgets = vec![
            panel("hud.top", [204, -272, 148, 272]),
            text(
                "title",
                [198, -264, 171, 40],
                Justify::Left,
                text_fit::line(&title, 290),
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
        ScreenSpec::hud(HUD, tool, widgets)
    }

    fn prompts(&self) -> Vec<Prompt> {
        if self.links {
            return self.link_prompts();
        }
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
            Mode::Idle | Mode::Linking => {}
        }
        if matches!(self.mode, Mode::Idle | Mode::Aimed) {
            p.push(Prompt::new(&[Glyph::X], "Palette"));
        }
        if self.mode == Mode::Idle {
            p.push(Prompt::new(&[Glyph::DpadRight], "Links"));
            p.push(Prompt::new(&[Glyph::Start], "Forge menu"));
            p.push(Prompt::new(&[Glyph::DpadUp], "Player"));
        }
        p
    }

    fn link_prompts(&self) -> Vec<Prompt> {
        match self.mode {
            Mode::Linking => vec![
                Prompt::new(&[Glyph::A], "Link"),
                Prompt::new(&[Glyph::B], "Cancel"),
            ],
            Mode::Aimed => vec![
                Prompt::new(&[Glyph::A], "Start link"),
                Prompt::new(&[Glyph::Y], "Link"),
                Prompt::new(&[Glyph::B], "Hold to unlink"),
            ],
            _ => vec![
                Prompt::new(&[Glyph::DpadRight], "Objects"),
                Prompt::new(&[Glyph::Start], "Forge menu"),
                Prompt::new(&[Glyph::DpadUp], "Player"),
            ],
        }
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
