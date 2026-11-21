//! The lobby's modes on Game Setup (docs/game/forge-firefight-ui.md, "The
//! lobbies"): Multiplayer, Forge and Firefight as modes of the retail
//! pregame lobby, chosen on a row of ours.
//!
//! **The Mode row.** "Change Mode..." ([`MODE_ACTION`]) is the first row of
//! every Game Setup list that has Change Map (Split Screen, a hosted System
//! Link game, an Xbox Live custom game), while a mode besides Multiplayer is
//! on offer ([`Offer::any`]). A opens the value picker
//! ([`crate::ui_kit::picker`]) of the modes on offer ([`mode_pick`]) and
//! closes Game Setup behind it, as retail's Switch To rows open their screen
//! and close the dialog: Game Setup builds its list each time it opens
//! (`crate::h1_options_row`), so the next open has the new mode's rows. The
//! skin has no value column, so the mode shows in the row's help line and
//! the lobby's header, never as "Mode: Forge".
//!
//! **Rows by mode** ([`arrange`]): an `after` on the list's constructor,
//! after every other row of ours is added, rewrites the list
//! ([`crate::h1_options_row::rewrite_rows`]): Mode first; in Forge,
//! **Forge Map...** ([`FORGE_MAP_ACTION`]) after Change Map, the game type
//! row kept, Clanker Options and Firefight Options hidden; in Firefight,
//! Firefight Options after Change Map and the game type row hidden.
//!
//! **Forge Map...** opens the picker of New and the maps saved under
//! `<data>/forge` ([`forge_map_pick`]); the Forge session loads the choice
//! as it opens ([`crate::forge_glue`]).
//!
//! **The panel and header** ([`panel_ids`], [`set_header`]): the lobby's
//! variant panel update (`crate::lobby_options`) adds the mode's lines and
//! sets the header to FORGE or FIREFIGHT, putting retail's back in
//! Multiplayer.
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

use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;
use settings::{Store, Value};

use crate::firefight::options as ff;
use crate::h1_options_row::{self as h1row, CHANGE_MAP};
use crate::lobby_start::{self, Mode, Offer};
use crate::ui_kit::ids::{self, Family};
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::screen::SlotTexts;
use crate::ui_kit::view::Opener;

/// The feature (`features.lobby-mode` switches it).
pub const FEATURE: &str = "lobby-mode";
/// The Mode row's action: past Firefight Options' 13 (retail uses 0 to 9;
/// the row-chosen jump table has no bounds check, so the press never
/// reaches the original).
pub const MODE_ACTION: i16 = 14;
/// The Forge Map row's action.
pub const FORGE_MAP_ACTION: i16 = 15;
/// Retail's game type row, "Change Rules...".
pub const CHANGE_RULES: i16 = 1;

/// The rows' texts, the help lines, the header and the panel lines.
pub const IDS: Family = ids::family("lobby mode");
const MODE_LABEL_SLOT: u32 = 0;
const FORGE_MAP_LABEL_SLOT: u32 = 1;
const MODE_HELP_SLOT: u32 = 2;
const FORGE_MAP_HELP_SLOT: u32 = 3;
const HEADER_SLOT: u32 = 4;
const PANEL_SLOTS: u32 = 5;
/// The most panel lines a mode has (Firefight's three).
pub const MAX_PANEL_LINES: usize = 3;

