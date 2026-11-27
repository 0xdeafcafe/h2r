//! The custom game lobby's map (`h2 bots test --map NAME`): before Start
//! Game, through the game's own Game Setup and Select Map screens (docs/
//! game/bots.md, "Testing"; docs/game/map-select.md).
//!
//! The game has no setting for the lobby's map, so the driver presses it:
//!
//! * Game Setup (`squad_settings_dialog`, dialog `0x17`, [`GAME_SETUP`]) is
//!   a row of the lobby's list, and Change Map one of Game Setup's. Neither
//!   position is assumed: the harness reads the focused list with
//!   `ui.audit` ([`rows_from_audit`]), the driver finds the row by its text
//!   and presses DOWN or UP from the focused row to it, then A;
//! * Select Map lists the categories, then a category's maps. The game
//!   names each list's rows in order (`map select: rows: Halo 2, Map Pack`;
//!   `map select: rows: ascendance, beavercreek, ... zanzibar`), so the
//!   driver counts DOWN presses to the map's row instead of guessing at the
//!   catalogue's order. A category without the map is backed out of (B) and
//!   the next one tried;
//! * `map select: picked NAME` says the lobby has it. The pick returns to
//!   Game Setup; B returns to the lobby, then Start Game as before.
//!
//! The rows lines and the dialog id are the game's own; every wait has a
//! timeout that names the screen it stopped on, and a row that is not in
//! the audited list stops the drive with the rows it did see.

use super::{Driver, Step, press};
use crate::coop_drive::Press;
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
    /// The lobby's Game Setup row: waiting for the audited rows
    /// (`pressed` false), then for the dialog after the presses sent at `at`.
    Setup { at: u64, tries: u32, pressed: bool },
    /// Game Setup's Change Map row: waiting for the audited rows, then for
    /// the categories' rows after the presses sent at `at`.
    Change { at: u64, pressed: bool },
    /// A on category `cat` sent at `at`; waiting for its maps' rows.
    Category { at: u64, cat: usize },
    /// The map's row pressed at `at`; waiting for `map select: picked`.
    Picking { at: u64 },
    /// B out of Game Setup sent at `at`; the lobby is next.
    Leaving { at: u64 },
}

/// The rows of the list that has the focus, from `ui.audit`: each row's
/// index and text, and the focused row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub rows: Vec<(u32, String)>,
    pub focus: Option<u32>,
}

impl Rows {
    /// The row whose text is `label`, ignoring case and trailing dots.
    fn find(&self, label: &str) -> Option<u32> {
        let norm = |s: &str| s.trim().trim_end_matches('.').trim().to_ascii_lowercase();
        let want = norm(label);
        self.rows
            .iter()
            .find(|(_, t)| norm(t) == want)
            .map(|(i, _)| *i)
    }

    /// The presses from the focused row to row `to`, then A.
    fn go_to(&self, to: u32, label: &str) -> Vec<Press> {
        let from = self
            .focus
            .or_else(|| self.rows.first().map(|(i, _)| *i))
            .unwrap_or(0);
        let (key, n) = if to >= from {
            ("down", to - from)
        } else {
            ("up", from - to)
        };
        let mut q: Vec<Press> = (0..n)
            .map(|_| press(key, &format!("to {label} (row {to})")))
            .collect();
        q.push(press("a", label));
        q
    }

    fn said(&self) -> String {
        let t: Vec<_> = self
            .rows
            .iter()
            .map(|(i, t)| format!("{i} {t:?}"))
            .collect();
        format!("rows [{}], focus {:?}", t.join(", "), self.focus)
    }
}

