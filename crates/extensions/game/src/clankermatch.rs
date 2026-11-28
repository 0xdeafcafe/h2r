//! Clankermatch (docs/game/bots.md §8): matchmaking where every other
//! player is a Clanker this console runs.
//!
//! **The row.** The Xbox Live menu's Clankermatch row, after OptiMatch
//! (`crate::custom_games`), runs the OptiMatch row's press with
//! [`active`] set; every other row's press clears it.
//!
//! **The playlists.** The OptiMatch playlist screen (loader `0x00230c8d`)
//! builds its rows in `0x002b1ff4`, which asks
//! [`HOPPER_LISTED`](guest_layout::points::HOPPER_LISTED) (`0x00192db0`, ECX the
//! descriptor index, AL the answer) once per hopper and stores the index in
//! each row. A `before` on it answers "not listed" for a hopper outside
//! the [`Scope`] the opening press set: Clankermatch only the server's
//! clanker hoppers (`live_hle::Hle::clanker_hoppers`), OptiMatch and the
//! party lobby's MULTIPLAYER mode both kinds on one page
//! (docs/game/party-lobby.md, "The modes"). A hopper it lets through
//! still goes to the game's own check.
//!
//! **Quickmatch** searches the playlist the player last picked in
//! OptiMatch, which the playlist list's press saves (`0x001902fc`, from
//! `0x002b21e3`). A clanker hopper is never saved: a `before` on
//! [`PLAYLIST_REMEMBER`](guest_layout::points::PLAYLIST_REMEMBER) skips the save
//! during Clankermatch or for a clanker hopper, so the human playlist
//! picked before stays. And an `after` on
//! [`QUICKMATCH_HOPPER`](guest_layout::points::QUICKMATCH_HOPPER) answers "none"
//! (-1, and Quickmatch starts nothing) for a clanker hopper saved anyway
//! (a profile saved before the server marked it). See [`remembered`] and
//! [`quickmatch_hopper`].
//!
//! **The search** is the HLE's (`live_hle::clankermatch`): a clanker
//! hopper's search finds no session after 4 to 20 s, and the game hosts.
//!
//! **The lobby's switch.** A Live lobby's Game Setup dialog has retail's
//! "Switch To: Matchmaking" row (action 8, or 9 in one lobby kind), whose
//! press (`0x002bba8c`) opens the OptiMatch playlist screen through
//! `0x002bbf06` and closes the dialog. [`SwitchRow`] adds "Switch To:
//! Clankermatch" ([`SWITCH_ACTION`]) right under it while the server has
//! clanker hoppers; its press sets [`active`] and runs that same press, so
//! the party goes to the clanker playlists. Retail's row sets
//! [`Scope::Players`] first, so it always lists the human ones. In a
//! matchmaking lobby (the party lobby's MULTIPLAYER mode) action 9 is
//! Change Playlist... ([`CHANGE_PLAYLIST`]), and its press lists every
//! hopper ([`Scope::All`]). The party lobby's Game Setup drops both
//! Switch To rows (`crate::lobby_mode::arrange`): the GAME MODE picker
//! changes mode.
//!
//! **The fill.** While this console hosts a clanker hopper's lobby, the
//! Clankers driver (`crate::clankers`) asks Live for the playlist's roster
//! and seats its phantoms one at a time ([`schedule`]) in the match session.
//! They fill the lobby to the playlist's minimum player count, on teams
//! [`balance`] picks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use live_proto::hopper::clankers::ClankerHoppers;
use runtime::GuestFault;

use crate::h1_options_row as h1row;

/// The feature (`features.clankermatch`).
pub const FEATURE: &str = "clankermatch";
/// The lobby's "Switch To: Clankermatch" row (`features.clankermatch-switch`).
pub const SWITCH_FEATURE: &str = "clankermatch-switch";

/// Which hoppers the playlist screen lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The ordinary ones: the Xbox Live menu's OptiMatch row, and retail's
    /// "Switch To: Matchmaking".
    Players,
    /// The clanker ones: the Xbox Live menu's Clankermatch row and "Switch
    /// To: Clankermatch".
    Clankers,
    /// Both, on one page: the party lobby's MULTIPLAYER mode and its Change
    /// Playlist... row (docs/game/party-lobby.md, "The modes").
    All,
    /// First-time Quickmatch opens the combined picker under its own title.
    QuickMatch,
}

