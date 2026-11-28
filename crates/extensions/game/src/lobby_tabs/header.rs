//! The lobby's header and its prompt line (docs/game/party-lobby.md,
//! "The header and the prompt"), and the lobby as the modes read it.
//!
//! **The header** is the lobby tag's own title widget, the first text
//! widget of the screen. After retail's panel update writes it, the
//! update ([`update`]) sets its text to the lobby's mode
//! ([`Tab::label`]: CUSTOM GAMES, FORGE, FIREFIGHT, MULTIPLAYER, CO-OP)
//! with `TEXT_SET`, so the header names the mode on every update.
//!
//! **The prompt line** is a text block of the lobby tag's pane 0, one row
//! above its privacy line: block [`PROMPT_BLOCK`], which the lobby's tag
//! edit appends (docs/game/party-lobby.md, "The prompt line"). The update
//! sets its text ([`super::CHANGE_MODE_TEXT`]) and shows it for the party
//! leader only, and not while the countdown runs or a search has its lobby
//! (its visible byte). A lobby whose tag has no such block has no prompt
//! line, and the update leaves it alone.
//!
//! **The game line** is the lobby's "<game type> on <map>" text, which
//! retail's panel update writes from the record's game type and the map
//! slot's name. A game with a mode of its own (Firefight, Siege, a rules
//! script) plays on the Slayer record and a Firefight map is a kit
//! package, so retail's line reads "Slayer on Firefight – Lockout". The
//! update writes the line from the mode and the variant instead
//! ([`own_mode`], [`title`]): "Firefight: Holdout on Lockout". Any other
//! game on a kit package keeps retail's game and names the map plainly
//! ([`on_plain_map`]).
//!
//! **The update** runs where the lobby's panel is (`crate::lobby_options`,
//! on `LOBBY_VARIANT_PANEL`). It reads the lobby ([`read`]), works out its
//! mode ([`shown`]), runs a switch a Live menu row asked for
//! ([`super::request`]), then writes the header and the prompt line.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bots::lobby;
use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use serde_json::{Value, json};
use settings::Store;

use super::{Avail, Facts, Place, Present, RetailMode, Tab, look, row, seen_mode};
use crate::layout_glue::ReadGm;
use crate::lobby_start::Mode;

/// The lobby's header rectangle as the disc's tag has it: pane 0's text
/// block 0 (`[135 -258 61 45]`, font 4). The tag edit shortens it to
/// `[135 -258 95 45]` to make room for the prompt line; its width stays.
pub const HEADER: [i16; 4] = [135, -258, 61, 45];
/// The prompt line's block index in the lobby's pane 0: the first after
/// the tag's 48 text blocks, where the tag edit appends it.
pub const PROMPT_BLOCK: u16 = 48;

const WIDGET_KIND: u32 = 4;
const WIDGET_INDEX: u32 = 0xa;
const WIDGET_CHILD: u32 = 0x14;
const WIDGET_NEXT: u32 = 0x18;
/// The visible byte retail's panel update sets (`crate::lobby_options`).
const WIDGET_VISIBLE: u32 = 0x6e;
const TEXT_KIND: u32 = 6;
const TEXT_VTABLE: u32 = 0x0045_8940;
/// The lobby screen's pane index (`crate::lobby_options`): 0 is the custom
/// game pane.
const SCREEN_PANE: u32 = 0x142c;
/// The most children a walk reads.
const MAX_CHILDREN: usize = 256;

// ---- the widgets on the screen ----

/// The header and the prompt line on a lobby screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// The first text widget: the lobby's header.
    pub header: Option<u32>,
    /// The text widget of block [`PROMPT_BLOCK`] on pane 0.
    pub prompt: Option<u32>,
    /// The two text widgets retail's panel update writes besides the
    /// option rows ([`GAME_LINES`]): one of them is the game line.
    pub lines: [Option<u32>; 2],
}