pub const MODE_LABEL: &str = "Change Mode...";
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
/// besides Multiplayer on offer.
pub fn arrange(mode: Mode, offer: Offer, actions: &[i16]) -> Option<Vec<i16>> {
    if !offer.any() || !actions.contains(&CHANGE_MAP) {
        return None;
    }
    let ours = |a: &i16| *a == MODE_ACTION || *a == FORGE_MAP_ACTION;
    let firefight_row = mode == Mode::Firefight && actions.contains(&ff::ROW_ACTION);
    let mut out = vec![MODE_ACTION];
    for &a in actions.iter().filter(|a| !ours(a) && shown(mode, **a)) {
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

/// The Mode row's help line: the lobby's mode and what it plays, or why
/// Firefight has no map to play on.
pub fn mode_help(mode: Mode, kit_maps: usize) -> String {
    if mode == Mode::Firefight && kit_maps == 0 {
        return "This lobby plays Firefight, and no installed map carries its kit. \
                Build one from a disc map with h2 firefight kit MAP."
            .into();
    }
    format!("This lobby plays {}. {}", mode.label(), mode.description())
}

/// The Forge Map row's help line: the map's description.
pub fn forge_map_help(name: Option<&str>) -> String {
    match name {
        None => "New starts from the map as it is.".into(),
        Some(name) => format!("Loads the saved map {name} and its edits."),
    }
}

/// The Mode row's pick: the modes on offer, the lobby's held. Choosing
/// Firefight holds the `firefight` setting on for the games the lobby
/// starts (`store`).
pub fn mode_pick(offer: Offer, current: Mode, store: Option<Arc<Store>>) -> Pick {
    let modes = offer.modes();
    let held = modes.iter().position(|&m| m == current).unwrap_or(0);
    let description = modes
        .iter()
        .map(|m| m.summary())
        .collect::<Vec<_>>()
        .join(" ");
    let values = modes.iter().map(|m| m.label().to_owned()).collect();
    Pick::new(
        "Mode",
        values,
        held,
        &description,
        Arc::new(move |i: usize| {
            let Some(&mode) = modes.get(i) else {
                return format!("mode {i} is not on offer");
            };
            lobby_start::set_mode(mode);
            if mode == Mode::Firefight {
                match store
                    .as_ref()
                    .map(|s| s.set(ff::ENABLED.key, Value::Bool(true)))
                {
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        return format!("lobby mode: Firefight, but {}: {e}", ff::ENABLED.key);
                    }
                    None => return "lobby mode: Firefight, but no settings store holds it".into(),
                }
            }
            format!("lobby mode: {}", mode.label())
        }),
    )
    .over(Opener::GAME_SETUP_MODE)
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

// ---- the panel and the header ----

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

const WIDGET_CHILD: u32 = 0x14;
const WIDGET_NEXT: u32 = 0x18;
const WIDGET_KIND: u32 = 4;
const TEXT_KIND: u32 = 6;
const TEXT_VTABLE: u32 = 0x0045_8940;
/// A text widget's inline UTF-16 buffer (`crate::lobby_options`).
const TEXT_BUFFER: u32 = 0x74 + 0x44;
const TEXT_BYTES: usize = 256 * 2;

/// The lobby screen's header: its first text widget, the built-in header
/// (`crate::lobby_options`: the first two texts are the header and the
/// footer).
pub fn header_widget(m: &dyn GuestMemory, screen: u32) -> Option<u32> {
    let mut child = m.u32(screen.checked_add(WIDGET_CHILD)?).ok()?;
    for _ in 0..128 {
        if child == 0 {
            return None;
        }
        if m.u32(child.checked_add(WIDGET_KIND)?).ok()? == TEXT_KIND {
            return (m.u32(child).ok()? == TEXT_VTABLE).then_some(child);
        }
        child = m.u32(child.checked_add(WIDGET_NEXT)?).ok()?;
    }
    None
}

/// Retail's header text on the screen it was taken from, kept while ours
/// shows so Multiplayer can put it back.
static RETAIL_HEADER: Mutex<Option<(u32, Vec<u8>)>> = Mutex::new(None);

/// Sets the lobby `screen`'s header for its mode, after the panel update:
/// FORGE or FIREFIGHT through `TEXT_SET`, keeping retail's text; in
/// Multiplayer, retail's put back.
pub async fn set_header(cx: &Cx, screen: u32) -> Result<(), EntryError> {
    let Some(widget) = cx.read(|m| header_widget(m, screen)) else {
        return Ok(());
    };
    let header = lobby_start::mode().header();
    let saved = RETAIL_HEADER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .filter(|(s, _)| *s == screen);
    match header {
        None => {
            if let Some((_, bytes)) = saved {
                cx.write(|m| m.write(widget + TEXT_BUFFER, &bytes))?;
                *RETAIL_HEADER.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        }
        Some(text) => {
            if saved.is_none() {
                let bytes = cx.read(|m| {
                    let mut b = vec![0; TEXT_BYTES];
                    m.read(widget + TEXT_BUFFER, &mut b).map(|()| b)
                })?;
                *RETAIL_HEADER.lock().unwrap_or_else(|e| e.into_inner()) = Some((screen, bytes));
            }
            let id = TEXTS.set(HEADER_SLOT, text);
            cx.write(|m| TEXTS.write(m))?;
            cx.call(GuestCall::new(points::TEXT_SET.base).edi(widget).args([id]))
                .await?;
        }
    }
    Ok(())
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

/// The Mode and Forge Map rows, the rows by mode, the start refusal and the
/// leave.
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
        let slot = match action {
            Some(MODE_ACTION) => MODE_LABEL_SLOT,
            Some(FORGE_MAP_ACTION) => FORGE_MAP_LABEL_SLOT,
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
    let (action, player, screen, stack) = {
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
        (action, player, screen, stack)
    };
    let pick = match action {
        Some(MODE_ACTION) => mode_pick(me.offer, lobby_start::mode(), me.store.clone()),
        Some(FORGE_MAP_ACTION) => forge_map_pick(
            ::forge::saved_names(&me.data),
            lobby_start::forge_map().as_deref(),
        ),
        _ => return Ok(Flow::Continue),
    };
    let what = if action == Some(MODE_ACTION) {
        "Mode"
    } else {
        "Forge Map"
    };
    let loader = picker::offer_list(pick).and_then(list_class_loader);
    let (Some(player), Some(screen), Some((window, slot)), Some(loader)) =
        (player, screen, stack, loader)
    else {
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
    // The Mode picker closes Game Setup behind it, so the dialog is built
    // again, with the new mode's rows, when it next opens.
    let close = action == Some(MODE_ACTION);
    eprintln!(
        "lobby mode: opening the {what} picker (player {player}) on window {window}, slot {slot}"
    );
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            if close {
                crate::ui_kit::dialog_tag::close(&cx, screen).await?;
            }
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
            "Multiplayer, Forge and Firefight as modes of the lobby",
        )
        .doc("docs/game/forge-firefight-ui.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::{
            SQUAD_SETTINGS_LIST_CTOR as CTOR, SQUAD_SETTINGS_ROW_CHOSEN, SQUAD_SETTINGS_ROW_TEXT,
        };
        // The texts: a slot each, served to TEXT_SET by our locale source.
        let len = IDS.len * crate::ui_kit::screen::SLOT_LEN;
        let base = b.guest_alloc(len.next_multiple_of(0x1000), Permissions::RW)?;
        TEXTS.base.store(base, Ordering::SeqCst);
        TEXTS.set(MODE_LABEL_SLOT, MODE_LABEL);
        TEXTS.set(FORGE_MAP_LABEL_SLOT, FORGE_MAP_LABEL);
        b.locale().source("lobby mode", |id| TEXTS.address(id));

        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
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
        // Leaving the lobby clears its mode.
        b.raw()
            .before(
                &points::PARTY_LEAVE,
                "the lobby's mode ends with it",
                |_| {
                    lobby_start::clear();
                    Ok(Flow::Continue)
                },
            )
            // The bots' own leave runs first; clearing the mode touches none of their state.
            .after_feature("bots")
            .optional();

        b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
            move || h1row::building_live(&live),
            save_state,
            |bytes, _| load_state(bytes),
        ));
        Ok(())
    }
}

/// Adds the rows and registers their help lines.
pub fn install(registry: &mut hooks::Registry, feature: LobbyMode) -> anyhow::Result<()> {
    crate::game_setup_help::register(
        MODE_ACTION,
        crate::game_setup_help::image::CUSTOM_GAME,
        Arc::new(|m: &mut dyn GuestMemory| {
            let kit = crate::map_select::current().map_or(0, |c| lobby_start::kit_maps(&c).len());
            let id = TEXTS.set(MODE_HELP_SLOT, &mode_help(lobby_start::mode(), kit));
            TEXTS.write(m).ok()?;
            Some(id)
        }),
    );
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
