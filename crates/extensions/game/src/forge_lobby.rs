//! The Forge lobby (docs/game/forge-design.md, "The UI"): **FORGE** on
//! the main menu opens it, on the pregame lobby's skin
//! ([`crate::lobby_start`]). Its rows are:
//!
//! - **Map**: A opens a picker of the original maps (the catalogue's Halo 2
//!   category). The first is held until another is chosen. With none
//!   installed the picker says so.
//! - **Forge map**: A opens a picker of **New** and the drafts saved under
//!   `<data>/forge` ([`forge::saved_names`]). A draft saved on another map is
//!   refused when the session loads it, with the editor's notice.
//! - **Start**: begins the local Forge session of F4: a local multiplayer
//!   game on the map ([`crate::lobby_start::start`]); the session opens
//!   itself on an original map and D-pad Up or Tab enters the monitor
//!   ([`crate::forge_glue`]). The chosen draft is loaded as the session
//!   opens ([`crate::forge_glue::request_draft`]). A refusal is on the
//!   status line, in plain words.
//!
//! The page is a kit screen ([`Lobby::page`]) over a [`Lobby`] whose maps,
//! drafts and start are injected, so the table tests step it with none of
//! the game.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::lobby_start::{self as shared, fit};
use crate::map_select::Map;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::input::{Glyph, Prompt};
use crate::ui_kit::picker::Pick;
use crate::ui_kit::screen::{KitScreen, KitTexts, Row, Then};
use crate::ui_kit::spec::ValueStyle;

/// The page's feature (`features.forge-lobby`) and screen name.
pub const FEATURE: &str = "forge-lobby";
pub const SCREEN: &str = "forge.lobby";
/// The page's string ids.
pub const IDS: Family = ids::family("forge lobby");
pub const TITLE: &str = "FORGE";
/// The draft row's first choice: no draft.
pub const NEW_MAP: &str = "New";
/// What the Map row says with no original map installed.
pub const NO_MAP: &str = "No original map is installed";

type Maps = dyn Fn() -> Vec<Map> + Send + Sync;
type Drafts = dyn Fn() -> Result<Vec<String>, String> + Send + Sync;
type Start = dyn Fn(&Map, Option<&str>) -> Result<(), String> + Send + Sync;

#[derive(Default)]
struct Choice {
    /// The map chosen, by list id: the first map while none is.
    map: Option<u32>,
    /// The draft chosen: a new map while none is.
    draft: Option<String>,
    /// Why the last Start did not begin.
    error: Option<String>,
}

/// The lobby's state and what it starts through.
pub struct Lobby {
    maps: Arc<Maps>,
    drafts: Arc<Drafts>,
    start: Arc<Start>,
    choice: Mutex<Choice>,
}

