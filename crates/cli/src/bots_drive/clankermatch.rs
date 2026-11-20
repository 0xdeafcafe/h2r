//! The drive's Clankermatch path (`h2 bots test --clankermatch`,
//! docs/game/bots.md §8 and §13): the same boot, sign-in and Xbox Live
//! menu as the custom game's, then
//!
//! * the Live menu's Clankermatch row (row 2), A. The game says
//!   `clankermatch: the playlists are the clanker hoppers` and opens the
//!   OptiMatch playlist list with only the clanker hoppers on it, in
//!   hopper order (Clanker Slayer, Teams, Doubles on rows 0 to 2 with the
//!   built-in lineup). The list is the first settled screen after A; its
//!   cursor row is its list value's low 16 bits, or, when that reads as no
//!   row, the DOWN presses sent from row 0;
//! * to the planned playlist's row, A; the HLE's search starts
//!   (`live: clankermatch: searching clanker playlist H ...`); a human
//!   playlist's search (`live: optimatch: searching playlist`) stops the
//!   drive;
//! * nothing more to press: after 4 to 20 s the game hosts
//!   (`clankers: clankermatch: hosting clanker playlist H ...`), the fill
//!   seats the phantoms in the lobby and the lobby's countdown starts
//!   the game once it has the playlist's fewest players;
//! * the game: wait for phase `gameplay`, then for the menus again (the
//!   game over);
//! * out: B (A on a dialog, the second dialog DOWN then A) until this
//!   console hosts no session and the screen holds, so the fill ends and
//!   the bots go.

use super::{CUSTOM_GAME_ROW, DIALOG_HOLD_MS, Driver, Step, Target, press};
use crate::coop_drive::{SETTLE_MS, STEP_MS, TRIES};

/// The Xbox Live menu's Clankermatch row (`boot::custom_games`).
pub const CLANKERMATCH_ROW: u32 = 2;
/// The Clankermatch playlist list's rows (the built-in lineup's three
/// clanker hoppers).
pub const PLAYLIST_ROWS: u32 = 3;
/// Presses on the playlist list before giving up.
pub const PLAYLIST_PRESSES: u32 = 8;
/// The HLE's clanker search line.
pub const SEARCH: &str = "live: clankermatch: searching clanker playlist ";
/// The HLE's human playlist search line.
pub const HUMAN_SEARCH: &str = "live: optimatch: searching playlist ";
/// The Clankers driver's fill of a hosted clanker lobby.
pub const HOSTING: &str = "clankers: clankermatch: hosting clanker playlist ";
/// How long the search may run before this console hosts: the HLE holds
/// it 4 to 20 s, then the game widens its search and hosts.
pub const HOST_MS: u64 = 90_000;
/// How long the hosted lobby may take to start the game: the fill seats
/// its bots 0.5 to 3 s apart, then the countdown.
pub const MATCH_START_MS: u64 = 180_000;
/// On the way out, a screen held this long gets the next press.
pub const LEAVE_HOLD_MS: u64 = 2_500;
/// Out: no session hosted and no screen change or press for this long.
pub const LEFT_QUIET_MS: u64 = 5_000;
/// Presses on the way out before giving up.
pub const LEAVE_PRESSES: u32 = 12;

impl Driver {
    /// A driver for Clankermatch: the clanker playlist on `row` of the
    /// list (every bot a phantom, none to wait for in the party's lobby).
    pub fn clankermatch(row: u32) -> Self {
        Driver {
            target: Target::Clankermatch { row },
            ..Driver::new(0)
        }
    }

    /// The Live menu row the drive goes for, and its name.
    pub(super) fn live_row(&self) -> (u32, &'static str) {
        match self.target {
            Target::CustomGame => (CUSTOM_GAME_ROW, "Custom Game"),
            Target::Clankermatch { .. } => (CLANKERMATCH_ROW, "Clankermatch"),
        }
    }

