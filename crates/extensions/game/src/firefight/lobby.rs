//! The Firefight lobby (docs/game/firefight.md, "The lobby"):
//! **FIREFIGHT** on the main menu opens it, on the pregame lobby's skin
//! ([`crate::lobby_start`]). Its rows are:
//!
//! - **Map**: A opens a picker of the installed maps that carry the kit
//!   ([`crate::lobby_start::is_kit_map`]), each by its base map's name. With
//!   none installed the picker says so and how to build one.
//! - **Options...**: opens the Firefight options page
//!   ([`super::options::page`]) over the lobby.
//! - **Start**: begins a local custom game on the map
//!   ([`crate::lobby_start::start`]); the runtime then starts the mode from
//!   the page's choices, as for any game on a kit map
//!   ([`super::feature`]). It is refused, with the reason on the status
//!   line, when no map carries the kit, the options cannot be read, or
//!   Firefight is off in them.
//!
//! The page is a kit screen ([`Lobby::page`]) over a [`Lobby`] whose maps,
//! options and start are injected, so the table tests step it with none of
//! the game.

use std::sync::{Arc, Mutex, MutexGuard};

use super::options;
use crate::lobby_start::{self as shared, KIT_NAME_PREFIX, fit};
use crate::map_select::Map;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Prompt};
use crate::ui_kit::picker::Pick;
use crate::ui_kit::screen::{KitScreen, KitTexts, Row, Then};
use crate::ui_kit::spec::ValueStyle;

/// The page's feature (`features.firefight-lobby`) and screen name.
pub const FEATURE: &str = "firefight-lobby";
pub const SCREEN: &str = "firefight.lobby";
/// The page's string ids.
pub const IDS: Family = ids::family("firefight lobby");
pub const TITLE: &str = "FIREFIGHT";
/// What the Map row, its picker and Start say with no kit map installed.
pub const NO_MAP: &str = "No installed map carries the Firefight kit";
/// How to get one, in the picker's description.
pub const HOW_TO: &str = "Build one from a disc map with: h2 firefight kit MAP";
/// Start, with Firefight turned off in the options.
pub const OFF: &str = "Firefight is off. Turn it on in Options.";

type Maps = dyn Fn() -> Vec<Map> + Send + Sync;
type Start = dyn Fn(&Map) -> Result<(), String> + Send + Sync;

#[derive(Default)]
struct Choice {
    /// The map chosen, by list id: the first kit map while none is.
    map: Option<u32>,
    /// Why the last Start did not begin.
    error: Option<String>,
}

/// A kit map as the lobby names it: the base map, without the package's
/// `Firefight – `.
pub fn title(map: &Map) -> String {
    map.name
        .strip_prefix(KIT_NAME_PREFIX)
        .unwrap_or(&map.name)
        .to_owned()
}

/// The lobby's state and what it reads and starts through.
pub struct Lobby {
    maps: Arc<Maps>,
    options: Arc<options::Source>,
    start: Arc<Start>,
    choice: Mutex<Choice>,
}

impl Lobby {
    pub fn new(maps: Arc<Maps>, options: Arc<options::Source>, start: Arc<Start>) -> Arc<Self> {
        Arc::new(Lobby {
            maps,
            options,
            start,
            choice: Mutex::default(),
        })
    }

    fn choice(&self) -> MutexGuard<'_, Choice> {
        self.choice.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The map the lobby will start on: the one chosen, else the first.
    fn selected(&self, maps: &[Map]) -> Option<usize> {
        let id = self.choice().map;
        id.and_then(|id| maps.iter().position(|m| m.id == id))
            .or(if maps.is_empty() { None } else { Some(0) })
    }

    fn map_shown(&self) -> String {
        let maps = (self.maps)();
        self.selected(&maps)
            .map_or_else(|| NO_MAP.into(), |i| title(&maps[i]))
    }