/// The direct text children retail's panel update (`0x2501c0`) looks up
/// for its own lines, counted as its lookup counts them (`0x22edb8(6, n)`
/// at `0x250252` and `0x250260`).
pub const GAME_LINES: [usize; 2] = [3, 4];
/// A text widget's inline UTF-16 buffer and its room
/// (`crate::lobby_options`).
const TEXT_BUFFER: u32 = 0x74 + 0x44;
const TEXT_UNITS: usize = 255;

// ---- the game line ----

/// The mode a lobby's game has of its own, and its variant's name; `None`
/// when the record plays as its own game type and retail's line stands.
/// `lobby` is the lobby's mode, `script` the record's script mode, `siege`
/// whether it is the Siege game type, `record` its name and `waves` the
/// wave set a Firefight lobby plays. A mode's label is `None` for a rules
/// script with no mode name: its variant's name is the game.
pub fn own_mode(
    lobby: Mode,
    script: variants::ext::Mode,
    siege: bool,
    record: &str,
    waves: Option<&str>,
) -> Option<(Option<&'static str>, String)> {
    use variants::ext::Mode as Script;
    match (script, lobby) {
        // A Firefight variant names its wave set.
        (Script::Firefight, _) => Some((Some(crate::map_modes::FIREFIGHT_TAG), record.to_owned())),
        // A Firefight lobby plays the options page's set on any record.
        (_, Mode::Firefight) => Some((Some(crate::map_modes::FIREFIGHT_TAG), waves.unwrap_or_default().to_owned())),
        (Script::Rules, _) if siege => Some((Some(crate::map_modes::SIEGE_TAG), record.to_owned())),
        (Script::Rules, _) => Some((None, record.to_owned())),
        (Script::None | Script::Unknown(_), _) => None,
    }
}

/// The game line of a game with a mode of its own: `Firefight: Holdout on
/// Lockout`. The mode stands alone when the variant has no other name
/// (`Siege on Lockout`), a variant without a mode name stands alone
/// (`Bounce Practice on Lockout`), and a kit package is named as its map
/// (`crate::lobby_start::KIT_NAME_PREFIX`). The record's base game type is
/// never part of it.
pub fn title(mode: Option<&str>, variant: &str, map: &str) -> String {
    let map = map.strip_prefix(crate::lobby_start::KIT_NAME_PREFIX).unwrap_or(map).trim();
    let variant = variant.trim();
    let game = match mode {
        Some(mode) if variant.is_empty() || variant.eq_ignore_ascii_case(mode) => mode.to_owned(),
        Some(mode) => format!("{mode}: {variant}"),
        None => variant.to_owned(),
    };
    match (game.is_empty(), map.is_empty()) {
        (false, false) => format!("{game} on {map}"),
        (false, true) => game,
        (true, _) => map.to_owned(),
    }
}

/// Whether the text `shown` on a widget is retail's game line for the map
/// slot named `map`: it ends with the slot's name and says more than it.
pub fn is_game_line(shown: &str, map: &str) -> bool {
    let shown = shown.trim();
    !map.is_empty() && shown.len() > map.len() && shown.ends_with(map)
}

/// Retail's game line `shown` for the map slot named `map`, with a kit
/// package named as its map: `None` when the slot is no kit package. A
/// game with no mode of its own on a kit map keeps retail's game and
/// loses only the package's prefix.
pub fn on_plain_map(shown: &str, map: &str) -> Option<String> {
    let plain = map.strip_prefix(crate::lobby_start::KIT_NAME_PREFIX)?.trim();
    let game = shown.trim().strip_suffix(map)?;
    Some(format!("{game}{plain}"))
}

