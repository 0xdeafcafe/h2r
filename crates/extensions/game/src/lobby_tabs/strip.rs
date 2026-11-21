//! The strip drawn on the pregame lobby (docs/game/party-lobby.md, "The
//! strip").
//!
//! **The widgets** are text widgets of the lobby screen, added with
//! `PANE_TEXT_ADD` from blocks of ours in the lobby's font, colours,
//! animation and depth bias ([`block`]). Each tab has three in its cell,
//! one per [`Look`], and each glyph two (bright and dimmed); the update
//! shows one of them by the visible byte, so a state changes what shows,
//! never the widget set. A widget is known by its block index
//! ([`BLOCK_BASE`] and up, past any of the tag's), found again on every
//! update ([`find`]), never by a kept address: the pane's rebuild on a mode
//! change tears them down, and the next update adds what is missing.
//!
//! **The room.** The strip takes the header band (`[204 -270 180 70]`):
//! the header is hidden (its visible byte), as the selected tab is the
//! screen's title, and the player count keeps its right edge and narrows
//! from [`COUNT`] to [`COUNT_NARROW`].
//!
//! **The update** runs where the lobby's panel is (`crate::lobby_options`,
//! on `LOBBY_VARIANT_PANEL`). It reads the lobby ([`read`]), works out
//! the tabs ([`shown`]) and writes the visible bytes ([`show`]).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use bots::lobby;
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points;
use serde_json::{Value, json};

use super::{
    Avail, Dir, Facts, Look, NEXT, NEXT_GLYPH_ID, PREV, PREV_GLYPH_ID, Place, Present, RetailMode,
    Tab, cells, glyph_on, look, row, seen_mode,
};
use crate::layout_glue::ReadGm;
use crate::list_class::{TEXT_BLOCK_SIZE, TextBlock};
use crate::lobby_start::Mode;
use crate::mods_screen::page_layout::{PALE_BLUE, TEXT_ANIMATION, TEXT_BIAS, WHITE};

/// The lobby's header rectangle, the band the strip takes.
pub const HEADER: [i16; 4] = [204, -270, 180, 70];
/// The player count's rectangle, and the one it narrows to with the strip.
pub const COUNT: [i16; 4] = [199, 85, 179, 280];
pub const COUNT_NARROW: [i16; 4] = [199, 200, 179, 280];
/// The lobby's text font (conduit-12).
pub const FONT: i16 = 1;
/// An unfocused retail row's brightness.
pub const DIM: f32 = 0.55;

/// Our widgets' block indices: the first, past any block of the lobby
/// tag's, so no retail lookup by index (`0x22edb8`) finds one of ours.
pub const BLOCK_BASE: u16 = 0x7100;
const LOOKS: u16 = 3;
const GLYPH_LOOKS: u16 = 2;
/// How many widgets the strip has with every tab present.
pub const WIDGETS: u16 = Tab::ALL.len() as u16 * LOOKS + 2 * GLYPH_LOOKS;

const WIDGET_KIND: u32 = 4;
const WIDGET_INDEX: u32 = 0xa;
const WIDGET_CHILD: u32 = 0x14;
const WIDGET_NEXT: u32 = 0x18;
/// A widget's rectangle, in UI units (`crate::ui_audit`).
const WIDGET_RECT: u32 = 0x20;
/// The visible byte retail's panel update sets (`crate::lobby_options`).
const WIDGET_VISIBLE: u32 = 0x6e;
const TEXT_KIND: u32 = 6;
const TEXT_VTABLE: u32 = 0x0045_8940;
/// A text block's string id (`tags::ui::TextBlock::string`).
const BLOCK_STRING: usize = 0x24;
/// The most children a walk reads.
const MAX_CHILDREN: usize = 256;

// ---- the pieces ----

/// One widget of the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Piece {
    /// A tab's label in one look.
    Cell(Tab, Look),
    /// A bumper's glyph, bright (`true`) or dimmed.
    Glyph(Dir, bool),
}

