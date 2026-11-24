//! The lobby's modes on Game Setup (docs/game/forge-firefight-ui.md, "The
//! lobbies"): Multiplayer, Forge and Firefight as modes of the retail
//! pregame lobby, chosen in the party lobby's GAME MODE picker
//! (`crate::lobby_tabs`, docs/game/party-lobby.md).
//!
//! **No mode row.** X's GAME MODE picker is the one way to change mode in
//! the lobby (`crate::lobby_tabs::input`, on `PREGAME_LOBBY_INPUT`, which
//! this feature places); Game Setup has no row that does. Retail's "Switch To"
//! rows (co-op 6, custom 7, matchmaking 8, and 9 in a custom list) and
//! Clankermatch's (12) are dropped from every Game Setup list
//! ([`SWITCH_ROWS`]).
//!
//! **Rows by mode** ([`arrange`]): an `after` on the list's constructor,
//! after every other row of ours is added, rewrites the list
//! ([`crate::h1_options_row::rewrite_rows`]): in Forge,
//! **Forge Map...** ([`FORGE_MAP_ACTION`]) after Change Map, the game type
//! row and Clanker Options kept, Firefight Options hidden; in Firefight,
//! Firefight Options after Change Map and the game type row hidden. In a
//! matchmaking lobby (MULTIPLAYER) action 9 stays, drawn as
//! **Change Playlist...** ([`CHANGE_PLAYLIST_LABEL`]): its press opens the
//! playlist screen with the Clanker and the human playlists on one page
//! (`crate::clankermatch::Scope::All`).
//!
//! **Forge Map...** opens the picker of New and the maps saved under
//! `<data>/forge` ([`forge_map_pick`]); the Forge session loads the choice
//! as it opens ([`crate::forge_glue`]).
//!
//! **The panel** ([`panel_ids`]): the lobby's variant panel update
//! (`crate::lobby_options`) adds the mode's lines. The lobby's header
//! names the mode, set by this feature's own `before` on the update,
//! inside `lobby-options`'s (`crate::lobby_tabs::header::panel`), so it
//! does not depend on that feature.
//!
//! **Start Game with a remote member.** Forge across consoles needs the
//! session log on the network (slice F5). Until then, an `after` on the
//! lobby's start state turns a ready Forge lobby with a member on another
//! console ([`remote_members`]) away from ready, so Start Game does nothing
//! (it acts only on ready, `0x251050`), and the status line says
//! [`REMOTE_REFUSAL`] (`crate::lobby_status`).
//!
//! **Start Game with no Firefight map.** The same `after` holds a
//! Firefight lobby whose map does not carry the kit ([`start_refusal`]):
//! the lobby switches to FIREFIGHT with no such map installed, and the status line
//! and the panel ([`firefight_map_lines`]) say why it cannot start.
//!
//! **Start Game on a blocked pair.** The same `after` holds the start
//! while the lobby's start gate (`crate::rules::gate`) finds the map lacks
//! what the variant's script needs, outside Forge mode; the status line
//! names the first problem ([`gate_text_id`]). The party lobby's `before`
//! on the panel update refreshes the gate's verdict: a changed map, Forge
//! map or variant runs the check once (docs/game/rules-start-gate.md).
//!
//! **Leaving.** The party's leave (`PARTY_LEAVE`) clears the mode
//! ([`crate::lobby_start::clear`]), as does building the main menu or the
//! Xbox Live menu again (`crate::layout_glue`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use settings::Store;

use crate::firefight::options as ff;
use crate::h1_options_row::{self as h1row, CHANGE_MAP};
use crate::lobby_start::{self, KitMaps, Mode, Offer};
use crate::lobby_tabs::Reason;
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::screen::SlotTexts;
use crate::ui_kit::view::Opener;