/// The lobby map slot's name, and the game line for it when the lobby's
/// game has a mode of its own ([`own_mode`]).
fn game_line(m: &dyn GuestMemory, store: Option<&Store>) -> Option<(Option<String>, String)> {
    let id = m.u32(crate::h1_options_row::LOBBY_MAP).ok()?;
    let catalogue = crate::map_select::current()?;
    let map = catalogue.all().find(|map| map.id == id)?.name.clone();
    let record = crate::pause_change_map::lobby_variant(m)?;
    let lobby = crate::lobby_start::mode();
    #[cfg(feature = "rules")]
    let siege = crate::rules::gate::is_siege_record(&record);
    #[cfg(not(feature = "rules"))]
    let siege = false;
    let waves = (lobby == Mode::Firefight)
        .then(|| store.and_then(|store| crate::lobby_mode::firefight_waves(m, store)))
        .flatten();
    let ours = own_mode(lobby, record.ext().script.mode, siege, &record.name(), waves.as_deref())
        .map(|(mode, variant)| title(mode, &variant, &map));
    Some((ours, map))
}

/// The lobby `screen`'s header and prompt line, read from its children;
/// `None` when the children cannot be read.
pub fn find(m: &dyn GuestMemory, screen: u32) -> Option<Found> {
    let mut f = Found::default();
    let pane0 = m.u16(screen.checked_add(SCREEN_PANE)?).ok()? == 0;
    let mut child = m.u32(screen.checked_add(WIDGET_CHILD)?).ok()?;
    let mut texts = 0usize;
    for _ in 0..MAX_CHILDREN {
        if child == 0 {
            return Some(f);
        }
        let kind = m.u32(child.checked_add(WIDGET_KIND)?).ok()? == TEXT_KIND;
        let text = kind && m.u32(child).ok()? == TEXT_VTABLE;
        if kind {
            if let Some(line) = GAME_LINES.iter().position(|&n| n == texts) {
                f.lines[line] = text.then_some(child);
            }
            texts += 1;
        }
        if text {
            if f.header.is_none() {
                f.header = Some(child);
            } else if pane0
                && f.prompt.is_none()
                && m.u16(child.checked_add(WIDGET_INDEX)?).ok()? == PROMPT_BLOCK
            {
                f.prompt = Some(child);
            }
        }
        child = m.u32(child.checked_add(WIDGET_NEXT)?).ok()?;
    }
    Some(f)
}

// ---- the lobby's mode ----

/// The lobby's modes as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub place: Place,
    /// This console leads the party: the prompt line shows and X opens the
    /// picker.
    pub leader: bool,
    /// The lobby's mode: the header's text.
    pub selected: Tab,
    pub row: Vec<(Tab, Avail)>,
    /// On MULTIPLAYER, whether the lobby's playlist is a Clanker one
    /// (`None` in any other mode, or with no playlist held).
    pub clanker: Option<bool>,
}

impl Shown {
    /// Whether mode `tab` can be switched to here, or its reason; `None`
    /// for a mode not present.
    pub fn avail(&self, tab: Tab) -> Option<Avail> {
        self.row.iter().find(|(t, _)| *t == tab).map(|(_, a)| *a)
    }

    /// Whether the prompt line shows: the leader's, while no lock holds the
    /// lobby (`locked`, [`super::switch::locked`]).
    pub fn prompt_visible(&self, locked: bool) -> bool {
        self.leader && !locked
    }
}

/// The lobby as the modes read it from guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub place: Place,
    pub leader: bool,
    pub retail: RetailMode,
    /// The lobby's playlist is a Clanker one (`None` with no playlist
    /// held).
    pub clanker: Option<bool>,
    pub facts: Facts,
}

/// The modes for `reading`, with `mode` the lobby's mode of ours.
pub fn shown(reading: &Reading, present: Present, mode: Mode) -> Shown {
    let mode = seen_mode(reading.leader, mode);
    let selected = Tab::of(reading.retail, mode);
    Shown {
        place: reading.place,
        leader: reading.leader,
        selected,
        row: row(present, reading.place, reading.facts),
        clanker: reading.clanker.filter(|_| selected == Tab::Multiplayer),
    }
}

/// The modes now: the lobby read with the modes present and our mode.
pub fn shown_now(m: &dyn GuestMemory) -> Option<Shown> {
    read(m).map(|r| shown(&r, super::present(), crate::lobby_start::mode()))
}