impl Piece {
    /// Its block index.
    pub fn index(self) -> u16 {
        match self {
            Piece::Cell(tab, look) => {
                let l = Look::ALL.iter().position(|&x| x == look).unwrap_or(0) as u16;
                BLOCK_BASE + tab as u16 * LOOKS + l
            }
            Piece::Glyph(dir, on) => {
                let d = match dir {
                    Dir::Prev => 0,
                    Dir::Next => 1,
                };
                BLOCK_BASE + Tab::ALL.len() as u16 * LOOKS + d * GLYPH_LOOKS + u16::from(!on)
            }
        }
    }

    /// The piece of block index `index`, if it is one of ours.
    pub fn of_index(index: u16) -> Option<Piece> {
        let k = index.checked_sub(BLOCK_BASE)?;
        let cells = Tab::ALL.len() as u16 * LOOKS;
        if k < cells {
            let tab = Tab::ALL[usize::from(k / LOOKS)];
            return Some(Piece::Cell(tab, Look::ALL[usize::from(k % LOOKS)]));
        }
        let g = k - cells;
        let dir = match g / GLYPH_LOOKS {
            0 => Dir::Prev,
            1 => Dir::Next,
            _ => return None,
        };
        Some(Piece::Glyph(dir, g % GLYPH_LOOKS == 0))
    }

    /// Its string id.
    pub fn id(self) -> u32 {
        match self {
            Piece::Cell(tab, _) => tab.id(),
            Piece::Glyph(Dir::Prev, _) => PREV_GLYPH_ID,
            Piece::Glyph(Dir::Next, _) => NEXT_GLYPH_ID,
        }
    }

    /// Its rectangle with `present`'s tabs.
    pub fn rect(self, present: Present) -> Option<[i16; 4]> {
        match self {
            Piece::Cell(tab, _) => cells(&present.tabs())
                .into_iter()
                .find(|(t, _)| *t == tab)
                .map(|(_, c)| c),
            Piece::Glyph(Dir::Prev, _) => Some(PREV),
            Piece::Glyph(Dir::Next, _) => Some(NEXT),
        }
    }

    /// Its colour: white selected, pale blue on, each at 55% dimmed.
    pub fn colour(self) -> [f32; 3] {
        let dim = |c: [f32; 3]| c.map(|v| v * DIM);
        match self {
            Piece::Cell(_, Look::Selected) => WHITE,
            Piece::Cell(_, Look::On) => PALE_BLUE,
            Piece::Cell(_, Look::Off) => dim(PALE_BLUE),
            Piece::Glyph(_, true) => WHITE,
            Piece::Glyph(_, false) => dim(WHITE),
        }
    }
}

/// Whether block index `index` is one of the strip's.
pub fn is_ours(index: u16) -> bool {
    Piece::of_index(index).is_some()
}

/// Every piece the strip has with `present`'s tabs.
pub fn pieces(present: Present) -> Vec<Piece> {
    let mut v: Vec<Piece> = present
        .tabs()
        .into_iter()
        .flat_map(|t| Look::ALL.map(|l| Piece::Cell(t, l)))
        .collect();
    for dir in [Dir::Prev, Dir::Next] {
        v.push(Piece::Glyph(dir, true));
        v.push(Piece::Glyph(dir, false));
    }
    v
}

/// The text block a piece's widget is made from: centred, font 1, the
/// lobby texts' animation and depth bias, its string id set.
pub fn block(piece: Piece, present: Present) -> Option<[u8; TEXT_BLOCK_SIZE as usize]> {
    let rect = piece.rect(present)?;
    let mut b = TextBlock {
        rect,
        font: FONT,
        colour: piece.colour(),
        animation: TEXT_ANIMATION,
    }
    .bytes();
    // Centred: neither justification bit (`tags::ui::justification`).
    b[0..4].copy_from_slice(&0u32.to_le_bytes());
    b[BLOCK_STRING..BLOCK_STRING + 4].copy_from_slice(&piece.id().to_le_bytes());
    b[0x28..0x2a].copy_from_slice(&TEXT_BIAS.to_le_bytes());
    Some(b)
}

// ---- the widgets on the screen ----

/// The strip's widgets on a lobby screen, with the header and player count.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub ours: Vec<(Piece, u32)>,
    /// The first text widget not ours: the lobby's header.
    pub header: Option<u32>,
    /// The text widget at [`COUNT`] or [`COUNT_NARROW`].
    pub count: Option<u32>,
}