    /// A went on the Clankermatch row at `at`: on the playlist list, to
    /// the planned row, A.
    pub(super) fn on_playlists(&mut self, now: u64, at: u64, tries: u32, downs: u32, presses: u32) {
        let Target::Clankermatch { row: want } = self.target else {
            return;
        };
        if self.ui_since <= at {
            // No screen since A.
            if now.saturating_sub(at.max(self.last_press)) > STEP_MS {
                if tries + 1 >= TRIES {
                    let ui = self.ui.clone();
                    self.give_up(format!(
                        "A on Clankermatch opened no playlist list after {TRIES} tries; the last \
                         screen: {ui}"
                    ));
                } else if self.live_menu() {
                    self.say("still on the Xbox Live menu after A on Clankermatch; again".into());
                    self.at_live_menu(now, tries + 1);
                } else {
                    self.step = Step::Playlists {
                        at: now,
                        tries: tries + 1,
                        downs,
                        presses,
                    };
                }
            }
            return;
        }
        if !self.quiet(now, SETTLE_MS) {
            return;
        }
        if self.dialog() {
            let (ui, name) = (self.ui.clone(), self.dialog_name());
            self.give_up(format!(
                "{name} over the Clankermatch playlists (an empty list?): {ui}"
            ));
            return;
        }
        if presses >= PLAYLIST_PRESSES {
            let ui = self.ui.clone();
            self.give_up(format!(
                "no clanker playlist picked after {PLAYLIST_PRESSES} presses on the list; the last \
                 screen: {ui}"
            ));
            return;
        }
        let read = self
            .list_value
            .map(|v| v & 0xffff)
            .filter(|r| *r < PLAYLIST_ROWS);
        let from = read.unwrap_or(downs);
        let unread = if read.is_none() {
            ", the row unread"
        } else {
            ""
        };
        let (button, why) = if from < want {
            (
                "down",
                format!("toward the clanker playlist (row {want}, from {from}{unread})"),
            )
        } else if from > want {
            (
                "up",
                format!("toward the clanker playlist (row {want}, from {from}{unread})"),
            )
        } else {
            ("a", format!("the clanker playlist on row {want}: search"))
        };
        if presses == 0 {
            self.say(format!("on the Clankermatch playlist list: {}", self.ui));
        }
        self.queue = vec![press(button, &why)];
        self.step = match button {
            "a" => Step::Searching { at: now, tries: 0 },
            b => Step::Playlists {
                at,
                tries,
                downs: if b == "down" {
                    downs + 1
                } else {
                    downs.saturating_sub(1)
                },
                presses: presses + 1,
            },
        };
    }

    /// A on the playlist went at `at`: the clanker search, or why not.
    pub(super) fn searching(&mut self, now: u64, at: u64, tries: u32) {
        if self.cm_search {
            self.step = Step::Matching { since: now };
            self.say("the clanker search runs".into());
            return;
        }
        if let Some(line) = self.human_search.clone() {
            self.give_up(format!(
                "a human playlist's search started, not a clanker one (Clankermatch was off, or \
                 the HLE has no marks): {line}"
            ));
            return;
        }
        if now.saturating_sub(at.max(self.last_press)) <= STEP_MS {
            return;
        }
        if tries + 1 >= TRIES {
            let ui = self.ui.clone();
            self.give_up(format!(
                "A on the clanker playlist started no search after {TRIES} tries; the last \
                 screen: {ui}"
            ));
        } else if self.ui_since <= at && !self.dialog() {
            // The list still: A again.
            self.queue = vec![press("a", "the clanker playlist again: no search seen")];
            self.step = Step::Searching {
                at: now,
                tries: tries + 1,
            };
        } else {
            self.step = Step::Searching {
                at: now,
                tries: tries + 1,
            };
        }
    }