impl Scope {
    const fn byte(self) -> u8 {
        match self {
            Scope::Players => 0,
            Scope::Clankers => 1,
            Scope::All => 2,
            Scope::QuickMatch => 3,
        }
    }
    const fn of_byte(b: u8) -> Scope {
        match b {
            1 => Scope::Clankers,
            2 => Scope::All,
            3 => Scope::QuickMatch,
            _ => Scope::Players,
        }
    }
}

/// The scope the next playlist screen lists: set by the press that opens
/// it.
static SCOPE: AtomicU8 = AtomicU8::new(0);

/// Which hoppers the playlist screen lists now.
pub fn scope() -> Scope {
    Scope::of_byte(SCOPE.load(Ordering::Relaxed))
}

/// From the press that opens the playlist screen.
pub fn set_scope(scope: Scope) {
    let was = Scope::of_byte(SCOPE.swap(scope.byte(), Ordering::Relaxed));
    if was != scope {
        eprintln!(
            "clankermatch: {}",
            match scope {
                Scope::Players => "the playlists are the human hoppers",
                Scope::Clankers => "the playlists are the clanker hoppers",
                Scope::QuickMatch | Scope::All => "the playlists are every hopper, human and clanker",
            }
        );
    }
}

/// Whether the playlist screen is Clankermatch's alone.
pub fn active() -> bool {
    scope() == Scope::Clankers
}

/// Which descriptors the opening menu action includes on the playlist list.
pub fn listed(index: u32, marks: &ClankerHoppers, scope: Scope) -> bool {
    match scope {
        Scope::QuickMatch | Scope::All => true,
        Scope::Clankers => marks.contains(index),
        Scope::Players => !marks.contains(index),
    }
}

/// Whether the lobby's playlist `hopper` is a Clanker one.
pub fn is_clanker(hopper: u32, marks: &ClankerHoppers) -> bool {
    marks.contains(hopper)
}

/// No hopper: what Quickmatch's getter answers for a player with no saved
/// playlist.
pub const NO_HOPPER: u32 = u32::MAX;

/// Whether the OptiMatch press saves `hopper` for Quickmatch: not in
/// Clankermatch, and never a clanker hopper (on the one page too, where
/// only an ordinary hopper is saved).
pub fn remembered(hopper: u32, marks: &ClankerHoppers, scope: Scope) -> bool {
    scope != Scope::Clankers && !marks.contains(hopper)
}

/// The hopper Quickmatch searches, from the one saved: a clanker hopper
/// becomes [`NO_HOPPER`]; anything else stays.
pub fn quickmatch_hopper(saved: u32, marks: &ClankerHoppers) -> u32 {
    if saved != NO_HOPPER && marks.contains(saved) {
        NO_HOPPER
    } else {
        saved
    }
}

// ---- the lobby's "Switch To: Clankermatch" row ----

/// Retail's "Switch To: Matchmaking" actions on a Game Setup list: both go
/// to `0x002bba8c` in the row-chosen jump table (`0x002bb884`). The
/// constructor (`0x002bb4d2`) gives lobby kinds 4 and 5 action 8, kind 6
/// action 9.
pub const MATCHMAKING_SWITCHES: [i16; 2] = [8, 9];
/// Our row's action: past the Halo 1 Options row's 10 and the Clankers
/// row's 11. The row-chosen jump table has no bounds check, so its press
/// must never reach the original.
pub const SWITCH_ACTION: i16 = 12;
/// Retail's "Switch To: Matchmaking" press (EAX the press's player chain,
/// the list handler on the stack, `ret 4`): opens the OptiMatch playlist
/// screen (`0x002bbf06`) for the chain's player, then closes the dialog
/// (`0x0022eeee`, `0x0022e957`).
pub const SWITCH_TO_MATCHMAKING: u32 = 0x002b_ba8c;

/// Whether a Game Setup list with these actions gets "Switch To:
/// Clankermatch": it has retail's "Switch To: Matchmaking", the server
/// has clanker hoppers, the party isn't on Clankermatch already
/// (`on_clankermatch`, [`active`]), and the row is not there yet.
pub fn wants_switch_row(actions: &[i16], clanker_hoppers: bool, on_clankermatch: bool) -> bool {
    clanker_hoppers
        && !on_clankermatch
        && actions.iter().any(|a| MATCHMAKING_SWITCHES.contains(a))
        && !actions.contains(&SWITCH_ACTION)
}