impl Lobby {
    pub fn new(maps: Arc<Maps>, drafts: Arc<Drafts>, start: Arc<Start>) -> Arc<Self> {
        Arc::new(Lobby {
            maps,
            drafts,
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
            .map_or_else(|| NO_MAP.into(), |i| maps[i].name.clone())
    }

    fn map_offer(self: &Arc<Self>) -> Result<Pick, String> {
        let maps = (self.maps)();
        if maps.is_empty() {
            return Ok(shared::nothing_to_pick(
                "Map",
                NO_MAP,
                "Forge opens on a map from the Halo 2 disc. None is installed.",
            ));
        }
        let me = Arc::clone(self);
        Ok(shared::map_pick(
            &maps,
            |map: &Map| map.name.clone(),
            self.selected(&maps).unwrap_or(0),
            "The original map Forge opens on.",
            move |map: &Map| {
                let mut c = me.choice();
                c.map = Some(map.id);
                c.error = None;
                format!("forge lobby: map {}", map.name)
            },
        ))
    }

    fn draft_shown(&self) -> String {
        self.choice()
            .draft
            .clone()
            .unwrap_or_else(|| NEW_MAP.into())
    }

    fn draft_offer(self: &Arc<Self>) -> Result<Pick, String> {
        let drafts = match (self.drafts)() {
            Ok(names) => names,
            Err(why) => {
                return Ok(shared::nothing_to_pick(
                    "Forge map",
                    "Saved maps cannot be listed",
                    &format!("{why} New still starts a fresh map."),
                ));
            }
        };
        let mut values = vec![NEW_MAP.to_owned()];
        values.extend(drafts);
        let current = self
            .choice()
            .draft
            .as_ref()
            .and_then(|d| values.iter().position(|v| v == d))
            .unwrap_or(0);
        let me = Arc::clone(self);
        let held = values.clone();
        Ok(Pick::new(
            "Forge map",
            values,
            current,
            "New starts from the map as it is. A saved map loads its edits.",
            Arc::new(move |i: usize| {
                let mut c = me.choice();
                c.error = None;
                match held.get(i) {
                    Some(name) if i > 0 => {
                        c.draft = Some(name.clone());
                        format!("forge lobby: saved map {name}")
                    }
                    _ => {
                        c.draft = None;
                        "forge lobby: a new map".into()
                    }
                }
            }),
        ))
    }

    /// Start: the session on the chosen map, or why not.
    fn press_start(&self) -> Then {
        let maps = (self.maps)();
        let Some(map) = self.selected(&maps).map(|i| maps[i].clone()) else {
            self.choice().error = Some(format!("{NO_MAP}. Forge needs one."));
            return Then::Stay(format!("forge lobby: {NO_MAP}"));
        };
        let draft = self.choice().draft.clone();
        match (self.start)(&map, draft.as_deref()) {
            Ok(()) => {
                self.choice().error = None;
                Then::Stay(format!(
                    "forge lobby: starting {} with {}",
                    map.name,
                    draft.as_deref().unwrap_or("a new map")
                ))
            }
            Err(why) => {
                self.choice().error = Some(why.clone());
                Then::Stay(format!("forge lobby: not started: {why}"))
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
        match self.selected(&maps) {
            None => fit(&format!("{NO_MAP}. Forge needs one.")),
            Some(i) => {
                let draft = self.choice().draft.clone();
                match draft {
                    Some(name) => fit(&format!("{}: saved map {name}", maps[i].name)),
                    None => fit(&format!("{}: a new map", maps[i].name)),
                }
            }
        }
    }

    /// The page.
    pub fn page(self: &Arc<Self>) -> KitScreen {
        let (shown, offer) = (Arc::clone(self), Arc::clone(self));
        let map = Row::pick("Map", move || shown.map_shown(), move || offer.map_offer())
            .describe("The original map to Forge on.");
        let (shown, offer) = (Arc::clone(self), Arc::clone(self));
        let draft = Row::pick(
            "Forge map",
            move || shown.draft_shown(),
            move || offer.draft_offer(),
        )
        .describe("A new map, or one you saved.");
        let start = Arc::clone(self);
        let go = Row::action("Start", move || start.press_start())
            .describe("Begin a local game to Forge in.");
        let status = Arc::clone(self);
        KitScreen::new(FEATURE, SCREEN, IDS, TITLE)
            .layout(shared::LAYOUT, ValueStyle::Inline)
            .live_help(shared::STATUS, move || status.status())
            .row(map)
            .row(draft)
            .row(go)
            .prompts(&[
                Prompt::new(&[Glyph::A], "Select"),
                Prompt::new(&[Glyph::B], "Back"),
            ])
    }
}

/// The lobby over the installed maps, the drafts under `data` and the
/// local game start: Start asks [`crate::forge_glue::request_draft`] to load
/// the draft, then queues the game.
pub fn lobby(data: &Path) -> Arc<Lobby> {
    let data = data.to_owned();
    Lobby::new(
        shared::maps_from(shared::forge_maps),
        Arc::new(move || ::forge::saved_names(&data)),
        Arc::new(|map: &Map, draft: Option<&str>| {
            crate::forge_glue::request_draft(draft.map(str::to_owned));
            shared::start(&shared::request(map))
        }),
    )
}

/// Adds the page and its texts.
pub fn install(registry: &mut hooks::Registry, data: &Path) -> anyhow::Result<()> {
    let screen = Arc::new(lobby(data).page());
    registry
        .add(&KitTexts(Arc::clone(&screen)))
        .map_err(|e| anyhow::anyhow!("forge lobby texts: {e}"))?;
    registry
        .add(&screen.class().panel(shared::panel()))
        .map_err(|e| anyhow::anyhow!("forge lobby page: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