/// The party lobby's feature: its modes, the header that names them and
/// X's GAME MODE picker. It has no switch ([`hooks::Manifest::always_on`]):
/// it is how the lobby works (docs/game/party-lobby.md).
pub const FEATURE: &str = "party-lobby";
/// The Forge Map row's action: past Firefight Options' 13 (retail uses 0
/// to 9; the row-chosen jump table has no bounds check, so the press never
/// reaches the original).
pub const FORGE_MAP_ACTION: i16 = 15;
/// Retail's game type row, "Change Rules...".
pub const CHANGE_RULES: i16 = 1;

/// The rows' texts, the help lines and the panel lines.
pub const IDS: Family = ids::family("lobby mode");
const FORGE_MAP_LABEL_SLOT: u32 = 1;
const CHANGE_PLAYLIST_SLOT: u32 = 2;
const FORGE_MAP_HELP_SLOT: u32 = 3;
const PANEL_SLOTS: u32 = 5;
/// The most panel lines a mode has (Firefight's three).
pub const MAX_PANEL_LINES: usize = 3;
/// The status line's text while the start gate holds Start Game, after
/// the panel lines' slots.
const GATE_SLOT: u32 = PANEL_SLOTS + MAX_PANEL_LINES as u32 + 1;
/// The status line's text while the lobby's map variant builds or is
/// refused ([`variant_text_id`]).
const VARIANT_SLOT: u32 = GATE_SLOT + 1;

pub const FORGE_MAP_LABEL: &str = "Forge Map...";
/// A matchmaking lobby's action 9 ([`crate::clankermatch::CHANGE_PLAYLIST`]).
pub const CHANGE_PLAYLIST_LABEL: &str = "Change Playlist...";
/// The rows that change the lobby's mode, which the GAME MODE picker does
/// instead:
/// retail's "Switch To: Co-Op Game" (6), "Switch To: Custom Game" (7) and
/// "Switch To: Matchmaking" (8), and Clankermatch's "Switch To:
/// Clankermatch". Action 9 is dropped only from a custom game's list,
/// where it is "Switch To: Matchmaking" too.
pub const SWITCH_ROWS: [i16; 4] = [6, 7, 8, crate::clankermatch::SWITCH_ACTION];
/// The Forge Map picker's first choice: no saved map.
pub const NEW_MAP: &str = "New";
/// Why Start Game does nothing in a Forge lobby with a member on another
/// console, on the lobby's status line.
pub const REMOTE_REFUSAL: &str = "Forge with players on other consoles is not available.";
/// The start state a refused Forge lobby answers in place of ready (9):
/// any other makes Start Game do nothing; the status line's text is ours.
pub const REFUSED_STATE: u32 = 7;
const READY_STATE: u32 = 9;

static TEXTS: LazyLock<SlotTexts> = LazyLock::new(|| SlotTexts::new(IDS));

// ---- the rows ----

/// Whether Game Setup shows `action` in `mode`: Forge hides Firefight
/// Options; Firefight hides the game type row. Clanker Options shows in
/// every mode of the custom lobby.
pub fn shown(mode: Mode, action: i16) -> bool {
    match mode {
        Mode::Multiplayer => true,
        Mode::Forge => action != ff::ROW_ACTION,
        Mode::Firefight => action != CHANGE_RULES,
    }
}

/// Whether a Game Setup row changes the lobby's mode, so the GAME MODE
/// picker stands in for it: [`SWITCH_ROWS`], and action 9 in a custom game's list
/// (`custom`, it has Change Map).
pub fn switches_mode(action: i16, custom: bool) -> bool {
    SWITCH_ROWS.contains(&action) || (custom && action == crate::clankermatch::CHANGE_PLAYLIST)
}