/// The list's actions with our row right under the first "Switch To:
/// Matchmaking" (at the end when there is none).
pub fn switch_placed(actions: &[i16]) -> Vec<i16> {
    let at = actions
        .iter()
        .position(|a| MATCHMAKING_SWITCHES.contains(a))
        .map_or(actions.len(), |i| i + 1);
    let mut out = actions.to_vec();
    out.insert(at, SWITCH_ACTION);
    out
}

/// Retail's action 9: in a matchmaking lobby, Change Playlist...
/// (`crate::lobby_mode::CHANGE_PLAYLIST`), the same press as "Switch To:
/// Matchmaking".
pub const CHANGE_PLAYLIST: i16 = 9;

/// What a Game Setup press of `action` means for the playlists: ours lists
/// the clanker ones, retail's "Switch To: Matchmaking" the human ones, and
/// a matchmaking lobby's Change Playlist... both on one page; `None` any
/// other row.
pub fn switch_press(action: i16) -> Option<Scope> {
    if action == SWITCH_ACTION {
        Some(Scope::Clankers)
    } else if action == CHANGE_PLAYLIST {
        Some(Scope::All)
    } else if MATCHMAKING_SWITCHES.contains(&action) {
        Some(Scope::Players)
    } else {
        None
    }
}

/// The "Switch To: Clankermatch" row on a Live lobby's Game Setup dialog.
pub struct SwitchRow {
    /// `None` with no Live: no clanker hoppers, so no row. Placed anyway,
    /// so the boot plan always holds the hooks.
    pub hle: Option<Arc<live_hle::Hle>>,
}

fn switch_text(c: &mut Call, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let item = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + 0x70).ok())
            .and_then(|row| h1row::row_action(&*mem, c.cpu.ecx, row));
        item.filter(|_| action == Some(SWITCH_ACTION))
    };
    let Some(item) = item else {
        return Ok(Flow::Continue);
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("clankermatch: Game Setup's Switch To: Clankermatch row text");
    }
    let id = crate::custom_games::SWITCH_CLANKERMATCH_TEXT_ID;
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            crate::list_class::draw_texts(&cx, item, vec![(0, id)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn switch_chosen(c: &mut Call) -> Result<Flow, GuestFault> {
    let list = c.cpu.ecx;
    let (chain, action) = {
        let mem = c.ctx.runtime().memory();
        let row = mem.u32(c.cpu.esp + 8).ok().and_then(|p| mem.u32(p).ok());
        (
            mem.u32(c.cpu.esp + 4).ok(),
            row.and_then(|row| h1row::row_action(&*mem, list, row)),
        )
    };
    let Some(scope) = action.and_then(switch_press) else {
        return Ok(Flow::Continue);
    };
    set_scope(scope);
    if scope != Scope::Clankers {
        // Retail's press: Switch To: Matchmaking's human playlists, or a
        // matchmaking lobby's Change Playlist... with every playlist.
        return Ok(Flow::Continue);
    }
    let Some(chain) = chain else {
        eprintln!("clankermatch: Switch To: Clankermatch: no player chain; nothing opened");
        return c.ret(8).map(Flow::Return);
    };
    eprintln!("clankermatch: Switch To: Clankermatch: opening the clanker playlists");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.call(
                GuestCall::new(SWITCH_TO_MATCHMAKING)
                    .eax(chain)
                    .args([list]),
            )
            .await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl hooks::Feature for SwitchRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            SWITCH_FEATURE,
            "Clankermatch: a \"Switch To: Clankermatch\" row under a Live lobby's \
             \"Switch To: Matchmaking\"",
        )
        .doc("docs/game/bots.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use guest_layout::points::{
            SQUAD_SETTINGS_LIST_CTOR as CTOR, SQUAD_SETTINGS_ROW_CHOSEN, SQUAD_SETTINGS_ROW_TEXT,
        };
        // The handler being built (ESI at entry); a stack in case of nesting.
        let building: Arc<std::sync::Mutex<Vec<u32>>> = Arc::default();
        let (b2, live) = (Arc::clone(&building), Arc::clone(&building));
        b.raw()
            .before(&CTOR, "note the Game Setup list", move |c| {
                b2.lock().unwrap_or_else(|e| e.into_inner()).push(c.cpu.esi);
                Ok(Flow::Continue)
            })
            .after_feature(h1row::ROW_FEATURE)
            .after_feature(crate::clankers::ROW_FEATURE);
        let hle = self.hle.clone();
        b.raw()
            .after(&CTOR, "add the Switch To: Clankermatch row", move |c| {
                let Some(list) = building.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                    return Ok(());
                };
                let marks = hle
                    .as_ref()
                    .is_some_and(|h| !h.clanker_hoppers().hoppers.is_empty());
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let actions = h1row::setup_actions(&*mem, list).unwrap_or_default();
                if !wants_switch_row(&actions, marks, active()) {
                    return Ok(());
                }
                match h1row::insert_row_ordered(&mut *mem, list, switch_placed) {
                    Ok(n) => eprintln!(
                        "clankermatch: Game Setup list: Switch To: Clankermatch row added ({n} rows)"
                    ),
                    Err(why) => eprintln!(
                        "clankermatch: Game Setup list: Switch To: Clankermatch row not added: {why}"
                    ),
                }
                Ok(())
            })
            .after_feature(h1row::ROW_FEATURE)
            .after_feature(crate::clankers::ROW_FEATURE);
        let seen = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(
                &SQUAD_SETTINGS_ROW_TEXT,
                "the Switch To: Clankermatch row's text",
                move |c| switch_text(c, &seen),
            )
            .after_feature(h1row::ROW_FEATURE)
            .after_feature(crate::clankers::ROW_FEATURE);
        b.raw()
            .before(
                &SQUAD_SETTINGS_ROW_CHOSEN,
                "Switch To: Clankermatch, and Switch To: Matchmaking's human playlists",
                switch_chosen,
            )
            .after_feature(h1row::ROW_FEATURE)
            .after_feature(crate::clankers::ROW_FEATURE);
        // Never snapshot while a Game Setup list is being built.
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            h1row::building_live(&live)
        }));
        Ok(())
    }
}

