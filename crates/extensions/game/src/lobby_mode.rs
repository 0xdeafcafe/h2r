//! The lobby's modes on Game Setup (docs/game/forge-firefight-ui.md, "The
//! lobbies"): Multiplayer, Forge and Firefight as modes of the retail
//! pregame lobby, chosen on the party lobby's mode strip
//! (`crate::lobby_tabs`, docs/game/party-lobby.md).
//!
//! **No mode row.** The strip's bumpers are the one way to change mode
//! (`crate::lobby_tabs::input`, on `PREGAME_LOBBY_INPUT`, which this
//! feature places); Game Setup has no row that does.
//!
//! **Rows by mode** ([`arrange`]): an `after` on the list's constructor,
//! after every other row of ours is added, rewrites the list
//! ([`crate::h1_options_row::rewrite_rows`]): in Forge,
//! **Forge Map...** ([`FORGE_MAP_ACTION`]) after Change Map, the game type
//! row kept, Clanker Options and Firefight Options hidden; in Firefight,
//! Firefight Options after Change Map and the game type row hidden.
//!
//! **Forge Map...** opens the picker of New and the maps saved under
//! `<data>/forge` ([`forge_map_pick`]); the Forge session loads the choice
//! as it opens ([`crate::forge_glue`]).
//!
//! **The panel** ([`panel_ids`]): the lobby's variant panel update
//! (`crate::lobby_options`) adds the mode's lines. The lobby's title is the
//! mode strip's selected tab (`crate::lobby_tabs::strip`), which hides the
//! retail header.
//!
//! **Start Game with a remote member.** Forge across consoles needs the
//! session log on the network (slice F5). Until then, an `after` on the
//! lobby's start state turns a ready Forge lobby with a member on another
//! console ([`remote_members`]) away from ready, so Start Game does nothing
//! (it acts only on ready, `0x251050`), and the status line says
//! [`REMOTE_REFUSAL`] (`crate::lobby_status`).
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
use crate::lobby_start::{self, Mode, Offer};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::screen::SlotTexts;
use crate::ui_kit::view::Opener;

/// The party lobby's feature: the mode strip and the modes of ours. It has
/// no switch ([`hooks::Manifest::always_on`]): the strip is how the lobby
/// works (docs/game/party-lobby.md).
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
const FORGE_MAP_HELP_SLOT: u32 = 3;
const PANEL_SLOTS: u32 = 5;
/// The most panel lines a mode has (Firefight's three).
pub const MAX_PANEL_LINES: usize = 3;

pub const FORGE_MAP_LABEL: &str = "Forge Map...";
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

/// Whether Game Setup shows `action` in `mode`: Forge hides Clanker
/// Options and Firefight Options; Firefight hides the game type row.
pub fn shown(mode: Mode, action: i16) -> bool {
    match mode {
        Mode::Multiplayer => true,
        Mode::Forge => action != crate::clankers::ROW_ACTION && action != ff::ROW_ACTION,
        Mode::Firefight => action != CHANGE_RULES,
    }
}

/// The rows of a Game Setup list with `actions` in `mode`, or `None` to
/// leave it as built: not a custom game's list (no Change Map), or no mode
/// besides Multiplayer on offer. No row changes the mode: that is the
/// strip's.
pub fn arrange(mode: Mode, offer: Offer, actions: &[i16]) -> Option<Vec<i16>> {
    if !offer.any() || !actions.contains(&CHANGE_MAP) {
        return None;
    }
    let firefight_row = mode == Mode::Firefight && actions.contains(&ff::ROW_ACTION);
    let mut out = Vec::new();
    for &a in actions
        .iter()
        .filter(|a| **a != FORGE_MAP_ACTION && shown(mode, **a))
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
    Some(out)
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
/// Firefight's wave set, difficulty and starting lives (`firefight`, the
/// options page's choices, or why they cannot be read).
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
                let difficulty = choice_label(&ff::DIFFICULTY, difficulty_value(o.difficulty));
                let lives = match o.lives {
                    None => "The wave set's own lives".to_owned(),
                    Some(1) => "1 starting life".to_owned(),
                    Some(n) => format!("{n} starting lives"),
                };
                vec![
                    format!("{wave} waves"),
                    format!("{difficulty} difficulty"),
                    lives,
                ]
            }
        },
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
/// guest memory first; none in Multiplayer.
pub fn panel_ids(m: &mut dyn GuestMemory, store: &Store) -> Result<Vec<u32>, MemFault> {
    let state = lobby_start::state();
    let firefight = (state.mode == Mode::Firefight).then(|| ff::Options::read(store));
    let lines = panel_lines(state.mode, state.forge_map.as_deref(), firefight);
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

/// Whether the lobby's Start Game is refused: a Forge lobby with a member
/// on another console (one that cannot be read counts as one).
pub fn refused(mode: Mode, remote: Option<u32>) -> bool {
    mode == Mode::Forge && remote != Some(0)
}

/// [`refused`] for the lobby now.
pub fn start_refused(m: &dyn GuestMemory) -> bool {
    let state = lobby_start::state();
    state.lobby && refused(state.mode, remote_members(m))
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
/// the mode's arrangement sees the rows they add.
fn after_rows(h: hooks::HookRef<'_>) {
    h.after_feature(crate::clankers::ROW_FEATURE)
        .after_feature(crate::clankermatch::SWITCH_FEATURE)
        .after_feature(h1row::ROW_FEATURE)
        .after_feature(ff::ROW_FEATURE);
}

/// The Forge Map row, the rows by mode, the mode strip's bumpers, the start
/// refusal and the leave.
pub struct LobbyMode {
    pub offer: Offer,
    /// The settings store Firefight's choices live in.
    pub store: Option<Arc<Store>>,
    /// The data directory, whose `forge` folder holds the saved maps.
    pub data: PathBuf,
}

fn row_text(c: &mut Call, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let (item, id) = {
        let mut mem = rt.memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + 0x70).ok())
            .and_then(|row| h1row::row_action(&*mem, c.cpu.ecx, row));
        if action != Some(FORGE_MAP_ACTION) {
            return Ok(Flow::Continue);
        }
        let slot = FORGE_MAP_LABEL_SLOT;
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
    });
    TEXTS.rewrite_all();
    Ok(())
}

impl hooks::Feature for LobbyMode {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "The party lobby: its mode strip and the modes of ours",
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
        b.locale().source("lobby mode", |id| TEXTS.address(id));

        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        // The mode strip's text blocks (`crate::lobby_tabs::strip`).
        let blocks = b.guest_alloc(0x1000, Permissions::RW)?;
        crate::lobby_tabs::strip::install(blocks);
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
        after_rows(
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
                }),
        );
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
        });
        after_rows(b.raw().before(
            &SQUAD_SETTINGS_ROW_CHOSEN,
            "the lobby mode rows' press",
            move |c| row_chosen(c, &me, request_at),
        ));

        // Start Game in a Forge lobby with a member on another console.
        b.raw()
            .after(
                &points::LOBBY_START_STATE,
                "refuse Forge across consoles",
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
        // Leaving the lobby clears its mode and the strip's reason line.
        b.raw()
            .before(
                &points::PARTY_LEAVE,
                "the lobby's mode ends with it",
                |_| {
                    lobby_start::clear();
                    crate::lobby_tabs::clear_notice();
                    Ok(Flow::Continue)
                },
            )
            .optional();
        // The mode strip's bumpers (docs/game/party-lobby.md, "Switching").
        let store = self.store.clone();
        b.raw().before(
            &points::PREGAME_LOBBY_INPUT,
            "the mode strip's bumpers",
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