/// The rows of a Game Setup list with `actions` in `mode`, or `None` to
/// leave it as built (nothing to change). No row changes the mode: that is
/// the GAME MODE picker's, so the Switch To rows go from every list. The mode's rows
/// are a custom game's list's (it has Change Map), with a mode besides
/// Multiplayer on `offer`.
pub fn arrange(mode: Mode, offer: Offer, actions: &[i16]) -> Option<Vec<i16>> {
    let custom = actions.contains(&CHANGE_MAP);
    let mode = if custom && offer.any() {
        mode
    } else {
        Mode::Multiplayer
    };
    let firefight_row = mode == Mode::Firefight && actions.contains(&ff::ROW_ACTION);
    let mut out = Vec::new();
    for &a in actions
        .iter()
        .filter(|a| **a != FORGE_MAP_ACTION && !switches_mode(**a, custom) && shown(mode, **a))
    {
        if firefight_row && a == ff::ROW_ACTION {
            continue;
        }
        out.push(a);
        if a == CHANGE_MAP {
            match mode {
                Mode::Forge => out.push(FORGE_MAP_ACTION),
                Mode::Firefight if firefight_row => out.push(ff::ROW_ACTION),
                _ => {}
            }
        }
    }
    (out != actions).then_some(out)
}

/// The Forge Map row's help line: the map's description.
pub fn forge_map_help(name: Option<&str>) -> String {
    match name {
        None => "New starts from the map as it is.".into(),
        Some(name) => format!("Loads the saved map {name} and its edits."),
    }
}

/// The Forge Map row's pick over `saved` (`forge::saved_names`): New, then
/// each saved map; the lobby's held. A list that cannot be read is named in
/// the description, and New still starts a fresh map.
pub fn forge_map_pick(saved: Result<Vec<String>, String>, current: Option<&str>) -> Pick {
    let (names, description) = match saved {
        Ok(names) => (
            names,
            "New starts from the map as it is. A saved map loads its edits.".to_owned(),
        ),
        Err(why) => (
            Vec::new(),
            format!("The saved maps cannot be listed: {why}. New still starts a fresh map."),
        ),
    };
    let mut values = vec![NEW_MAP.to_owned()];
    values.extend(names);
    let held = current
        .and_then(|c| values.iter().skip(1).position(|v| v == c).map(|i| i + 1))
        .unwrap_or(0);
    let chosen = values.clone();
    Pick::new(
        "Forge Map",
        values,
        held,
        &description,
        Arc::new(move |i: usize| match chosen.get(i) {
            Some(name) if i > 0 => {
                lobby_start::set_forge_map(Some(name.clone()));
                format!("lobby mode: Forge map {name}")
            }
            Some(_) => {
                lobby_start::set_forge_map(None);
                "lobby mode: a new Forge map".into()
            }
            None => format!("Forge map {i} is not in the list"),
        }),
    )
    .over(Opener::GAME_SETUP_FORGE)
}

// ---- the panel ----

/// The variant panel's lines for the lobby's `mode`: the Forge map, or
/// Firefight's wave set with its player teams when there are two, the
/// difficulty and the starting lives (`firefight`, the options page's
/// choices, or why they cannot be read).
pub fn panel_lines(
    mode: Mode,
    forge_map: Option<&str>,
    firefight: Option<Result<ff::Options, String>>,
) -> Vec<String> {
    match mode {
        Mode::Multiplayer => Vec::new(),
        Mode::Forge => vec![match forge_map {
            None => "New Forge map".into(),
            Some(name) => format!("Forge map {name}"),
        }],
        Mode::Firefight => match firefight {
            None => vec!["No settings store holds Firefight's options".into()],
            Some(Err(why)) => vec![format!("Firefight options unread ({why})")],
            Some(Ok(o)) => {
                let wave = choice_label(&ff::WAVE_SET, o.wave_set as i64);
                let waves = match o.player_teams() {
                    1 => format!("{wave} waves"),
                    n => format!("{wave} waves, {n} teams"),
                };
                let difficulty = choice_label(&ff::DIFFICULTY, difficulty_value(o.difficulty));
                let lives = match o.lives {
                    None => "The wave set's own lives".to_owned(),
                    Some(1) => "1 starting life".to_owned(),
                    Some(n) => format!("{n} starting lives"),
                };
                vec![waves, format!("{difficulty} difficulty"), lives]
            }
        },
    }
}

