//! The custom game lobby's map (`h2 bots test --map NAME`): before Start
//! Game, through the game's own Game Setup and Select Map screens (docs/
//! game/bots.md, "Testing"; docs/game/map-select.md).
//!
//! The game has no setting for the lobby's map, so the driver presses it:
//!
//! * Game Setup (`squad_settings_dialog`, dialog `0x17`, [`GAME_SETUP`]) is
//!   the lobby's row under Start Game: DOWN, A. Change Map is its first
//!   row in a custom game: A;
//! * Select Map lists the categories, then a category's maps. The game
//!   names each list's rows in order (`map select: rows: Halo 2, Map Pack`;
//!   `map select: rows: ascendance, beavercreek, ... zanzibar`), so the
//!   driver counts DOWN presses to the map's row instead of guessing at the
//!   catalogue's order. A category without the map is backed out of (B) and
//!   the next one tried;
//! * `map select: picked NAME` says the lobby has it. The pick returns to
//!   Game Setup; B returns to the lobby, then Start Game as before.
//!
//! The rows lines and the dialog id are the game's own; the path above is
//! the docs' account of the screens, not yet seen in a run, so every wait
//! has a timeout that names the screen it stopped on.

use super::{Driver, Step, press};
use crate::coop_drive::{SETTLE_MS, STEP_MS, TRIES};

/// Game Setup's dialog id (screen 23).
pub const GAME_SETUP: u32 = 0x17;
/// `map select: Select Map lists Categories (4 rows)`.
const LISTS: &str = "map select: Select Map lists ";
/// `map select: rows: a, b, c`.
const ROWS: &str = "map select: rows: ";
/// `map select: picked zanzibar`.
const PICKED: &str = "map select: picked ";

/// Where the drive is on the way to the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MapStep {
    /// DOWN, A on the lobby's Game Setup row sent at `at`.
    Setup { at: u64, tries: u32 },
    /// A on Change Map sent at `at`; waiting for the categories' rows.
    Change { at: u64 },
    /// A on category `cat` sent at `at`; waiting for its maps' rows.
    Category { at: u64, cat: usize },
    /// The map's row pressed at `at`; waiting for `map select: picked`.
    Picking { at: u64 },
    /// B out of Game Setup sent at `at`; the lobby is next.
    Leaving { at: u64 },
}

/// What the game said of its Select Map list: the list kind, then its rows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Seen {
    pub kind: Option<String>,
    pub rows: Option<Vec<String>>,
    pub picked: Option<String>,
    /// The categories the game listed.
    pub categories: usize,
}

impl Driver {
    /// The lobby's map to pick before Start Game (`--map`).
    pub fn with_map(self, map: Option<String>) -> Self {
        Driver { map, ..self }
    }

    /// One of the game's lines about Select Map.
    pub(super) fn map_line(&mut self, text: &str) {
        if let Some(r) = text.strip_prefix(LISTS) {
            self.seen.kind = Some(r.split(' ').next().unwrap_or("").to_string());
            self.seen.rows = None;
        } else if let Some(r) = text.strip_prefix(ROWS) {
            let rows: Vec<String> = r.split(", ").map(|s| s.trim().to_string()).collect();
            if self.seen.kind.as_deref() == Some("Categories") {
                self.seen.categories = rows.len();
            }
            self.seen.rows = Some(rows);
        } else if let Some(r) = text.strip_prefix(PICKED) {
            self.seen.picked = Some(r.trim().to_string());
        }
    }

    /// In the lobby with a map still to pick: starts the drive to it.
    /// Whether the lobby step is taken up with it.
    pub(super) fn begin_map(&mut self, now: u64) -> bool {
        if self.map.is_none() || self.map_done {
            return false;
        }
        if now.saturating_sub(self.ui_since.max(self.last_press)) >= SETTLE_MS {
            self.seen = Seen::default();
            self.queue = vec![
                press("down", "to Game Setup, for the map"),
                press("a", "Game Setup"),
            ];
            self.step = Step::Map(MapStep::Setup { at: now, tries: 0 });
        }
        true
    }

