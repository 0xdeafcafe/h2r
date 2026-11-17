//! Our existing screens, described for the kit: the skulls pages, the
//! co-op screens, the sign-in prompt and the account menu
//! ([`Describe`]), each with a sample state for the checks and previews
//! ([`sample`], `h2 ui preview <screen>`).
//!
//! The descriptions read the screens' own rows (their string ids resolved
//! through the tables that serve them), so a check here is a check of
//! what the game is handed.

use std::sync::Arc;

use crate::coop_live::{self, FriendRows, HostRows, JoinRows, Lobby};
use crate::list_class::Rows;
use crate::skulls::{self, SKULLS, State, TOTAL};
use crate::skulls_menu::{self, Mode, SkullsScreen};
use crate::ui_kit::input::{Glyph, Input, Prompt};
use crate::ui_kit::spec::{Describe, ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};

/// The campaign page's legend: left/right toggle, A starts, B backs out.
pub fn skulls_prompts() -> Vec<Prompt> {
    vec![
        Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Toggle"),
        Prompt::new(&[Glyph::A], "Start Level"),
        Prompt::new(&[Glyph::B], "Back"),
    ]
}

/// A string the skulls pages serve.
fn skull_text(id: u32) -> Option<String> {
    skulls_menu::strings()
        .into_iter()
        .find(|(i, _)| *i == id)
        .map(|(_, t)| t.to_string())
}

impl Describe for SkullsScreen {
    fn spec(&self) -> ScreenSpec {
        let title = skull_text(self.title()).unwrap_or_else(|| "?".into());
        let campaign = self.mode() == Mode::Campaign;
        let mut s = ScreenSpec::new(
            if campaign {
                crate::skulls_screen::SCREEN
            } else {
                "skulls.mp"
            },
            &title,
        );
        s.ids = vec![self.title()];
        s.list = ListPlace::from(Some(skulls_menu::LIST_LAYOUT));
        s.style = ValueStyle::Inline;
        s.description = Some(skulls_menu::DESCRIPTION_TEXT.into());
        s.scrolls = true;
        if campaign {
            s.prompts = skulls_prompts();
            s.inputs = vec![
                Input::Up,
                Input::Down,
                Input::Left,
                Input::Right,
                Input::A,
                Input::B,
                Input::X,
                Input::Start,
            ];
            let mut order: Vec<(i16, usize)> = (0..TOTAL)
                .filter_map(|i| Some((skulls_menu::row_of(i)?, i)))
                .collect();
            order.sort();
            for (row, i) in order {
                let label = SKULLS[i].label;
                let r = match self.state(i) {
                    State::On => RowSpec::value(label, &["On", "Off"], 0),
                    State::Off => RowSpec::value(label, &["On", "Off"], 1),
                    State::Locked => RowSpec::info(label, skulls_menu::LOCKED),
                };
                let mut ids: Vec<u32> = self.texts(row).into_iter().map(|(_, id)| id).collect();
                ids.extend(Rows::description(self, row));
                s.rows.push(
                    r.press(Press::Act)
                        .describe(&skulls_menu::description(i))
                        .ids(&ids),
                );
            }
        } else {
            s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
            for row in 0..self.count() as i16 {
                let ids: Vec<u32> = self
                    .texts(row)
                    .into_iter()
                    .map(|(_, id)| id)
                    .chain(Rows::description(self, row))
                    .collect();
                let label = ids
                    .first()
                    .and_then(|id| skull_text(*id))
                    .unwrap_or_else(|| "?".into());
                s.rows.push(
                    RowSpec::action(&label)
                        .press(Press::Step)
                        .describe(&skulls_menu::mp_description(row as usize))
                        .ids(&ids),
                );
            }
        }
        s
    }
}

/// The co-op screens' inputs: the list's, and the action on A, X, Start.
fn coop_inputs() -> Vec<Input> {
    vec![
        Input::Up,
        Input::Down,
        Input::A,
        Input::B,
        Input::X,
        Input::Start,
    ]
}

fn coop_spec(
    name: &str,
    title: String,
    rows: impl Iterator<Item = (String, String, Press)>,
) -> ScreenSpec {
    let mut s = ScreenSpec::new(name, &title);
    s.inputs = coop_inputs();
    for (label, value, press) in rows {
        let r = if value.is_empty() && press != Press::Nothing {
            RowSpec::action(&label)
        } else {
            RowSpec::info(&label, &value)
        };
        s.rows.push(r.press(press));
    }
    s
}