/// The focused list of a `ui.audit` result (the active surface's active
/// pane: the list with a focused row, else its only list).
pub fn rows_from_audit(v: &serde_json::Value) -> Option<Rows> {
    let active = v.get("active")?;
    let pane = active.get("pane").and_then(|p| p.as_u64());
    let panes = active.get("panes")?.as_array()?;
    let pane = panes.iter().find(|p| {
        p.get("active").and_then(|a| a.as_bool()) == Some(true)
            && p.get("index").and_then(|i| i.as_u64()) == pane
    })?;
    let lists = pane.get("lists")?.as_array()?;
    let list = lists
        .iter()
        .find(|l| l.get("focused_row").is_some_and(|f| !f.is_null()))
        .or_else(|| (lists.len() == 1).then(|| &lists[0]))?;
    let rows = list
        .get("rows")?
        .as_array()?
        .iter()
        .filter_map(|r| {
            let i = r.get("index")?.as_u64()? as u32;
            Some((i, r.get("text")?.as_str()?.to_string()))
        })
        .collect();
    let focus = list
        .get("focused_row")
        .and_then(|f| f.as_u64())
        .map(|f| f as u32);
    Some(Rows { rows, focus })
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
            self.queue.clear();
            self.step = Step::Map(MapStep::Setup {
                at: now,
                tries: 0,
                pressed: false,
            });
        }
        true
    }

    /// Whether the driver waits on the focused list's rows (`ui.audit`),
    /// for the lobby's Game Setup row or Game Setup's Change Map row.
    pub fn wants_rows(&self) -> bool {
        matches!(
            self.step,
            Step::Map(
                MapStep::Setup { pressed: false, .. } | MapStep::Change { pressed: false, .. }
            )
        )
    }

    /// The focused list the harness read (`ui.audit`).
    pub fn rows(&mut self, rows: Rows) {
        self.last_rows = Some(rows.clone());
        self.rows = Some(rows);
    }

    fn rows_said(&self) -> String {
        self.last_rows
            .as_ref()
            .map_or("no rows read".into(), Rows::said)
    }

    fn stop(&mut self, what: &str) {
        let ui = self.ui.clone();
        self.give_up(format!("{what}; the last screen: {ui}"));
    }

    pub(super) fn on_map(&mut self, now: u64, step: MapStep) {
        let want = self.map.clone().unwrap_or_default();
        match step {
            MapStep::Setup {
                at,
                tries,
                pressed: false,
            } => {
                let found = self
                    .rows
                    .take()
                    .and_then(|r| r.find("Game Setup").map(|i| (i, r)));
                if let Some((i, r)) = found {
                    self.queue = r.go_to(i, "Game Setup");
                    self.step = Step::Map(MapStep::Setup {
                        at: now,
                        tries,
                        pressed: true,
                    });
                } else if now.saturating_sub(at) > STEP_MS {
                    let seen = self.rows_said();
                    self.stop(&format!(
                        "the lobby's list has no Game Setup row for --map {want} ({seen})"
                    ));
                }
            }
            MapStep::Setup {
                at,
                tries,
                pressed: true,
            } => {
                if self.dialog_id == Some(GAME_SETUP) && self.quiet(now, SETTLE_MS) {
                    self.rows = None;
                    self.step = Step::Map(MapStep::Change {
                        at: now,
                        pressed: false,
                    });
                } else if self.queue.is_empty() && now.saturating_sub(at) > STEP_MS {
                    if tries + 1 >= TRIES {
                        self.stop(&format!(
                            "Game Setup (dialog {GAME_SETUP:#x}) never opened from the lobby \
                             for --map {want}"
                        ));
                    } else {
                        self.rows = None;
                        self.step = Step::Map(MapStep::Setup {
                            at: now,
                            tries: tries + 1,
                            pressed: false,
                        });
                    }
                }
            }
            MapStep::Change { at, pressed: false } => {
                let found = self
                    .rows
                    .take()
                    .and_then(|r| r.find("Change Map").map(|i| (i, r)));
                if let Some((i, r)) = found {
                    self.queue = r.go_to(i, "Change Map");
                    self.step = Step::Map(MapStep::Change {
                        at: now,
                        pressed: true,
                    });
                } else if now.saturating_sub(at) > STEP_MS {
                    let seen = self.rows_said();
                    self.stop(&format!("Game Setup has no Change Map row ({seen})"));
                }
            }
            MapStep::Change { at, pressed: true } => {
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
