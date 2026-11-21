//! `h2 ui preview lobby.tabs`: the strip as the lobby draws it, in each
//! place and on each tab it can be on, for the leader and for a member
//! (docs/game/party-lobby.md, "Interfaces"). The widgets are the ones the
//! update shows ([`super::strip::Shown::visible`]), at their cells, in the
//! lobby's font and colours.

use super::strip::{FONT, Piece, Shown, pieces, shown};
use super::{Facts, Place, Present, Tab, available};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::Page;
use crate::ui_kit::spec::ScreenSpec;
use crate::ui_kit::view::{Justify, Kind, Layer, Widget};

/// The preview's name.
pub const NAME: &str = "lobby.tabs";

/// One drawn state: the strip for a lobby.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub present: Present,
    pub shown: Shown,
}

/// The strip's widgets for `s`: the pieces that show, by their ids
/// (`tab.forge`, `glyph.prev`).
pub fn widgets(s: &Sample) -> Vec<Widget> {
    pieces(s.present)
        .into_iter()
        .filter(|&p| s.shown.visible(p))
        .filter_map(|p| {
            let rect = p.rect(s.present)?;
            let (id, text) = match p {
                Piece::Cell(tab, _) => (format!("tab.{}", tab.name()), tab.label()),
                Piece::Glyph(super::Dir::Prev, _) => ("glyph.prev".into(), super::PREV_TEXT),
                Piece::Glyph(super::Dir::Next, _) => ("glyph.next".into(), super::NEXT_TEXT),
            };
            Some(Widget {
                id,
                rect,
                layer: Layer::Content,
                alpha: 255,
                kind: Kind::Text {
                    text: text.into(),
                    font: FONT,
                    justify: Justify::Centre,
                    colour: p.colour().map(|c| (c * 255.0).round() as u8),
                },
            })
        })
        .collect()
}

fn slug(place: Place) -> &'static str {
    match place {
        Place::SplitScreen => "split-screen",
        Place::SystemLink => "system-link",
        Place::XboxLive => "xbox-live",
    }
}

/// The lobby at `place`, the leader's or a member's, on `tab`: every
/// feature present, a kit map installed and the server's playlists listed.
fn sample(place: Place, leader: bool, tab: Tab, facts: Facts, present: Present) -> Sample {
    let p = tab.parts();
    let reading = super::strip::Reading {
        place,
        leader,
        retail: p.retail,
        kit_map: tab == Tab::Firefight,
        facts,
    };
    Sample {
        present,
        shown: shown(&reading, present, p.mode, p.clanker),
    }
}

/// The preview's page: the strip has no focus and takes no pad input of
/// the kit's (its bumpers are the lobby's).
pub struct TabsPage;

impl Page for TabsPage {
    type State = Sample;
    type Event = Input;
    type Effect = ();

    fn step(&self, _: &mut Sample, _: Input) {}
    fn handled(_: &()) -> bool {
        false
    }

    fn spec(&self, s: &Sample) -> ScreenSpec {
        ScreenSpec::hud(NAME, s.shown.selected.label(), widgets(s))
    }

    fn states(&self) -> Vec<(String, Sample)> {
        let facts = Facts {
            playlists: true,
            clanker_playlists: true,
            kit_maps: true,
            remote_members: false,
        };
        let mut out = Vec::new();
        for place in [Place::SplitScreen, Place::SystemLink, Place::XboxLive] {
            for tab in Tab::ALL {
                if available(tab, place, facts).is_on() {
                    let name = format!("{}.{}", slug(place), tab.name().replace('_', "-"));
                    out.push((name, sample(place, true, tab, facts, Present::ALL)));
                }
            }
        }
        out.push((
            "system-link.member".into(),
            sample(
                Place::SystemLink,
                false,
                Tab::CustomGames,
                facts,
                Present::ALL,
            ),
        ));
        out.push((
            "split-screen.no-kit-map".into(),
            sample(
                Place::SplitScreen,
                true,
                Tab::CustomGames,
                Facts {
                    kit_maps: false,
                    ..facts
                },
                Present::ALL,
            ),
        ));
        out.push((
            "split-screen.without-forge".into(),
            sample(
                Place::SplitScreen,
                true,
                Tab::CustomGames,
                facts,
                Present {
                    forge: false,
                    ..Present::ALL
                },
            ),
        ));
        out
    }

    fn summary(&self, s: &Sample) -> String {
        let tabs: Vec<String> = s
            .shown
            .row
            .iter()
            .map(|&(t, a)| format!("{} {:?}", t.label(), super::look(t, s.shown.selected, a)))
            .collect();
        format!(
            "{:?}{}: {}",
            s.shown.place,
            if s.shown.leader { "" } else { " (member)" },
            tabs.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_state_draws_one_label_per_tab_present() {
        for (name, s) in TabsPage.states() {
            let w = widgets(&s);
            let labels = w.iter().filter(|w| w.id.starts_with("tab.")).count();
            assert_eq!(labels, s.present.tabs().len(), "{name}");
            let glyphs = w.iter().filter(|w| w.id.starts_with("glyph.")).count();
            assert_eq!(glyphs, if s.shown.leader { 2 } else { 0 }, "{name}");
        }
    }

    #[test]
    fn the_states_cover_each_place_and_the_member() {
        let names: Vec<String> = TabsPage.states().into_iter().map(|(n, _)| n).collect();
        for want in [
            "split-screen.custom-games",
            "split-screen.forge",
            "system-link.firefight",
            "xbox-live.matchmaking",
            "xbox-live.clankers",
            "system-link.member",
        ] {
            assert!(names.iter().any(|n| n == want), "{want}: {names:?}");
        }
        assert!(
            !names.iter().any(|n| n == "system-link.co-op"),
            "CO-OP cannot be on System Link"
        );
    }
}