/// The variant panel's lines in place of Firefight's options when the
/// lobby's map cannot play Firefight (`kit`): no installed map carries the
/// kit, or this one does not. `None` on a map that carries it.
pub fn firefight_map_lines(kit: KitMaps) -> Option<Vec<String>> {
    match (kit.lobby, kit.installed) {
        (true, _) => None,
        (false, false) => Some(vec![
            "No installed map supports Firefight yet".into(),
            "Make one with h2 firefight kit".into(),
        ]),
        (false, true) => Some(vec![
            "This map does not support Firefight".into(),
            "Change Map lists the Firefight maps".into(),
        ]),
    }
}

/// `difficulty`'s value in the setting ([`ff::Options::read`]'s order).
fn difficulty_value(difficulty: ::firefight::Difficulty) -> i64 {
    use ::firefight::Difficulty as D;
    match difficulty {
        D::Easy => 0,
        D::Normal => 1,
        D::Heroic => 2,
        D::Legendary => 3,
    }
}

fn choice_label(def: &settings::Def, value: i64) -> String {
    match def.kind {
        settings::Kind::Choice(choices) => choices
            .iter()
            .find(|c| c.value == value)
            .map_or_else(|| format!("{} {value}", def.label), |c| c.label.to_owned()),
        _ => format!("{} {value}", def.label),
    }
}

/// The panel lines' string ids for the lobby now, their texts written to
/// guest memory first; none in Multiplayer. A Firefight lobby on a map
/// without the kit says so in place of the options
/// ([`firefight_map_lines`]).
pub fn panel_ids(m: &mut dyn GuestMemory, store: &Store) -> Result<Vec<u32>, MemFault> {
    let state = lobby_start::state();
    let no_map = (state.mode == Mode::Firefight)
        .then(|| firefight_map_lines(KitMaps::now(&*m)))
        .flatten();
    let lines = match no_map {
        Some(lines) => lines,
        None => {
            let firefight = (state.mode == Mode::Firefight).then(|| ff::Options::read(store));
            panel_lines(state.mode, state.forge_map.as_deref(), firefight)
        }
    };
    let ids = lines
        .iter()
        .take(MAX_PANEL_LINES)
        .enumerate()
        .map(|(i, line)| TEXTS.set(PANEL_SLOTS + i as u32, line))
        .collect();
    TEXTS.write(m)?;
    Ok(ids)
}

// ---- Start Game with a remote member ----

/// How many peers of the lobby's session are on another console and not a
/// Clanker's phantom: 0 in a local lobby (Split Screen), `None` while the
/// session cannot be read.
pub fn remote_members(m: &dyn GuestMemory) -> Option<u32> {
    use bots::lobby;
    use bots::phantom::session::{MAX, at};
    if m.u8(lobby::NETWORKING).ok()? == 0 {
        return Some(0);
    }
    let s = m.u32(lobby::SESSION).ok().filter(|&s| s != 0)?;
    let count = m.u32(s + at::PEER_COUNT).ok()?.min(MAX);
    let local = m.u32(s + at::LOCAL_PEER).ok()?;
    let mut n = 0;
    for p in (0..count).filter(|&p| p != local) {
        let mut mac = [0u8; 6];
        m.read(s + at::PEERS + p * at::PEER_SIZE + at::PEER_MAC, &mut mac)
            .ok()?;
        if bots::phantom::handle_of(mac).is_none() {
            n += 1;
        }
    }
    Some(n)
}

/// Why the lobby's Start Game is refused, besides the start gate: a Forge
/// lobby with a member on another console (one that cannot be read counts
/// as one), or a Firefight lobby on a map without the kit (`kit`), named
/// for whether any installed map has it.
pub fn refusal(mode: Mode, remote: Option<u32>, kit: KitMaps) -> Option<Reason> {
    match mode {
        Mode::Forge if remote != Some(0) => Some(Reason::ForgeRemote),
        Mode::Firefight if !kit.lobby && kit.installed => Some(Reason::NotKitMap),
        Mode::Firefight if !kit.lobby => Some(Reason::NoKitMap),
        Mode::Multiplayer | Mode::Forge | Mode::Firefight => None,
    }
}