/// The lobby's place: Split Screen when not networked, else the squad
/// session's network class (`crate::variant_ext::SESSION_NETWORK_CLASS`,
/// `+0x18`: 2 Xbox Live, 1 System Link).
pub fn place(networking: bool, class: Option<u32>) -> Option<Place> {
    match (networking, class) {
        (false, _) => Some(Place::SplitScreen),
        (true, Some(2)) => Some(Place::XboxLive),
        (true, Some(1)) => Some(Place::SystemLink),
        (true, _) => None,
    }
}

/// The lobby's place now ([`place`]). The class is the squad session's
/// network class at `+0x18`, which session creation (`0x59bd0`, from
/// `0x59890(class)`) stores and Live's paths create as 2. It is not
/// `+0x4980`, the squad's stage (`0x5a220`), which is 1 in any pregame
/// lobby, a Live party's too.
pub fn place_now(m: &dyn GuestMemory) -> Option<Place> {
    use crate::variant_ext::SESSION_NETWORK_CLASS;
    let networking = m.u8(lobby::NETWORKING).ok()? != 0;
    let session = m.u32(lobby::SESSION).ok().filter(|&s| s != 0);
    let class = session.and_then(|s| m.u32(s.checked_add(SESSION_NETWORK_CLASS)?).ok());
    place(networking, class)
}

/// The squad session's mode (`+0x49a4`), read by `0x64160` while the
/// session is in states 3 to 8: 2 is a matchmaking lobby, the lobby kind 6
/// of `0x19989d` (Game Setup's Change Playlist... list).
pub const SQUAD_MODE: u32 = 0x49a4;
pub const SQUAD_MATCHMAKING: u32 = 2;

/// The squad session's mode as `0x64160` reads it: `None` unless
/// networking is on and the squad session is in states 3 to 8.
pub fn squad_mode(m: &dyn GuestMemory) -> Option<u32> {
    if m.u8(lobby::NETWORKING).ok()? == 0 {
        return None;
    }
    let s = m.u32(lobby::SESSION).ok().filter(|&s| s != 0)?;
    let state = m.u32(s.checked_add(lobby::STATE)?).ok()?;
    if !(3..=8).contains(&state) {
        return None;
    }
    m.u32(s.checked_add(SQUAD_MODE)?).ok()
}

/// The retail lobby mode: matchmaking in a matchmaking life cycle (types 6
/// and 7) or on a squad session in matchmaking mode ([`squad_mode`]),
/// co-op on a campaign session, else the custom game lobby.
pub fn retail_mode(
    networking: bool,
    session_type: u32,
    squad_mode: Option<u32>,
    campaign: bool,
) -> RetailMode {
    if networking
        && (lobby::matchmaking_type(session_type) || squad_mode == Some(SQUAD_MATCHMAKING))
    {
        RetailMode::Matchmaking
    } else if campaign {
        RetailMode::CoOp
    } else {
        RetailMode::Custom
    }
}

/// The lobby's retail mode now, for the Clankers driver
/// (`crate::clankers::admits_page`); `None` while it cannot be read.
pub fn retail_now(m: &dyn GuestMemory) -> Option<RetailMode> {
    let networking = m.u8(lobby::NETWORKING).ok()? != 0;
    let session_type = m.u32(lobby::SESSION_TYPE).ok()?;
    let campaign = crate::coop_probe::read_session(&ReadGm(m)).is_some_and(|s| s.is_campaign());
    Some(retail_mode(
        networking,
        session_type,
        squad_mode(m),
        campaign,
    ))
}

/// The lobby's playlist, when one is held (`LOBBY_HOPPER_HELD`).
pub fn held_hopper(m: &dyn GuestMemory) -> Option<u32> {
    use super::switch::{LOBBY_HOPPER, LOBBY_HOPPER_HELD};
    (m.u8(LOBBY_HOPPER_HELD).ok()? != 0)
        .then(|| m.u32(LOBBY_HOPPER).ok())
        .flatten()
        .filter(|&h| h != u32::MAX)
}

static LIVE: Mutex<Option<Arc<live_hle::Hle>>> = Mutex::new(None);