// ---- the fill ----

/// The shortest and longest wait before each bot joins (§8.4 item 2).
pub const MIN_GAP: Duration = Duration::from_millis(500);
pub const MAX_GAP: Duration = Duration::from_millis(3000);

/// SplitMix64: a small, seeded generator for the schedule.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// When each of `bots` bots joins, from the lobby's start: each one
/// [`MIN_GAP`] to [`MAX_GAP`] after the one before.
pub fn schedule(bots: usize, seed: u64) -> Vec<Duration> {
    let span = (MAX_GAP - MIN_GAP).as_millis() as u64 + 1;
    let mut state = seed;
    let mut at = Duration::ZERO;
    (0..bots)
        .map(|_| {
            at += MIN_GAP + Duration::from_millis(splitmix(&mut state) % span);
            at
        })
        .collect()
}

/// How many of a schedule's bots are in by `elapsed`.
pub fn seated(schedule: &[Duration], elapsed: Duration) -> usize {
    schedule.iter().take_while(|t| **t <= elapsed).count()
}

/// The teams of `skills` bots (in roster order) in a game of `teams`
/// teams with the humans' `humans` (each a team and a skill), by §9.1's
/// rule "the host balances by total skill": each bot goes to the team
/// with the fewest players, of those the lowest total skill, of those the
/// lowest index. With no teams (free for all), none.
pub fn balance(teams: u32, humans: &[(u8, f32)], skills: &[f32]) -> Vec<Option<u8>> {
    if teams < 2 {
        return vec![None; skills.len()];
    }
    let n = teams.min(16) as usize;
    let mut count = vec![0usize; n];
    let mut total = vec![0f32; n];
    for (t, k) in humans {
        let i = usize::from(*t);
        if i < n {
            count[i] += 1;
            total[i] += k;
        }
    }
    skills
        .iter()
        .map(|k| {
            let best = (0..n)
                .min_by(|a, b| {
                    count[*a]
                        .cmp(&count[*b])
                        .then(total[*a].total_cmp(&total[*b]))
                        .then(a.cmp(b))
                })
                .expect("two teams or more");
            count[best] += 1;
            total[best] += k;
            Some(best as u8)
        })
        .collect()
}

// ---- the hook ----