/// The problems the lobby's start gate holds Start Game on
/// (`crate::rules::gate::blocked`); none in Forge mode, and none without
/// the rules runtime compiled in.
pub fn gate_blocked() -> Option<Vec<String>> {
    #[cfg(feature = "rules")]
    {
        crate::rules::gate::blocked()
    }
    #[cfg(not(feature = "rules"))]
    {
        None
    }
}

/// Why the lobby now refuses Start Game besides the start gate
/// ([`refusal`]); `None` outside a lobby (the level boot's game).
pub fn start_refusal(m: &dyn GuestMemory) -> Option<Reason> {
    let state = lobby_start::state();
    if !state.lobby {
        return None;
    }
    match state.mode {
        Mode::Multiplayer => None,
        Mode::Forge => refusal(state.mode, remote_members(m), KitMaps::default()),
        Mode::Firefight => refusal(state.mode, Some(0), KitMaps::now(m)),
    }
}

/// Whether the lobby now refuses Start Game: [`start_refusal`], or the
/// start gate holds it ([`gate_blocked`]).
pub fn start_refused(m: &dyn GuestMemory) -> bool {
    let state = lobby_start::state();
    state.lobby
        && (gate_blocked().is_some() || start_refusal(m).is_some() || variant_hold(m).is_some())
}

/// What holds Start Game on the lobby's map variant: it builds, or a check
/// of the table failed ([`crate::map_variant::hold`]). A pick made on
/// another map is cleared first.
pub fn variant_hold(m: &dyn GuestMemory) -> Option<String> {
    crate::map_variant::sync(m.u32(crate::h1_options_row::LOBBY_MAP).ok());
    crate::map_variant::hold()
}

/// The status line's text while the map variant holds Start Game, written
/// to its slot. `None` while it does not.
pub fn variant_text_id(m: &mut dyn GuestMemory) -> Option<u32> {
    let text = variant_hold(&*m)?;
    let id = TEXTS.set(VARIANT_SLOT, &text);
    TEXTS.write(m).ok()?;
    Some(id)
}

/// The status line's text while the start gate holds Start Game: the
/// first problem, with ` (and n more)` ([`gate_text`]), written to its
/// slot. `None` while the gate does not hold.
pub fn gate_text_id(m: &mut dyn GuestMemory) -> Option<u32> {
    let text = gate_text(&gate_blocked()?)?;
    let id = TEXTS.set(GATE_SLOT, &text);
    TEXTS.write(m).ok()?;
    Some(id)
}

/// The status line's text for `problems`: the first, with ` (and n more)`.
pub fn gate_text(problems: &[String]) -> Option<String> {
    let first = problems.first()?;
    Some(match problems.len() {
        1 => first.clone(),
        n => format!("{first} (and {} more)", n - 1),
    })
}

/// The start state the lobby answers: `retail`'s, unless it is ready and
/// Start Game is refused.
pub fn start_state(retail: u32, refused: bool) -> u32 {
    if retail == READY_STATE && refused {
        REFUSED_STATE
    } else {
        retail
    }
}

// ---- the hooks ----

/// A Game Setup hook of ours sits after every other row's on the plan, so
/// the mode's arrangement sees the rows they add. That holds for `before`
/// hooks; an `after` sitting inside another feature's runs *before* it
/// (`after` hooks run in reverse, docs/engine/feature-engine.md, "Hook
/// ordering"), so the list's arrangement is [`hooks::HookRef::first`]
/// instead: the outermost `after`, which runs last.
fn after_rows(h: hooks::HookRef<'_>) {
    h.after_feature(crate::clankers::ROW_FEATURE)
        .after_feature(crate::clankermatch::SWITCH_FEATURE)
        .after_feature(h1row::ROW_FEATURE)
        .after_feature(ff::ROW_FEATURE);
}