    fn map_offer(self: &Arc<Self>) -> Result<Pick, String> {
        let maps = (self.maps)();
        if maps.is_empty() {
            return Ok(shared::nothing_to_pick("Map", NO_MAP, HOW_TO));
        }
        let me = Arc::clone(self);
        Ok(shared::map_pick(
            &maps,
            title,
            self.selected(&maps).unwrap_or(0),
            "The map Firefight is fought on.",
            move |map: &Map| {
                let mut c = me.choice();
                c.map = Some(map.id);
                c.error = None;
                format!("firefight lobby: map {}", title(map))
            },
        ))
    }

    /// Why Start cannot begin on `maps` now, or the map it begins on.
    fn ready(&self, maps: &[Map]) -> Result<Map, String> {
        let map = self
            .selected(maps)
            .map(|i| maps[i].clone())
            .ok_or_else(|| format!("{NO_MAP}."))?;
        let options = (self.options)().map_err(|why| format!("Options: {why}"))?;
        if !options.enabled {
            return Err(OFF.into());
        }
        Ok(map)
    }

    /// Start: the game on the chosen map, or why not.
    fn press_start(&self) -> Then {
        let maps = (self.maps)();
        let started = self.ready(&maps).and_then(|map| {
            (self.start)(&map)?;
            Ok(map)
        });
        match started {
            Ok(map) => {
                self.choice().error = None;
                Then::Stay(format!("firefight lobby: starting on {}", title(&map)))
            }
            Err(why) => {
                self.choice().error = Some(why.clone());
                Then::Stay(format!("firefight lobby: not started: {why}"))
            }
        }
    }

    /// The status line: a refusal, else what Start does.
    fn status(&self) -> String {
        if let Some(why) = self.choice().error.clone() {
            return fit(&why);
        }
        if let Some(why) = shared::failure() {
            return fit(&why);
        }
        let maps = (self.maps)();
        match self.ready(&maps) {
            Ok(map) => fit(&format!("Start: Firefight on {}", title(&map))),
            Err(why) => fit(&why),
        }
    }

    /// The page.
    pub fn page(self: &Arc<Self>) -> KitScreen {
        let (shown, offer) = (Arc::clone(self), Arc::clone(self));
        let map = Row::pick("Map", move || shown.map_shown(), move || offer.map_offer())
            .describe("The map to fight on.");
        let choices = Row::action("Options...", || Then::Open(options::SCREEN))
            .describe("The waves, difficulty and lives.");
        let start = Arc::clone(self);
        let go = Row::action("Start", move || start.press_start())
            .describe("Begin a local game of Firefight.");
        let status = Arc::clone(self);
        KitScreen::new(FEATURE, SCREEN, IDS, TITLE)
            .layout(shared::LAYOUT, ValueStyle::Inline)
            .live_help(shared::STATUS, move || status.status())
            .row(map)
            .row(choices)
            .row(go)
            .prompts(&[
                Prompt::new(&[Glyph::A], "Select"),
                Prompt::new(&[Glyph::B], "Back"),
            ])
    }
}

/// The lobby over the installed kit maps, the options page's choices and
/// the local game start.
pub fn lobby(options: Arc<options::Source>) -> Arc<Lobby> {
    Lobby::new(
        shared::maps_from(shared::kit_maps),
        options,
        Arc::new(|map: &Map| shared::start(&shared::request(map))),
    )
}

/// Adds the page and its texts.
pub fn install(
    registry: &mut hooks::Registry,
    options: Arc<options::Source>,
) -> anyhow::Result<()> {
    let screen = Arc::new(lobby(options).page());
    registry
        .add(&KitTexts(Arc::clone(&screen)))
        .map_err(|e| anyhow::anyhow!("firefight lobby texts: {e}"))?;
    registry
        .add(&screen.class().panel(shared::panel()))
        .map_err(|e| anyhow::anyhow!("firefight lobby page: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