/// The playlist filter on [`HOPPER_LISTED`](guest_layout::points::HOPPER_LISTED).
pub struct PlaylistFilter {
    /// `None` with no Live: every hopper is listed, as retail. Placed
    /// anyway, so the boot plan (and its tests) always holds the hook.
    pub hle: Option<Arc<live_hle::Hle>>,
}

impl hooks::Feature for PlaylistFilter {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "Clankermatch: the OptiMatch playlists split into human and clanker hoppers; \
             Quickmatch keeps to the human ones",
        )
        .doc("docs/game/bots.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use guest_layout::points::{HOPPER_LISTED, PLAYLIST_REMEMBER, QUICKMATCH_HOPPER};
        let hle = self.hle.clone();
        let told = Arc::new(AtomicBool::new(false));
        b.raw()
            .before(&HOPPER_LISTED, "clanker or human hoppers", move |c| {
                let Some(hle) = &hle else {
                    return Ok(Flow::Continue);
                };
                let index = c.cpu.ecx;
                let marks = hle.clanker_hoppers();
                let scope = scope();
                if scope == Scope::Clankers
                    && marks.hoppers.is_empty()
                    && !told.swap(true, Ordering::Relaxed)
                {
                    eprintln!(
                        "clankermatch: the server named no clanker hoppers: the list is empty"
                    );
                }
                if listed(index, &marks, scope) {
                    return Ok(Flow::Continue);
                }
                c.cpu.eax &= !0xff;
                c.ret(0).map(Flow::Return)
            });
        // Quickmatch never searches a clanker hopper: the playlist list's
        // press does not save one ...
        let hle = self.hle.clone();
        b.raw().before(
            &PLAYLIST_REMEMBER,
            "no clanker hopper for Quickmatch",
            move |c| {
                let Some(hle) = &hle else {
                    return Ok(Flow::Continue);
                };
                let Some(hopper) = c.arg_u32("hopper") else {
                    return Ok(Flow::Continue);
                };
                if remembered(hopper, &hle.clanker_hoppers(), scope()) {
                    return Ok(Flow::Continue);
                }
                eprintln!("clankermatch: hopper {hopper} not saved for Quickmatch");
                c.ret(8).map(Flow::Return)
            },
        );
        // ... and one saved anyway reads as none.
        let hle = self.hle.clone();
        b.raw().after(
            &QUICKMATCH_HOPPER,
            "Quickmatch skips a clanker hopper",
            move |c| {
                let Some(hle) = &hle else {
                    return Ok(());
                };
                let saved = c.cpu.eax;
                let hopper = quickmatch_hopper(saved, &hle.clanker_hoppers());
                if hopper != saved {
                    eprintln!(
                        "clankermatch: Quickmatch's saved playlist {saved} is a clanker hopper: none"
                    );
                    c.cpu.eax = hopper;
                }
                Ok(())
            },
        );
        // Its state is the menu's choice and the server's marks, both read
        // afresh at each list build; nothing a snapshot would miss.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the playlist filter (with no Live it lists every hopper) and the
/// lobby's "Switch To: Clankermatch" row (with no Live, never added).
pub fn install(
    registry: &mut hooks::Registry,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    registry
        .add(&PlaylistFilter { hle: live.clone() })
        .map_err(|e| anyhow::anyhow!("clankermatch playlists: {e}"))?;
    registry
        .add(&SwitchRow { hle: live })
        .map_err(|e| anyhow::anyhow!("clankermatch switch row: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hopper::clankers::ClankerHopper;

    fn marks(ids: &[u32]) -> ClankerHoppers {
        ClankerHoppers {
            hoppers: ids
                .iter()
                .map(|&hopper_id| ClankerHopper {
                    hopper_id,
                    playlist_id: hopper_id + 1,
                    players: 8,
                    teams: 0,
                    ranked: true,
                    ranks_as: None,
                })
                .collect(),
        }
    }

    #[test]
    fn each_row_lists_its_own_hoppers() {
        let m = marks(&[8, 9, 10]);
        let human: Vec<u32> = (0..16).filter(|i| listed(*i, &m, Scope::Players)).collect();
        let clanker: Vec<u32> = (0..16).filter(|i| listed(*i, &m, Scope::Clankers)).collect();
        assert_eq!(clanker, vec![8, 9, 10]);
        assert_eq!(human, vec![0, 1, 2, 3, 4, 5, 6, 7, 11, 12, 13, 14, 15]);
        // The party lobby's one page: every hopper, each still told apart.
        assert!((0..16).all(|i| listed(i, &m, Scope::All)));
        assert!(is_clanker(9, &m) && !is_clanker(2, &m));
        // No marks (an older server): OptiMatch as retail, Clankermatch
        // nothing.
        let none = ClankerHoppers::default();
        assert!((0..16).all(|i| listed(i, &none, Scope::Players)));
        assert!((0..16).all(|i| !listed(i, &none, Scope::Clankers)));
        assert!((0..16).all(|i| listed(i, &none, Scope::All)));
    }

    #[test]
    fn quickmatch_never_gets_a_clanker_hopper() {
        let m = marks(&[8, 9, 10]);
        // OptiMatch saves a human hopper; Clankermatch saves nothing, and
        // a clanker hopper is never saved.
        assert!(remembered(2, &m, Scope::Players));
        assert!(!remembered(2, &m, Scope::Clankers));
        assert!(!remembered(9, &m, Scope::Players));
        assert!(!remembered(9, &m, Scope::Clankers));
        // On the one page an ordinary hopper is saved, a clanker one never.
        assert!(remembered(2, &m, Scope::All));
        assert!(!remembered(9, &m, Scope::All));
        // Quickmatch reads a saved clanker hopper as none; a human one, or
        // none, as saved.
        assert_eq!(quickmatch_hopper(9, &m), NO_HOPPER);
        assert_eq!(quickmatch_hopper(2, &m), 2);
        assert_eq!(quickmatch_hopper(NO_HOPPER, &m), NO_HOPPER);
        // No marks (an older server): retail.
        let none = ClankerHoppers::default();
        assert!(remembered(9, &none, Scope::Players));
        assert_eq!(quickmatch_hopper(9, &none), 9);
    }

    #[test]
    fn switch_to_clankermatch_sits_under_switch_to_matchmaking() {
        // The Game Setup lists that have retail's Switch To: Matchmaking
        // (constructor 0x2bb4d2): kind 4 (2, 3, 7, 8), kind 5 (Change Map,
        // Change Rules, Quick Options, Switch To: Co-Op, Switch To:
        // Matchmaking) and kind 6 (9, 6).
        assert!(wants_switch_row(&[2, 3, 7, 8], true, false));
        assert!(wants_switch_row(&[0, 1, 4, 6, 8], true, false));
        assert!(wants_switch_row(&[9, 6], true, false));
        // No matchmaking switch (kinds 0 to 3), no clanker hoppers, or
        // already there: no row.
        assert!(!wants_switch_row(&[0, 1, 4, 6], true, false));
        assert!(!wants_switch_row(&[2, 3, 7], true, false));
        assert!(!wants_switch_row(&[0, 1, 4, 6, 8], false, false));
        assert!(!wants_switch_row(&[0, 1, 4, 6, 8, SWITCH_ACTION], true, false));
        // Already on Clankermatch: no row to switch to it.
        assert!(!wants_switch_row(&[2, 3, 7, 8], true, true));

        assert_eq!(switch_placed(&[2, 3, 7, 8]), [2, 3, 7, 8, SWITCH_ACTION]);
        assert_eq!(switch_placed(&[9, 6]), [9, SWITCH_ACTION, 6]);
        assert_eq!(switch_placed(&[1, 2]), [1, 2, SWITCH_ACTION], "none: last");
        // Beside the Halo 1 Options (10) and Clankers (11) rows, which go
        // above the first Switch To:, in either order of the hooks.
        let custom = [0, 1, 4, 6, 8];
        let theirs_first = switch_placed(&h1row::placed(&h1row::placed(&custom, 10), 11));
        let ours_first = h1row::placed(&h1row::placed(&switch_placed(&custom), 10), 11);
        assert_eq!(theirs_first, [0, 1, 4, 10, 11, 6, 8, SWITCH_ACTION]);
        assert_eq!(ours_first, theirs_first);
    }

    #[test]
    fn the_switch_rows_pick_the_playlists() {
        assert_eq!(switch_press(SWITCH_ACTION), Some(Scope::Clankers));
        assert_eq!(switch_press(8), Some(Scope::Players), "Switch To: Matchmaking");
        assert_eq!(
            switch_press(CHANGE_PLAYLIST),
            Some(Scope::All),
            "a matchmaking lobby's Change Playlist...: one page of both"
        );
        for other in [0, 1, 2, 3, 4, 6, 7, 10, 11] {
            assert_eq!(switch_press(other), None, "{other}");
        }
        // Our action is past every other Game Setup row's.
        const {
            assert!(SWITCH_ACTION > crate::clankers::ROW_ACTION);
            assert!(SWITCH_ACTION > h1row::OPTIONS_ACTION);
        }
        assert_eq!(
            crate::custom_games::SWITCH_CLANKERMATCH_TEXT,
            "Switch To: Clankermatch"
        );
    }

    #[test]
    fn the_switch_row_goes_after_the_other_game_setup_rows_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&h1row::SetupRow {
            data_root: std::env::temp_dir(),
            page: crate::h1_options_row::test_page(),
        })
        .unwrap();
        let store = Arc::new(settings::Store::defaults(&crate::clankers::DEFS));
        let page = Arc::new(crate::clankers::page(store));
        r.add(&crate::ui_kit::screen::KitTexts(Arc::clone(&page)))
            .unwrap();
        r.add(&page.class()).unwrap();
        r.add(&crate::clankers::SetupRow { page }).unwrap();
        r.add(&SwitchRow { hle: None }).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        for point in [
            "SQUAD_SETTINGS_LIST_CTOR",
            "SQUAD_SETTINGS_ROW_TEXT",
            "SQUAD_SETTINGS_ROW_CHOSEN",
        ] {
            let line = text.lines().find(|l| l.contains(point)).unwrap();
            let (h1, clankers, ours) = (
                line.find("h1-options-row:").unwrap(),
                line.find("clankers-row:").unwrap(),
                line.find("clankermatch-switch:").unwrap(),
            );
            assert!(h1 < clankers && clankers < ours, "{line}");
        }
    }

    #[test]
    fn bots_join_half_a_second_to_three_seconds_apart() {
        for seed in [0, 1, 0xc1a4, u64::MAX] {
            let s = schedule(7, seed);
            assert_eq!(s.len(), 7);
            let mut last = Duration::ZERO;
            for t in &s {
                let gap = *t - last;
                assert!((MIN_GAP..=MAX_GAP).contains(&gap), "{seed}: {gap:?}");
                last = *t;
            }
        }
        // Seeded: the same lobby, the same schedule.
        assert_eq!(schedule(5, 42), schedule(5, 42));
        assert_ne!(schedule(5, 42), schedule(5, 43));
        assert!(schedule(0, 1).is_empty());
    }

    #[test]
    fn the_lobby_fills_one_at_a_time() {
        let s = schedule(3, 7);
        assert_eq!(seated(&s, Duration::ZERO), 0);
        assert_eq!(seated(&s, s[0] - Duration::from_millis(1)), 0);
        assert_eq!(seated(&s, s[0]), 1);
        assert_eq!(seated(&s, s[1]), 2);
        assert_eq!(seated(&s, s[2] + Duration::from_secs(60)), 3);
    }

    #[test]
    fn teams_balance_by_count_then_skill() {
        // Free for all: no teams.
        assert_eq!(balance(0, &[(0, 0.5)], &[0.5, 0.5]), vec![None, None]);
        // Two teams of four, the human on red: three bots join red, four
        // blue.
        let t = balance(2, &[(0, 0.5)], &[0.5; 7]);
        assert_eq!(t.iter().filter(|t| **t == Some(0)).count(), 3);
        assert_eq!(t.iter().filter(|t| **t == Some(1)).count(), 4);
        // The human on blue: the other way round.
        let t = balance(2, &[(1, 0.5)], &[0.5; 7]);
        assert_eq!(t.iter().filter(|t| **t == Some(0)).count(), 4);
        // Equal counts: the weaker side gets the next bot. A human of 1.0
        // on red, then bots of 0.9 and 0.1: blue takes the 0.9 (fewest
        // players); then red (1.0) and blue (0.9) have one each, and the
        // 0.1 goes to blue, the lower total.
        assert_eq!(balance(2, &[(0, 1.0)], &[0.9, 0.1]), vec![Some(1), Some(1)]);
        // A human on a team the game does not have counts for none.
        assert_eq!(balance(2, &[(5, 1.0)], &[0.5, 0.5]), vec![Some(0), Some(1)]);
    }
}