fn rect_at(m: &dyn GuestMemory, widget: u32) -> Option<[i16; 4]> {
    let at = widget.checked_add(WIDGET_RECT)?;
    let mut r = [0i16; 4];
    for (i, v) in r.iter_mut().enumerate() {
        *v = m.u16(at + 2 * i as u32).ok()? as i16;
    }
    Some(r)
}

fn put_rect(m: &mut dyn GuestMemory, widget: u32, r: [i16; 4]) -> Result<(), MemFault> {
    for (i, v) in r.into_iter().enumerate() {
        m.set_u16(widget + WIDGET_RECT + 2 * i as u32, v as u16)?;
    }
    Ok(())
}

/// The lobby `screen`'s strip widgets, header and count, read from its
/// children; `None` when the children cannot be read.
pub fn find(m: &dyn GuestMemory, screen: u32) -> Option<Found> {
    let mut f = Found::default();
    let mut first_text = true;
    let mut child = m.u32(screen.checked_add(WIDGET_CHILD)?).ok()?;
    for _ in 0..MAX_CHILDREN {
        if child == 0 {
            return Some(f);
        }
        if m.u32(child.checked_add(WIDGET_KIND)?).ok()? == TEXT_KIND {
            let text = m.u32(child).ok()? == TEXT_VTABLE;
            let index = m.u16(child.checked_add(WIDGET_INDEX)?).ok()?;
            match Piece::of_index(index).filter(|_| text) {
                Some(piece) => f.ours.push((piece, child)),
                None => {
                    if first_text && text {
                        f.header = Some(child);
                    }
                    first_text = false;
                    if text
                        && f.count.is_none()
                        && rect_at(m, child).is_some_and(|r| r == COUNT || r == COUNT_NARROW)
                    {
                        f.count = Some(child);
                    }
                }
            }
        }
        child = m.u32(child.checked_add(WIDGET_NEXT)?).ok()?;
    }
    Some(f)
}

/// The pieces `found` lacks for `present`'s tabs: what the update adds.
pub fn missing(found: &Found, present: Present) -> Vec<Piece> {
    pieces(present)
        .into_iter()
        .filter(|p| !found.ours.iter().any(|(q, _)| q == p))
        .collect()
}

// ---- what the strip shows ----

/// The strip for the lobby as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    pub place: Place,
    /// This console leads the party: the glyphs show.
    pub leader: bool,
    pub selected: Tab,
    pub row: Vec<(Tab, Avail)>,
}

impl Shown {
    /// Whether `piece` shows.
    pub fn visible(&self, piece: Piece) -> bool {
        match piece {
            Piece::Cell(tab, l) => self
                .row
                .iter()
                .find(|(t, _)| *t == tab)
                .is_some_and(|&(t, avail)| look(t, self.selected, avail) == l),
            Piece::Glyph(dir, on) => self.leader && glyph_on(&self.row, self.selected, dir) == on,
        }
    }
}

/// The lobby as the strip reads it from guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub place: Place,
    pub leader: bool,
    pub retail: RetailMode,
    /// The lobby's map carries the Firefight kit.
    pub kit_map: bool,
    pub facts: Facts,
}

/// The strip for `reading`, with `mode` the lobby's mode of ours and
/// `clanker` the Clanker playlists' flag.
pub fn shown(reading: &Reading, present: Present, mode: Mode, clanker: bool) -> Shown {
    let mode = seen_mode(reading.leader, mode, reading.kit_map);
    Shown {
        place: reading.place,
        leader: reading.leader,
        selected: Tab::of(reading.retail, mode, clanker),
        row: row(present, reading.place, reading.facts),
    }
}

/// The lobby's place: Split Screen when not networked, else the squad
/// session's class (2 Xbox Live, 1 System Link).
pub fn place(networking: bool, class: Option<u32>) -> Option<Place> {
    match (networking, class) {
        (false, _) => Some(Place::SplitScreen),
        (true, Some(2)) => Some(Place::XboxLive),
        (true, Some(1)) => Some(Place::SystemLink),
        (true, _) => None,
    }
}

/// The retail lobby mode: matchmaking in a matchmaking life cycle (types 6
/// and 7), co-op on a campaign session, else the custom game lobby.
pub fn retail_mode(networking: bool, session_type: u32, campaign: bool) -> RetailMode {
    if networking && lobby::matchmaking_type(session_type) {
        RetailMode::Matchmaking
    } else if campaign {
        RetailMode::CoOp
    } else {
        RetailMode::Custom
    }
}