/// The Live service whose marks tell a Clanker playlist (boot, with
/// Clankermatch on).
pub fn set_live(hle: Option<Arc<live_hle::Hle>>) {
    *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = hle;
}

pub(crate) fn clanker_playlist(hopper: u32) -> bool {
    LIVE.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|live| crate::clankermatch::is_clanker(hopper, &live.clanker_hoppers()))
}

/// Reads the lobby; `None` while its session cannot be read.
pub fn read(m: &dyn GuestMemory) -> Option<Reading> {
    let networking = m.u8(lobby::NETWORKING).ok()? != 0;
    let place = place_now(m)?;
    let leader = !networking || lobby::facts(m).ok()?.leader;
    let session_type = m.u32(lobby::SESSION_TYPE).ok()?;
    let campaign = crate::coop_probe::read_session(&ReadGm(m)).is_some_and(|s| s.is_campaign());
    let retail = retail_mode(networking, session_type, squad_mode(m), campaign);
    let clanker = (retail == RetailMode::Matchmaking)
        .then(|| held_hopper(m))
        .flatten()
        .map(clanker_playlist);
    Some(Reading {
        place,
        leader,
        retail,
        clanker,
        facts: Facts {
            // The playlists are asked for by a switch to MULTIPLAYER
            // (`switch::listed_hoppers`); until one finds none, a Live
            // lobby's are taken as there.
            playlists: place == Place::XboxLive && !super::switch::no_playlists_seen(),
            live_co_op: crate::coop_live::lobby().is_some(),
            remote_members: crate::lobby_mode::remote_members(m) != Some(0),
        },
    })
}

// ---- the update ----

static ON: AtomicBool = AtomicBool::new(false);

/// The header and the prompt are installed (the party lobby feature,
/// `crate::lobby_mode`).
pub fn install() {
    ON.store(true, Ordering::SeqCst);
}

/// Whether the header and the prompt are installed.
pub fn on() -> bool {
    ON.load(Ordering::SeqCst)
}

static PASSING: AtomicBool = AtomicBool::new(false);

/// The party lobby's own `before` on `LOBBY_VARIANT_PANEL` (EAX the lobby
/// screen, the variant on the stack, `ret 4`), ordered after
/// `lobby-options`'s: it runs retail's panel update, then ours
/// ([`update`]), with `store` the settings a switch holds Firefight's
/// choice in. With `lobby-options` on, its answer's call to retail comes
/// through here; with it off, this is the only answer. Either way the
/// header names the mode.
pub fn panel(c: &mut hooks::Call, store: Option<Arc<Store>>) -> Result<hooks::Flow, runtime::GuestFault> {
    if PASSING.load(Ordering::SeqCst) {
        return Ok(hooks::Flow::Continue);
    }
    let screen = c.cpu.eax;
    let Some(variant) = c.arg_u32("variant") else {
        return Ok(hooks::Flow::Continue);
    };
    c.answer_async(move |cx| -> EntryFuture { Box::pin(panel_entry(cx, screen, variant, store)) })
}

/// [`panel`]'s answer: retail's panel update for `variant` on `screen`,
/// then ours.
pub async fn panel_entry(
    cx: Cx,
    screen: u32,
    variant: u32,
    store: Option<Arc<Store>>,
) -> Result<Ret, EntryError> {
    PASSING.store(true, Ordering::SeqCst);
    let retail = cx
        .call(
            GuestCall::new(points::LOBBY_VARIANT_PANEL.base)
                .eax(screen)
                .args([variant]),
        )
        .await;
    PASSING.store(false, Ordering::SeqCst);
    let retail = retail?;
    update(&cx, screen, store).await?;
    Ok(Ret::new(retail.eax, 4))
}