/// The Forge Map row, the rows by mode, the header, X's GAME MODE picker,
/// the start refusal and the leave.
pub struct LobbyMode {
    pub offer: Offer,
    /// The settings store Firefight's choices live in.
    pub store: Option<Arc<Store>>,
    /// The data directory, whose `forge` folder holds the saved maps.
    pub data: PathBuf,
    /// The mounted VFS, for the start gate's census of the lobby's map file.
    pub vfs: Option<Arc<Mutex<vfs::Vfs>>>,
}

fn row_text(c: &mut Call, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let (item, id) = {
        let mut mem = rt.memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + 0x70).ok())
            .and_then(|row| h1row::row_action(&*mem, c.cpu.ecx, row));
        let custom = h1row::setup_actions(&*mem, c.cpu.ecx)
            .is_ok_and(|actions| actions.contains(&CHANGE_MAP));
        let slot = match action {
            Some(FORGE_MAP_ACTION) => FORGE_MAP_LABEL_SLOT,
            Some(crate::clankermatch::CHANGE_PLAYLIST) if !custom => CHANGE_PLAYLIST_SLOT,
            _ => return Ok(Flow::Continue),
        };
        let Some(item) = item else {
            return Ok(Flow::Continue);
        };
        if let Err(e) = TEXTS.write(&mut *mem) {
            eprintln!("lobby mode: the row's text: {e:?}");
        }
        (item, IDS.id(slot))
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("lobby mode: Game Setup row text");
    }
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            crate::list_class::draw_texts(&cx, item, vec![(0, id)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, me: &LobbyMode, request_at: u32) -> Result<Flow, GuestFault> {
    let (action, player, stack) = {
        let mem = c.ctx.runtime().memory();
        let row = mem.u32(c.cpu.esp + 8).ok().and_then(|p| mem.u32(p).ok());
        let action = row.and_then(|row| h1row::row_action(&*mem, c.cpu.ecx, row));
        let player = crate::skulls_menu::player_index(&*mem, c.cpu.esp);
        let screen = h1row::screen_of(&*mem, c.cpu.ecx);
        let stack = screen.and_then(|s| {
            Some((
                mem.u32(s + crate::ui_kit::lifecycle::SCREEN_WINDOW).ok()?,
                mem.u32(s + crate::ui_kit::lifecycle::SCREEN_SLOT).ok()?,
            ))
        });
        (action, player, stack)
    };
    if action != Some(FORGE_MAP_ACTION) {
        return Ok(Flow::Continue);
    }
    let pick = forge_map_pick(
        ::forge::saved_names(&me.data),
        lobby_start::forge_map().as_deref(),
    );
    let what = "Forge Map";
    let loader = picker::offer_list(pick).and_then(list_class_loader);
    let (Some(player), Some((window, slot)), Some(loader)) = (player, stack, loader) else {
        eprintln!(
            "lobby mode: the {what} picker not opened (player {player:?}, stack {stack:?}, picker {})",
            if list_class_loader(picker::NAME).is_some() {
                "on"
            } else {
                "off"
            }
        );
        return c.ret(8).map(Flow::Return);
    };
    eprintln!(
        "lobby mode: opening the {what} picker (player {player}) on window {window}, slot {slot}"
    );
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn list_class_loader(name: &'static str) -> Option<u32> {
    crate::list_class::loader_of(name)
}

fn save_state() -> Vec<u8> {
    let s = lobby_start::state();
    let mode = Mode::ALL.iter().position(|&m| m == s.mode).unwrap_or(0) as u8;
    let mut out = vec![u8::from(s.lobby), mode];
    if let Some(name) = s.forge_map {
        out.extend(name.into_bytes());
    }
    out
}

fn load_state(bytes: &[u8]) -> Result<(), String> {
    let [lobby, mode, name @ ..] = bytes else {
        return Err(format!("{} bytes, not the lobby's mode", bytes.len()));
    };
    let mode = *Mode::ALL
        .get(usize::from(*mode))
        .ok_or_else(|| format!("mode {mode} is not a lobby mode"))?;
    let name = if name.is_empty() {
        None
    } else {
        Some(String::from_utf8(name.to_vec()).map_err(|e| format!("the Forge map's name: {e}"))?)
    };
    lobby_start::restore(lobby_start::State {
        lobby: *lobby != 0,
        mode,
        forge_map: name,
        map_variant: None,
    });
    crate::map_variant::forget();
    TEXTS.rewrite_all();
    // The start gate checks the restored lobby again at its next refresh.
    #[cfg(feature = "rules")]
    crate::rules::gate::clear();
    Ok(())
}

impl hooks::Feature for LobbyMode {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "The party lobby: its modes, the header that names them and X's GAME MODE picker",
        )
        .doc("docs/game/party-lobby.md")
        .always_on()
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::{
            SQUAD_SETTINGS_LIST_CTOR as CTOR, SQUAD_SETTINGS_ROW_CHOSEN, SQUAD_SETTINGS_ROW_TEXT,
        };
        // The texts: a slot each, served to TEXT_SET by our locale source.
        let len = IDS.len * crate::ui_kit::screen::SLOT_LEN;
        let base = b.guest_alloc(len.next_multiple_of(0x1000), Permissions::RW)?;
        TEXTS.base.store(base, Ordering::SeqCst);
        TEXTS.set(FORGE_MAP_LABEL_SLOT, FORGE_MAP_LABEL);
        TEXTS.set(CHANGE_PLAYLIST_SLOT, CHANGE_PLAYLIST_LABEL);
        b.locale().source("lobby mode", |id| TEXTS.address(id));

        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        // The lobby's header and prompt line (`crate::lobby_tabs::header`),
        // and the GAME MODE picker's open request (`crate::lobby_tabs::input`).
        crate::lobby_tabs::header::install();
        let picker_request = b.guest_alloc(0x1000, Permissions::RW)?;
        crate::lobby_tabs::input::install(picker_request);
        // The mode switch's out-parameters (`crate::lobby_tabs::switch`).
        let scratch = b.guest_alloc(0x1000, Permissions::RW)?;
        crate::lobby_tabs::switch::install(scratch);
        let offer = self.offer;
        let building: Arc<Mutex<Vec<u32>>> = Arc::default();
        let (noted, live) = (Arc::clone(&building), Arc::clone(&building));
        after_rows(b.raw().before(&CTOR, "note the Game Setup list", move |c| {
            noted
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(c.cpu.esi);
            Ok(Flow::Continue)
        }));
        // Outermost, so it runs after every other feature's `after` on the
        // constructor has added its row (Clanker Options, Firefight
        // Options, Halo 1 Options, Switch To: Clankermatch): the mode's
        // arrangement sees the whole list and drops the Switch To rows.
        b.raw()
            .after(&CTOR, "the rows of the lobby's mode", move |c| {
                let Some(list) = building.lock().unwrap_or_else(|e| e.into_inner()).pop()
                else {
                    return Ok(());
                };
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let actions = h1row::setup_actions(&*mem, list).unwrap_or_default();
                let mode = lobby_start::mode();
                let Some(rows) = arrange(mode, offer, &actions) else {
                    return Ok(());
                };
                match h1row::rewrite_rows(&mut *mem, list, &rows) {
                    Ok(n) => {
                        eprintln!("lobby mode: Game Setup list in {} ({n} rows)", mode.label())
                    }
                    Err(why) => {
                        eprintln!("lobby mode: Game Setup list: rows not arranged: {why}")
                    }
                }
                Ok(())
            })
            .first();
        let seen = Arc::new(AtomicBool::new(false));
        after_rows(b.raw().before(
            &SQUAD_SETTINGS_ROW_TEXT,
            "the lobby mode rows' text",
            move |c| row_text(c, &seen),
        ));
        let me = Arc::new(LobbyMode {
            offer: self.offer,
            store: self.store.clone(),
            data: self.data.clone(),
            vfs: self.vfs.clone(),
        });
        after_rows(b.raw().before(
            &SQUAD_SETTINGS_ROW_CHOSEN,
            "the lobby mode rows' press",
            move |c| row_chosen(c, &me, request_at),
        ));

        // Start Game in a Forge lobby with a member on another console, or
        // on a pair the start gate blocks.
        b.raw()
            .after(
                &points::LOBBY_START_STATE,
                "refuse Forge across consoles and blocked pairs",
                |c| {
                    let refused = start_refused(&*c.ctx.runtime().memory());
                    let out = start_state(c.cpu.eax, refused);
                    if out != c.cpu.eax {
                        c.cpu.eax = out;
                    }
                    Ok(())
                },
            )
            .optional();
        // Leaving the lobby clears its mode, the reason line and the
        // playlists a switch found missing.
        b.raw()
            .before(
                &points::PARTY_LEAVE,
                "the lobby's mode ends with it",
                |_| {
                    lobby_start::clear();
                    crate::lobby_tabs::clear_notice();
                    crate::lobby_tabs::switch::forget_playlists();
                    #[cfg(feature = "rules")]
                    crate::rules::gate::clear();
                    Ok(Flow::Continue)
                },
            )
            // The bots' own leave runs first; clearing the mode touches none of their state.
            .after_feature("bots")
            .optional();
        // The start gate's sources: the lobby's map file through the VFS,
        // and the saved Forge maps under the data directory.
        #[cfg(feature = "rules")]
        crate::rules::gate::install(Box::new(crate::rules::gate::InGame {
            vfs: self.vfs.clone(),
            data: self.data.clone(),
        }));
        // The start gate's verdict, then the header and prompt line after
        // retail's panel update, inside `lobby-options`'s answer when that
        // feature is on and alone when it is off (docs/game/party-lobby.md,
        // "The header and the prompt"). The gate reads the lobby with guest
        // memory held and checks without it, since its census reads the
        // map file (docs/game/rules-start-gate.md, "Where it runs").
        let store = self.store.clone();
        b.raw()
            .before(&points::LOBBY_VARIANT_PANEL, "the lobby's header and the start gate", move |c| {
                #[cfg(feature = "rules")]
                {
                    let variant = c.arg_u32("variant");
                    let input = variant.and_then(|v| {
                        let rt = Arc::clone(c.ctx.runtime());
                        let mem = rt.memory();
                        crate::rules::gate::read(&*mem, v)
                    });
                    crate::rules::gate::refresh(input);
                }
                crate::lobby_tabs::header::panel(c, store.clone())
            })
            .after_feature(crate::lobby_options::FEATURE);
        // X opens the GAME MODE picker (docs/game/party-lobby.md,
        // "Changing mode").
        let store = self.store.clone();
        b.raw().before(
            &points::PREGAME_LOBBY_INPUT,
            "X opens the GAME MODE picker",
            move |c| crate::lobby_tabs::input::press(c, store.clone()),
        );

        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || h1row::building_live(&live).or_else(crate::lobby_tabs::switch::in_flight),
            save_state,
            |bytes, _| load_state(bytes),
        ));
        Ok(())
    }
}

/// Adds the rows and registers their help lines.
pub fn install(registry: &mut hooks::Registry, feature: LobbyMode) -> anyhow::Result<()> {
    crate::game_setup_help::register(
        FORGE_MAP_ACTION,
        crate::game_setup_help::image::QUICK_OPTIONS,
        Arc::new(|m: &mut dyn GuestMemory| {
            let id = TEXTS.set(
                FORGE_MAP_HELP_SLOT,
                &forge_map_help(lobby_start::forge_map().as_deref()),
            );
            TEXTS.write(m).ok()?;
            Some(id)
        }),
    );
    registry
        .add(&feature)
        .map_err(|e| anyhow::anyhow!("{FEATURE}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests;