static LIVE: Mutex<Option<Arc<live_hle::Hle>>> = Mutex::new(None);

/// The Live service the Clanker playlists come from (boot, with
/// Clankermatch on).
pub fn set_live(hle: Option<Arc<live_hle::Hle>>) {
    *LIVE.lock().unwrap_or_else(|e| e.into_inner()) = hle;
}

/// Reads the lobby; `None` while its session cannot be read.
pub fn read(m: &dyn GuestMemory) -> Option<Reading> {
    use bots::phantom::session::at;
    let networking = m.u8(lobby::NETWORKING).ok()? != 0;
    let session = m.u32(lobby::SESSION).ok().filter(|&s| s != 0);
    let class = session.and_then(|s| m.u32(s.checked_add(at::CLASS)?).ok());
    let place = place(networking, class)?;
    let leader = !networking || lobby::facts(m).ok()?.leader;
    let session_type = m.u32(lobby::SESSION_TYPE).ok()?;
    let campaign = crate::coop_probe::read_session(&ReadGm(m)).is_some_and(|s| s.is_campaign());
    let catalogue = crate::map_select::current();
    let map = m.u32(crate::h1_options_row::LOBBY_MAP).ok();
    let kit_map = catalogue.as_ref().zip(map).is_some_and(|(c, id)| {
        c.all()
            .any(|x| x.id == id && crate::lobby_start::is_kit_map(x))
    });
    let kit_maps = catalogue
        .as_ref()
        .is_some_and(|c| !crate::lobby_start::kit_maps(c).is_empty());
    let clanker_playlists = LIVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|h| !h.clanker_hoppers().hoppers.is_empty());
    Some(Reading {
        place,
        leader,
        retail: retail_mode(networking, session_type, campaign),
        kit_map,
        facts: Facts {
            // The playlist list is read with slice T5; until then a Live
            // lobby's MATCHMAKING is taken as having one, and the switch
            // to it is refused by name (`Reason::MatchmakingNotYet`).
            playlists: place == Place::XboxLive,
            clanker_playlists,
            kit_maps,
            remote_members: crate::lobby_mode::remote_members(m) != Some(0),
        },
    })
}

/// Writes what `shown` shows: one look per tab, the glyphs, the header
/// hidden and the player count narrowed.
pub fn show(m: &mut dyn GuestMemory, found: &Found, shown: &Shown) -> Result<(), MemFault> {
    for &(piece, widget) in &found.ours {
        m.set_u8(widget + WIDGET_VISIBLE, u8::from(shown.visible(piece)))?;
    }
    if let Some(header) = found.header {
        m.set_u8(header + WIDGET_VISIBLE, 0)?;
    }
    if let Some(count) = found.count {
        put_rect(m, count, COUNT_NARROW)?;
    }
    Ok(())
}

/// Puts the header and the player count back as retail has them, and
/// hides our widgets: the lobby without the strip.
pub fn put_back(m: &mut dyn GuestMemory, found: &Found) -> Result<(), MemFault> {
    for &(_, widget) in &found.ours {
        m.set_u8(widget + WIDGET_VISIBLE, 0)?;
    }
    let Some(count) = found.count else {
        return Ok(());
    };
    if rect_at(m, count) == Some(COUNT_NARROW) {
        put_rect(m, count, COUNT)?;
        if let Some(header) = found.header {
            m.set_u8(header + WIDGET_VISIBLE, 1)?;
        }
    }
    Ok(())
}

// ---- the update ----

static ON: AtomicBool = AtomicBool::new(false);
/// Where the blocks the widgets are made from are written (an arena page
/// of ours).
static BLOCKS: AtomicU32 = AtomicU32::new(0);
static ADD_FAILED: AtomicBool = AtomicBool::new(false);

/// The strip is installed (the party lobby feature, `crate::lobby_mode`),
/// with `blocks` a page for its text blocks.
pub fn install(blocks: u32) {
    BLOCKS.store(blocks, Ordering::SeqCst);
    ON.store(true, Ordering::SeqCst);
}

/// Whether the strip is installed.
pub fn on() -> bool {
    ON.load(Ordering::SeqCst)
}