fn coop_title(rows: &dyn Rows, lobby: &Lobby) -> String {
    let id = rows.title();
    lobby
        .texts
        .get(id - coop_live::TEXT_ID_BASE)
        .unwrap_or_else(|| "?".into())
}

impl Describe for HostRows {
    fn spec(&self) -> ScreenSpec {
        use coop_live::{ROW_INVITE, ROW_START};
        let rows = (0..self.count() as i16).filter_map(|r| {
            let (l, v) = self.row_texts(r)?;
            let press = match r {
                ROW_INVITE => Press::Open(coop_live::FRIENDS.into()),
                ROW_START => Press::Act,
                _ => Press::Step,
            };
            Some((l, v, press))
        });
        coop_spec(coop_live::SCREEN, coop_title(self, &self.0), rows)
    }
}

impl Describe for FriendRows {
    fn spec(&self) -> ScreenSpec {
        let rows = (0..self.count() as i16).filter_map(|r| {
            let (l, v) = self.row_texts(r)?;
            Some((l, v, Press::Step))
        });
        let mut s = coop_spec(coop_live::FRIENDS, coop_title(self, &self.0), rows);
        s.scrolls = true;
        s
    }
}

impl Describe for JoinRows {
    fn spec(&self) -> ScreenSpec {
        let rows = (0..self.count() as i16).filter_map(|r| {
            let (l, v) = self.row_texts(r)?;
            let press = match r {
                coop_live::JOIN_READY => Press::Act,
                4 => Press::Step,
                _ => Press::Nothing,
            };
            Some((l, v, press))
        });
        coop_spec(coop_live::JOIN, coop_title(self, &self.0), rows)
    }
}

/// The account menu ([`crate::account_screen`]): drawn in the stock
/// dialog today, described as the list it would be on the kit.
pub fn account_spec(a: &crate::account_screen::AccountScreen, now: i64) -> ScreenSpec {
    use crate::account_screen::{MENU, Step};
    let mut s = ScreenSpec::new("account", a.title());
    s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
    s.description = Some(crate::signin_screen::TEXT_BLOCK.into());
    match &a.step {
        Step::Menu { focus } => {
            s.focus = *focus;
            s.rows = MENU.iter().map(|r| RowSpec::action(r)).collect();
            s.prompts = vec![
                Prompt::new(&[Glyph::A], "Select"),
                Prompt::new(&[Glyph::B], "Back"),
            ];
        }
        _ => {
            s.rows = vec![RowSpec::action(a.title()).describe(&a.text(now).replace("\r\n", "\n"))];
        }
    }
    s
}

/// The screens `h2 ui preview` knows, by name.
pub const NAMES: [&str; 8] = [
    "skulls",
    "skulls.mp",
    "coop",
    "coop.friends",
    "coop.join",
    "signin-code",
    "account",
    "kit.example",
];