    fn stop(&mut self, what: &str) {
        let ui = self.ui.clone();
        self.give_up(format!("{what}; the last screen: {ui}"));
    }

    pub(super) fn on_map(&mut self, now: u64, step: MapStep) {
        let want = self.map.clone().unwrap_or_default();
        match step {
            MapStep::Setup { at, tries } => {
                if self.dialog_id == Some(GAME_SETUP) && self.quiet(now, SETTLE_MS) {
                    self.queue = vec![press("a", "Change Map")];
                    self.step = Step::Map(MapStep::Change { at: now });
                } else if now.saturating_sub(at) > STEP_MS {
                    if tries + 1 >= TRIES {
                        self.stop(&format!(
                            "Game Setup (dialog {GAME_SETUP:#x}) never opened from the lobby \
                             for --map {want}"
                        ));
                    } else {
                        self.queue =
                            vec![press("down", "toward Game Setup"), press("a", "Game Setup")];
                        self.step = Step::Map(MapStep::Setup {
                            at: now,
                            tries: tries + 1,
                        });
                    }
                }
            }
            MapStep::Change { at } => {
                if self.seen.kind.as_deref() == Some("Categories") && self.seen.rows.is_some() {
                    self.seen.rows = None;
                    self.queue = vec![press("a", "the first category")];
                    self.step = Step::Map(MapStep::Category { at: now, cat: 0 });
                } else if now.saturating_sub(at) > STEP_MS {
                    self.stop("Change Map never listed the map categories (`map select: rows:`)");
                }
            }
            MapStep::Category { at, cat } => {
                let maps = self.seen.kind.as_deref().is_some_and(|k| k != "Categories");
                match self.seen.rows.clone().filter(|_| maps) {
                    Some(rows) => {
                        if let Some(i) = rows.iter().position(|r| r.eq_ignore_ascii_case(&want)) {
                            let mut q: Vec<_> = (0..i)
                                .map(|_| press("down", &format!("to {want} (row {i})")))
                                .collect();
                            q.push(press("a", &format!("pick {want}")));
                            self.queue = q;
                            self.step = Step::Map(MapStep::Picking { at: now });
                        } else if cat + 1 < self.seen.categories {
                            self.seen.rows = None;
                            self.seen.kind = None;
                            self.queue = vec![
                                press("b", &format!("back: {want} is not in category {cat}")),
                                press("down", "to the next category"),
                                press("a", "the next category"),
                            ];
                            self.step = Step::Map(MapStep::Category {
                                at: now,
                                cat: cat + 1,
                            });
                        } else {
                            self.stop(&format!("no category lists a map {want:?} (--map)"));
                        }
                    }
                    None if now.saturating_sub(at) > STEP_MS => {
                        self.stop(&format!(
                            "category {cat} never listed its maps for --map {want}"
                        ));
                    }
                    None => {}
                }
            }
            MapStep::Picking { at } => {
                let said = self.seen.picked.clone();
                match said {
                    Some(p) if p.eq_ignore_ascii_case(&want) => {
                        self.queue = vec![press("b", "out of Game Setup: the map is picked")];
                        self.step = Step::Map(MapStep::Leaving { at: now });
                    }
                    Some(p) => self.stop(&format!("the lobby took map {p:?}, not --map {want}")),
                    None if now.saturating_sub(at) > STEP_MS => {
                        self.stop(&format!(
                            "A on {want} never said `map select: picked {want}`"
                        ));
                    }
                    None => {}
                }
            }
            MapStep::Leaving { at } => {
                if self.queue.is_empty()
                    && self.dialog_id != Some(GAME_SETUP)
                    && self.quiet(now, SETTLE_MS)
                {
                    self.map_done = true;
                    self.step = Step::Lobby { since: now };
                    self.say(format!("the lobby's map is {want}"));
                } else if self.queue.is_empty() && now.saturating_sub(at) > STEP_MS {
                    self.queue = vec![press("b", "out of Game Setup")];
                    self.step = Step::Map(MapStep::Leaving { at: now });
                }
            }
        }
    }
}