static PASSING: AtomicBool = AtomicBool::new(false);

/// The party lobby's own `before` on `LOBBY_VARIANT_PANEL` (EAX the lobby
/// screen, the variant on the stack, `ret 4`), ordered after
/// `lobby-options`'s: it runs retail's panel update, then the strip's
/// ([`update`]). With `lobby-options` on, its answer's call to retail
/// comes through here; with it off, this is the only answer. Either way
/// the strip is drawn and the retail header hidden.
pub fn panel(c: &mut hooks::Call) -> Result<hooks::Flow, runtime::GuestFault> {
    if PASSING.load(Ordering::SeqCst) {
        return Ok(hooks::Flow::Continue);
    }
    let screen = c.cpu.eax;
    let Some(variant) = c.arg_u32("variant") else {
        return Ok(hooks::Flow::Continue);
    };
    c.answer_async(move |cx| -> EntryFuture { Box::pin(panel_entry(cx, screen, variant)) })
}

/// [`panel`]'s answer: retail's panel update for `variant` on `screen`,
/// then the strip's.
pub async fn panel_entry(cx: Cx, screen: u32, variant: u32) -> Result<Ret, EntryError> {
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
    update(&cx, screen).await?;
    Ok(Ret::new(retail.eax, 4))
}

/// The strip's update on the lobby `screen`, after its panel update: adds
/// the widgets the pane lacks, then shows the tabs.
pub async fn update(cx: &Cx, screen: u32) -> Result<(), EntryError> {
    if !on() {
        return Ok(());
    }
    let present = super::present();
    let (mode, clanker) = (crate::lobby_start::mode(), crate::clankermatch::active());
    let Some(state) = cx.read(|m| read(m).map(|r| shown(&r, present, mode, clanker))) else {
        return Ok(());
    };
    let Some(found) = cx.read(|m| find(m, screen)) else {
        return Ok(());
    };
    let need = missing(&found, present);
    let blocks = BLOCKS.load(Ordering::SeqCst);
    let found = if need.is_empty() || blocks == 0 {
        found
    } else {
        add(cx, screen, blocks, &need, present).await?;
        cx.read(|m| find(m, screen)).unwrap_or(found)
    };
    cx.write(|m| show(m, &found, &state))?;
    Ok(())
}

async fn add(
    cx: &Cx,
    screen: u32,
    blocks: u32,
    need: &[Piece],
    present: Present,
) -> Result<(), EntryError> {
    for (k, &piece) in need.iter().enumerate() {
        let Some(bytes) = block(piece, present) else {
            continue;
        };
        let at = blocks + k as u32 * TEXT_BLOCK_SIZE;
        cx.write(|m| m.write(at, &bytes))?;
        let widget = cx
            .call(
                GuestCall::new(points::PANE_TEXT_ADD.base)
                    .ebx(screen)
                    .args([u32::from(piece.index()), at]),
            )
            .await?
            .eax;
        if widget == 0 {
            if !ADD_FAILED.swap(true, Ordering::Relaxed) {
                eprintln!("lobby tabs: no strip widget {piece:?} (no memory for the widget)");
            }
            continue;
        }
        cx.call(
            GuestCall::new(points::TEXT_SET.base)
                .edi(widget)
                .esi(screen)
                .args([piece.id()]),
        )
        .await?;
    }
    Ok(())
}

// ---- the agent API ----

/// `ui.lobby.tabs`: the strip as the lobby on top shows it, or why there
/// is none.
pub fn inspect(m: &dyn GuestMemory) -> Result<Value, &'static str> {
    if !on() {
        return Err("the party lobby's strip is not installed (no settings store)");
    }
    let top = crate::ui_watch::read(&ReadGm(m))
        .top
        .ok_or("no menu screen is on top")?;
    if top.id != crate::lobby_options::SCREEN {
        return Err("the pregame lobby is not the screen on top");
    }
    let reading = read(m).ok_or("the lobby's session cannot be read")?;
    let s = shown(
        &reading,
        super::present(),
        crate::lobby_start::mode(),
        crate::clankermatch::active(),
    );
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
        "tabs": tabs,
        "notice": notice.map(|r| r.text()),
    })
}

#[cfg(test)]
mod tests;