/// Our update on the lobby `screen`, after its panel update: a switch a
/// Live menu row asked for, in the custom lobby that row made; then the
/// header set to the mode and the prompt line shown or hidden.
pub async fn update(cx: &Cx, screen: u32, store: Option<Arc<Store>>) -> Result<(), EntryError> {
    if !on() {
        return Ok(());
    }
    let Some(reading) = cx.read(|m| read(m)) else {
        return Ok(());
    };
    let state = shown(&reading, super::present(), crate::lobby_start::mode());
    if state.leader
        && reading.retail == RetailMode::Custom
        && let Some(to) = super::take_request()
    {
        super::switch::run_to(cx, to, super::present(), store).await?;
        // The next update draws the mode the switch left.
        return Ok(());
    }
    let Some(found) = cx.read(|m| find(m, screen)) else {
        return Ok(());
    };
    if let Some(header) = found.header {
        set_text(cx, screen, header, state.selected.id()).await?;
    }
    // Retail has just written "<game type> on <map slot>"; a game with a
    // mode of its own is named by that mode and its variant instead.
    if let Some((ours, map)) = cx.read(|m| game_line(m, store.as_deref())) {
        for widget in found.lines.into_iter().flatten() {
            let Some(shown) = cx.read(|m| m.utf16(widget + TEXT_BUFFER, TEXT_UNITS).ok()) else {
                continue;
            };
            if !is_game_line(&shown, &map) {
                continue;
            }
            let Some(line) = ours.clone().or_else(|| on_plain_map(&shown, &map)) else {
                continue;
            };
            if line != shown
                && let Some(id) = cx.write(|m| crate::lobby_mode::title_id(m, &line))
            {
                set_text(cx, screen, widget, id).await?;
            }
        }
    }
    if let Some(prompt) = found.prompt {
        set_text(cx, screen, prompt, super::CHANGE_MODE_ID).await?;
        let locked = locked_now(cx).await?;
        let visible = u8::from(state.prompt_visible(locked));
        cx.write(|m| m.set_u8(prompt + WIDGET_VISIBLE, visible))?;
    }
    Ok(())
}

/// `TEXT_SET` of string `id` on `widget` of `screen`.
async fn set_text(cx: &Cx, screen: u32, widget: u32, id: u32) -> Result<(), EntryError> {
    cx.call(
        GuestCall::new(points::TEXT_SET.base)
            .edi(widget)
            .esi(screen)
            .args([id]),
    )
    .await?;
    Ok(())
}

/// Whether a lock holds the lobby now: the countdown, the map loading, or
/// a search with its lobby ([`super::switch::locked`]).
async fn locked_now(cx: &Cx) -> Result<bool, EntryError> {
    let state = cx
        .call(GuestCall::new(points::LOBBY_START_STATE.base).args([0]))
        .await?
        .eax;
    let searching = cx.read(|m| lobby::matchmaking_cycle(m));
    Ok(super::switch::locked(state, searching).is_some())
}

// ---- the agent API ----

/// `ui.lobby.tabs`: the lobby's modes as the lobby on top has them, or why
/// there are none.
pub fn inspect(m: &dyn GuestMemory) -> Result<Value, &'static str> {
    if !on() {
        return Err("the party lobby's modes are not installed (no settings store)");
    }
    let top = crate::ui_watch::read(&ReadGm(m))
        .top
        .ok_or("no menu screen is on top")?;
    if top.id != crate::lobby_options::SCREEN {
        return Err("the pregame lobby is not the screen on top");
    }
    let s = shown_now(m).ok_or("the lobby's session cannot be read")?;
    Ok(json_of(&s, super::notice_now()))
}

/// [`inspect`]'s answer for `s`, with `notice` the reason line's.
pub fn json_of(s: &Shown, notice: Option<super::Reason>) -> Value {
    let tabs: Vec<Value> = s
        .row
        .iter()
        .map(|&(tab, avail)| {
            json!({
                "tab": tab.name(),
                "label": tab.label(),
                "state": look(tab, s.selected, avail),
                "reason": avail.reason().map(|r| r.text()),
            })
        })
        .collect();
    json!({
        "place": s.place,
        "leader": s.leader,
        "selected": s.selected.name(),
        "header": s.selected.label(),
        "tabs": tabs,
        "clanker": s.clanker,
        "notice": notice.map(|r| r.text()),
    })
}

#[cfg(test)]
mod tests;