/// A sample of screen `name`: the skulls pages for a profile with a few
/// skulls collected and one turned off, the co-op screens with no Live,
/// the sign-in prompt at its code page, the account menu, and the kit's
/// example.
pub fn sample(name: &str) -> Option<ScreenSpec> {
    let skull_store = || {
        let defs: Vec<&'static settings::Def> = SKULLS.iter().collect();
        let store = Arc::new(settings::Store::defaults(&defs));
        for i in [0, 2, 4, 9, 12] {
            skulls::record_pickup(&store, &["Chief".to_string()], i);
        }
        let _ = skulls::toggle(&store, "Chief", SKULLS[4].key);
        store
    };
    let lobby = || {
        let levels = vec![coop_live::Level {
            stem: "03a_oldmombasa".into(),
            campaign: 1,
            map: 3,
            name: Some("Outskirts".into()),
        }];
        Arc::new(Lobby::new(levels, None, None, Box::new(|_| Vec::new())))
    };
    Some(match name {
        "skulls" => {
            let s = SkullsScreen::new(skull_store());
            s.open("Chief".into());
            s.spec()
        }
        "skulls.mp" => {
            let s = SkullsScreen::multiplayer(skull_store());
            s.open("Chief".into());
            s.spec()
        }
        "coop" => HostRows(lobby()).spec(),
        "coop.friends" => FriendRows(lobby()).spec(),
        "coop.join" => JoinRows(lobby()).spec(),
        "signin-code" => crate::signin_screen::sample_spec(),
        "account" => account_spec(&crate::account_screen::AccountScreen::new(None), 0),
        "kit.example" => crate::ui_kit::screen::example().0.spec(),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::spec::{Finding, check_fallback};

    /// The kit's legend is the skulls page's own, glyph for glyph, and the
    /// description the page serves is the described one.
    #[test]
    fn the_skulls_legend_and_descriptions_are_the_served_ones() {
        assert_eq!(
            crate::ui_kit::input::legend(&skulls_prompts()),
            skulls_menu::PROMPT
        );
        let s = sample("skulls").unwrap();
        for (row, r) in s.rows.iter().enumerate() {
            let focused = ScreenSpec {
                focus: row,
                ..s.clone()
            };
            let served = skull_text(*r.ids.last().unwrap()).unwrap();
            assert_eq!(focused.description_text().unwrap(), served, "row {row}");
        }
    }

    /// Every screen, without the disc: ids in families, presses handled.
    /// Widths are the disc test's (`preview_and_check_against_the_disc`).
    #[test]
    fn every_screen_passes_the_fallback_checks_but_the_known_ones() {
        for name in NAMES {
            let s = sample(name).unwrap();
            let f: Vec<Finding> = check_fallback(&s)
                .into_iter()
                .filter(|f| !matches!(f, Finding::Wide { .. } | Finding::Description { .. }))
                .collect();
            let expect = match name {
                // Six rows in the Controls list's five; nothing says the
                // host screen scrolls.
                "coop" | "coop.join" => vec![Finding::Rows {
                    rows: 6,
                    visible: 5,
                }],
                _ => vec![],
            };
            assert_eq!(f, expect, "{name}: {f:#?}");
        }
    }

    #[test]
    fn screens_describe_themselves_as_json_for_the_ui_stack() {
        let s = sample("coop").unwrap();
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["name"], "coop");
        assert_eq!(j["rows"][2]["label"], "Invite a Friend");
        assert_eq!(j["rows"][2]["press"]["open"], "coop.friends");
        assert_eq!(j["rows"][0]["kind"], "info");
        let k = serde_json::to_value(sample("skulls").unwrap()).unwrap();
        assert_eq!(k["style"], "inline");
        assert_eq!(k["prompts"][0]["glyphs"][0], "DpadLeft");
    }

    /// With the disc (`disc/halo2.iso`, or `H2_DISC`): every screen checked
    /// against the real list geometry and fonts, and, with
    /// `H2_UI_PREVIEW_DIR` set, drawn there. Skipped without a disc.
    #[test]
    fn preview_and_check_against_the_disc() {
        let iso = std::env::var_os("H2_DISC")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../disc/halo2.iso")
            });
        if !iso.exists() {
            eprintln!("no disc at {}; skipped", iso.display());
            return;
        }
        let disc = crate::ui_kit::preview::Disc::open(&iso).unwrap();
        let out = std::env::var_os("H2_UI_PREVIEW_DIR").map(std::path::PathBuf::from);
        let mut report = Vec::new();
        for name in NAMES {
            let s = sample(name).unwrap();
            let g = disc.geometry(s.list).unwrap();
            eprintln!(
                "{name}: list at {:?}, {} visible, items {} high, label {:?} font {} ({:?}), value {:?}",
                g.anchor,
                g.visible,
                g.item_height,
                g.label.0,
                g.label.1,
                disc.fonts.file(g.label.1).map(|f| f.0),
                g.value
            );
            for f in disc.check(&s).unwrap() {
                report.push(format!("{name}: {f}"));
            }
            if let Some(dir) = &out {
                std::fs::create_dir_all(dir).unwrap();
                for focus in [0, s.rows.len().saturating_sub(1)] {
                    let f = ScreenSpec {
                        focus,
                        ..s.clone()
                    };
                    let path = dir.join(format!("{name}-focus{focus}.png"));
                    disc.render(&f).unwrap().write_png(&path).unwrap();
                    eprintln!("wrote {}", path.display());
                }
            }
        }
        for r in &report {
            eprintln!("finding: {r}");
        }
    }
}