    /// The search runs: waiting for this console to host its lobby.
    pub(super) fn matching(&mut self, now: u64, since: u64) {
        if self.cm_hosting {
            self.step = Step::MatchLobby { since: now };
            self.say("hosting the Clankermatch lobby: the bots fill it".into());
        } else if now.saturating_sub(since) > HOST_MS {
            self.give_up(format!(
                "the clanker search ran {} s and this console never hosted its lobby (no \
                 `clankers: clankermatch: hosting clanker playlist`)",
                HOST_MS / 1000
            ));
        }
    }

    /// In the hosted lobby: nothing to press; the phase says when it
    /// starts ([`Driver::match_phase`]).
    pub(super) fn match_lobby(&mut self, now: u64, since: u64) {
        if now.saturating_sub(since) > MATCH_START_MS {
            self.give_up(format!(
                "the Clankermatch lobby did not start in {} s ({} phantoms joined)",
                MATCH_START_MS / 1000,
                self.in_lobby.len(),
            ));
        }
    }

    /// The phase, for the Clankermatch steps: the start, the game, its end.
    pub(super) fn match_phase(&mut self, _now: u64, phase: &str) {
        match self.step {
            Step::Matching { .. } | Step::MatchLobby { .. } if phase != "menu" => {
                self.queue.clear();
                self.step = Step::Playing {
                    played: phase == "gameplay",
                };
                self.say(format!("the Clankermatch game started (phase {phase})"));
            }
            Step::Playing { played: false } if phase == "gameplay" => {
                self.step = Step::Playing { played: true };
            }
            Step::Playing { played: true } if phase == "menu" => {
                self.queue.clear();
                self.step = Step::Leaving {
                    presses: 0,
                    last_b: false,
                    dialogs: 0,
                };
                self.say("the game is over (phase menu): leaving, so the bots go".into());
            }
            Step::Leaving { .. } if phase == "gameplay" => {
                self.queue.clear();
                self.step = Step::Playing { played: true };
                self.say("another game started before the drive left: waiting for its end".into());
            }
            _ => {}
        }
    }

    /// Out of the Clankermatch: B (A on a dialog) until this console hosts
    /// no session.
    pub(super) fn leaving(&mut self, now: u64, presses: u32, last_b: bool, dialogs: u32) {
        if !self.hosting && !self.dialog() {
            // Out (or never hosting after the game): nothing to press.
            if self.quiet(now, LEFT_QUIET_MS) {
                self.step = Step::Done;
                self.say(
                    "left the Clankermatch: this console hosts no session; the fill ends 30 s on"
                        .into(),
                );
            }
            return;
        }
        let hold = if self.dialog() {
            DIALOG_HOLD_MS
        } else {
            LEAVE_HOLD_MS
        };
        if !self.quiet(now, hold) {
            return;
        }
        if presses >= LEAVE_PRESSES {
            let ui = self.ui.clone();
            self.give_up(format!(
                "still hosting after {LEAVE_PRESSES} presses to leave the Clankermatch; the last \
                 screen: {ui}"
            ));
            return;
        }
        let unchanged = self.ui_since < self.last_press;
        let (queue, b, dialogs) = if self.dialog() {
            let name = self.dialog_name();
            if dialogs == 0 {
                (
                    vec![press("a", &format!("{name} on the way out: A"))],
                    false,
                    1,
                )
            } else {
                (
                    vec![
                        press(
                            "down",
                            &format!("{name} on the way out again: its second button"),
                        ),
                        press(
                            "a",
                            &format!("{name} on the way out: A on its second button"),
                        ),
                    ],
                    false,
                    dialogs + 1,
                )
            }
        } else if last_b && unchanged {
            (
                vec![press("a", "B changed nothing (the carnage report?): A")],
                false,
                dialogs,
            )
        } else {
            (
                vec![press("b", "back out of the Clankermatch lobby")],
                true,
                dialogs,
            )
        };
        self.queue = queue;
        self.step = Step::Leaving {
            presses: presses + 1,
            last_b: b,
            dialogs,
        };
    }
}

#[cfg(test)]
mod tests;
