//! In-game button layouts: Bumper Jumper, Recon and Walkie Talkie as extra
//! rows on Halo 2's own Settings, Controls, Button Layout screen.
//!
//! The design, addresses and what is still unverified are in
//! docs/input/controller-layouts.md. In short: the profile's layout byte selects a
//! 16-slot action table at `0x190c34`; the game treats a byte it does not know
//! as Default. This module (1) rewrites the table after the original runs for
//! bytes 4 to 6, (2) grows the screen's list from four rows to seven and
//! teaches its row text, selection and initial row about the added rows, (3)
//! serves our own English text through the locale lookup, and (4) gives the
//! screen's tag three more panes per thumbstick diagram, cloned from the
//! default one with the labels moved to where the layout puts the actions.
//!
//! Every hook is a wrapper at a function entry. Rows and bytes 0 to 3 go to
//! the generated original untouched. Set `H2_NO_LAYOUTS=1` to skip all of it.
//! The logic is written over the small [`Gm`] trait so it is tested without
//! a guest.
//!
//! The same install also registers the "Controller Speaker" row on the
//! Controls screen (`crate::speaker_row`, `H2_NO_SPEAKER_ROW=1` to skip it):
//! both features grow a list through the same constructor, fill rows of the
//! same Controls row text function and serve text through the same string
//! lookup, and a function entry takes one override.
//!
//! It also registers the "Video" row on the top-level Settings list (after
//! Player Profiles and Game Variants) and the Video screen that row opens
//! (`crate::video_screen`, `H2_NO_VIDEO_SCREEN=1` skips both): a Controls
//! screen whose rows are Field of View (with the first-person camera hook
//! that applies it, `crate::fov_row`; `H2_NO_FOV_ROW=1` skips both), Frame
//! Rate, Resolution, Lighting and Mute Button, through the same entries plus the
//! screen header builder and the Settings list's row text and row chosen.
//!
//! Each hook says once on stderr when it first runs ("layouts: <name>
//! active"), so one run shows which hooks the game reaches.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::custom_games;
use crate::fov_row;
use crate::setting_defs::{FOV, PAD_SPEAKER};
use crate::skulls_menu;
use crate::speaker_row;
use crate::ui::{List, Model, RowKind};
use crate::video_screen;

use input::layouts::{Action, Button, Layout};
use runtime::dispatch::Dispatch;
use runtime::exec::OriginalHandler;
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

// ---- guest addresses (hook points; see docs/input/controller-layouts.md) ----

const ACTION_TABLE_BUILD: u32 = 0x0019_0c34;
const LIST_DATA_NEW: u32 = 0x001a_480d;
const ROW_TEXT: u32 = 0x002c_6512;
const ROW_SELECTED: u32 = 0x002c_6567;
const INITIAL_ROW: u32 = 0x002b_463d;
const PANE_SET: u32 = 0x0023_0427;
const SCREEN_TAG: u32 = 0x0022_f871;
const TEXT_SET: u32 = 0x0025_3b1a;
const PROFILE_ROWS: u32 = 0x0024_a886;
const TEXT_WIDGET_GET: u32 = 0x0022_edb8;
const LIST_SELECT: u32 = 0x0024_c591;
const SCREEN_REFRESH: u32 = 0x0022_f672;

/// Per-player profile settings (`+4` layout byte) and the action tables.
const SETTINGS_BASE: u32 = 0x0054_e9f4;
const SETTINGS_STRIDE: u32 = 0xc70;
const TABLE_BASE: u32 = 0x0051_ea18;
const TABLE_STRIDE: u32 = 0x1c;
const TABLE_SLOTS: u32 = 8;
/// The Controls edit buffer's layout byte.
const EDIT_LAYOUT: u32 = 0x0054_e6d8;
/// Name of the list's item array, which marks it as the layout list.
pub const LIST_NAME: u32 = 0x0045_ce18;
const VANILLA_ROWS: u32 = 4;
const SCREEN_ID: i16 = 40;
const VANILLA_PANES: u32 = 8;
/// The pane count once `extend_panes` grew a tag: the eight, then one clone
/// per added layout for each thumbstick diagram.
const EXTENDED_PANES: u32 = VANILLA_PANES + 2 * ADDED.len() as u32;
/// Where the screen of `INITIAL_ROW` keeps its list handler.
const SCREEN_LIST: u32 = 0x614;
/// Local players: the settings blocks and action tables hold four.
const MAX_PLAYERS: u32 = 4;
const PANE_SIZE: u32 = 0x4c;
const TEXT_SIZE: u32 = 0x2c;
/// Offset of the edit-profile screen's profile copy and the layout byte in it.
const PROFILE_COPY_LAYOUT: u32 = 0x176c + 0x104;

/// Our guest memory: strings, then the new pane tables.
const DATA_BASE: u32 = 0xa2f0_0000;
const DATA_SIZE: u32 = 0x1_0000;
const SHADOW_BASE: u32 = 0xa2f1_0000;
/// The Controls screen's added item widgets (one per added row), one slot
/// per open screen (`speaker_row::extra_slot`).
const EXTRA_BASE: u32 = 0xa2f2_0000;
const EXTRA_SIZE: u32 = 0x1000;

pub const ADDED: [Layout; 3] = [Layout::BumperJumper, Layout::Recon, Layout::WalkieTalkie];

// ---- guest memory seen by the logic ----

pub trait Gm {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool;
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool;
    fn u8(&self, a: u32) -> Option<u8> {
        let mut b = [0; 1];
        self.rd(a, &mut b).then_some(b[0])
    }
    fn u16(&self, a: u32) -> Option<u16> {
        let mut b = [0; 2];
        self.rd(a, &mut b).then_some(u16::from_le_bytes(b))
    }
    fn u32(&self, a: u32) -> Option<u32> {
        let mut b = [0; 4];
        self.rd(a, &mut b).then_some(u32::from_le_bytes(b))
    }
    fn set_u16(&mut self, a: u32, v: u16) -> bool {
        self.wr(a, &v.to_le_bytes())
    }
    fn set_u32(&mut self, a: u32, v: u32) -> bool {
        self.wr(a, &v.to_le_bytes())
    }
}

impl Gm for Guest {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.read_bytes(addr, out, 1).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.write_bytes(addr, data, 1).is_ok()
    }
}

/// An entry's guest memory (`Cx::read`), read-only, for readers written
/// over [`Gm`].
pub struct ReadGm<'a>(pub &'a dyn engine::memory::GuestMemory);

impl Gm for ReadGm<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, _: u32, _: &[u8]) -> bool {
        false
    }
}

// ---- pure logic ----

/// The layout for a row of the list, if it is one of ours.
pub fn layout_of_row(row: i16) -> Option<Layout> {
    let i = i32::from(row) - VANILLA_ROWS as i32;
    usize::try_from(i).ok().and_then(|i| ADDED.get(i)).copied()
}

/// The list's item count: ours for the layout list, unchanged otherwise.
pub fn list_count(name: u32, count: u32) -> u32 {
    if name == LIST_NAME && count == VANILLA_ROWS {
        count + ADDED.len() as u32
    } else {
        count
    }
}

/// The datum array constructor (`0x1a480d`) with the stack at `esp` and the
/// count in `eax`: the count to build with. The constructor takes the array
/// name at `[esp+4]` and the element size at `[esp+8]`; the count is EAX
/// (it reaches `0x16b570` in EAX, which multiplies it by the element size).
/// Both list screens push element size 4; the layout list's count happens to
/// be 4 as well, which is how an earlier version came to grow the element
/// size instead and left the list at four rows.
pub fn grown_count<M: Gm>(m: &M, esp: u32, eax: u32, layouts: bool, speaker: bool) -> u32 {
    let Some(name) = m.u32(esp.wrapping_add(4)) else {
        return eax;
    };
    let mut n = eax;
    if layouts {
        n = list_count(name, n);
    }
    if speaker {
        n = speaker_row::small_list_count(name, n);
    }
    n
}

/// The row of a list's row-chosen handler (thiscall, `ret 8`): a pointer to
/// the row's word is the second argument, `[esp+8]`. Both the layout list
/// (`0x2c6567`) and the Controls list (`0x2c8719`) read it there.
pub fn chosen_row<M: Gm>(m: &M, esp: u32) -> Option<i16> {
    let p = m.u32(esp.wrapping_add(8))?;
    m.u16(p).map(|v| v as i16)
}

/// After the original built player `player`'s action table: overwrite it for
/// an added layout. Retail bytes (and bytes nothing knows) are left alone,
/// and so is anything for a player index past the four local players (EAX
/// is not checked by the caller's contract we know of, and a stray index
/// would write far outside the tables).
pub fn apply_action_table<M: Gm>(m: &mut M, player: u32) -> bool {
    if player >= MAX_PLAYERS {
        return false;
    }
    let Some(byte) = m.u8(SETTINGS_BASE + SETTINGS_STRIDE.wrapping_mul(player) + 4) else {
        return false;
    };
    match Layout::from_preset(byte) {
        Some(l) if !l.is_retail() => m.wr(
            TABLE_BASE + TABLE_STRIDE.wrapping_mul(player) + TABLE_SLOTS,
            &l.action_map(),
        ),
        _ => false,
    }
}

/// The pane to show for list row `row` when the screen's pane base is `base`
/// (0, or 4 for the Legacy thumbstick diagram) and the screen's tag has
/// `panes` panes: the row value to hand the game so that `base + value`
/// lands on our pane. `None` leaves it alone, and always does unless the tag
/// was extended to [`EXTENDED_PANES`]: a tag we did not extend (the in-game
/// Button Layout screen's, say) has no pane 8 to 13 to land on.
pub fn pane_row(row: i16, base: i8, panes: u32) -> Option<i16> {
    if panes != EXTENDED_PANES {
        return None;
    }
    let l = layout_of_row(row)?;
    let slot = i16::from(l.preset()) - 4;
    let group = i16::from(base != 0);
    Some(VANILLA_PANES as i16 + slot + ADDED.len() as i16 * group - i16::from(base))
}

/// The number of rows a list handler's datum array holds (`[handler+0x70]`,
/// the count at `+0x20`, as the screen constructors loop over it).
pub fn list_rows<M: Gm>(m: &M, handler: u32) -> Option<u32> {
    let array = m.u32(handler.wrapping_add(0x70))?;
    let n = m.u32(array.wrapping_add(0x20))? as i32;
    u32::try_from(n).ok()
}

/// Whether `row` is a row the list `handler` really has. The row hooks act
/// on an added row only when it is: a list that did not grow (another
/// screen's, or the hook skipped) keeps its retail behaviour.
pub fn row_listed<M: Gm>(m: &M, handler: u32, row: i16) -> bool {
    match (u32::try_from(row), list_rows(m, handler)) {
        (Ok(r), Some(n)) => r < n,
        _ => false,
    }
}

/// Our strings. Ids live in an index range no real string uses.
pub const ID_BASE: u32 = crate::ui_kit::ids::TABLE;
pub fn is_ours(id: u32) -> bool {
    id & 0x00ff_ff00 == 0x00ff_f000
}
pub fn name_id(l: Layout) -> u32 {
    ID_BASE + (u32::from(l.preset()) - 4)
}
pub fn help_id(l: Layout) -> u32 {
    ID_BASE + 0x10 + (u32::from(l.preset()) - 4)
}
pub fn name_text(l: Layout) -> &'static str {
    match l {
        Layout::BumperJumper => "Bumper Jumper",
        Layout::Recon => "Recon",
        _ => "Walkie Talkie",
    }
}
pub fn help_text(l: Layout) -> &'static str {
    match l {
        Layout::BumperJumper => {
            "Jump on the left bumper, melee on the right. Grenades on A, reload on B, flashlight on X."
        }
        Layout::Recon => "Reload and action on the right bumper. Switch grenades moves to X.",
        _ => "Melee on the right bumper, reload on B, switch grenades on X.",
    }
}

/// The layout strings, as `(id, text)`.
pub fn strings() -> Vec<(u32, &'static str)> {
    ADDED
        .iter()
        .flat_map(|l| [(name_id(*l), name_text(*l)), (help_id(*l), help_text(*l))])
        .collect()
}

/// Every string served with these features on.
pub fn served_strings(layouts: bool, speaker: bool) -> Vec<(u32, &'static str)> {
    let mut v = if layouts { strings() } else { Vec::new() };
    if speaker {
        v.push((speaker_row::SMALL_ON_ID, speaker_row::SMALL_ON_TEXT));
        v.push((speaker_row::SMALL_OFF_ID, speaker_row::SMALL_OFF_TEXT));
    }
    v
}

/// Write `strings` (NUL-terminated UTF-8, as the locale blob stores them)
/// at `base`; returns the address of each id and the first free address.
pub fn write_strings<M: Gm>(
    m: &mut M,
    base: u32,
    strings: &[(u32, &str)],
) -> Option<(Vec<(u32, u32)>, u32)> {
    let mut at = base;
    let mut out = Vec::new();
    for (id, text) in strings {
        let mut b = text.as_bytes().to_vec();
        b.push(0);
        if !m.wr(at, &b) {
            return None;
        }
        out.push((*id, at));
        at += b.len() as u32;
    }
    Some((out, at))
}

/// Text block order of the default pane: the help text, then twelve labels.
const PANE_TEXT_ORDER: [Option<Action>; 13] = [
    None,
    Some(Action::Grenade),
    Some(Action::Crouch),
    Some(Action::Score),
    Some(Action::Pause),
    Some(Action::Fire),
    Some(Action::ActionReload),
    Some(Action::SwitchWeapon),
    Some(Action::Melee),
    Some(Action::Jump),
    Some(Action::SwitchGrenades),
    Some(Action::Flashlight),
    Some(Action::Zoom),
];

/// The label a diagram slot shows. A slot sits at one physical button. In
/// the default layout that button drives `slot_action`; in `l` it drives
/// whichever action `l` puts there.
pub fn relabel(l: Layout, slot_action: Action) -> Action {
    let button: Button = Layout::Default.button(slot_action);
    Action::GAMEPLAY
        .into_iter()
        .find(|a| l.button(*a) == button)
        .unwrap_or(slot_action)
}

fn text_block<M: Gm>(m: &M, pane: u32) -> Option<(u32, u32)> {
    Some((m.u32(pane + 0x1c)?, m.u32(pane + 0x20)?))
}

/// Grow the tag's pane array from 8 to 14: for each thumbstick diagram (pane
/// 0 and pane 4), one clone per added layout with its labels moved and its
/// help text replaced. `alloc` is where the new tables go. Does nothing (and
/// reports `Ok(false)`) unless the tag has exactly the retail 8 panes, so it
/// is safe to call again.
pub fn extend_panes<M: Gm>(m: &mut M, wgit: u32, alloc: &mut u32) -> Option<bool> {
    if m.u16(wgit + 4)? as i16 != SCREEN_ID || m.u32(wgit + 0x20)? != VANILLA_PANES {
        return Some(false);
    }
    let old = m.u32(wgit + 0x24)?;
    // Label sids by action, from the default pane's fixed text order.
    let (n, t0) = text_block(m, old)?;
    if n as usize != PANE_TEXT_ORDER.len() {
        return Some(false);
    }
    let mut label = std::collections::HashMap::new();
    for (i, a) in PANE_TEXT_ORDER.iter().enumerate() {
        if let Some(a) = a {
            label.insert(*a, m.u32(t0 + i as u32 * TEXT_SIZE + 0x24)?);
        }
    }
    let sid_action: std::collections::HashMap<u32, Action> =
        label.iter().map(|(a, s)| (*s, *a)).collect();

    let total = VANILLA_PANES + 2 * ADDED.len() as u32;
    let panes = *alloc;
    *alloc += total * PANE_SIZE;
    let mut buf = vec![0; (VANILLA_PANES * PANE_SIZE) as usize];
    if !m.rd(old, &mut buf) || !m.wr(panes, &buf) {
        return None;
    }
    for group in 0..2u32 {
        let template = old + group * 4 * PANE_SIZE;
        let (cnt, texts) = text_block(m, template)?;
        if cnt as usize != PANE_TEXT_ORDER.len() {
            return Some(false);
        }
        let mut pane = vec![0; PANE_SIZE as usize];
        if !m.rd(template, &mut pane) {
            return None;
        }
        let mut tbuf = vec![0; (cnt * TEXT_SIZE) as usize];
        if !m.rd(texts, &mut tbuf) {
            return None;
        }
        for (k, l) in ADDED.iter().enumerate() {
            let new_texts = *alloc;
            *alloc += cnt * TEXT_SIZE;
            let mut t = tbuf.clone();
            for i in 0..cnt as usize {
                let at = i * TEXT_SIZE as usize + 0x24;
                let sid = u32::from_le_bytes(t[at..at + 4].try_into().ok()?);
                let new = if i == 0 {
                    help_id(*l)
                } else {
                    match sid_action.get(&sid) {
                        Some(a) => *label.get(&relabel(*l, *a)).unwrap_or(&sid),
                        None => sid,
                    }
                };
                t[at..at + 4].copy_from_slice(&new.to_le_bytes());
            }
            if !m.wr(new_texts, &t) {
                return None;
            }
            let mut p = pane.clone();
            p[0x20..0x24].copy_from_slice(&new_texts.to_le_bytes());
            let index = VANILLA_PANES + group * ADDED.len() as u32 + k as u32;
            if !m.wr(panes + index * PANE_SIZE, &p) {
                return None;
            }
        }
    }
    // Publish last: a reader sees the old array or the complete new one.
    (m.set_u32(wgit + 0x24, panes) && m.set_u32(wgit + 0x20, total)).then_some(true)
}

// ---- hooks ----

/// 0 or the address of the layout byte the next empty value text stands for.
static PENDING_VALUE: AtomicU32 = AtomicU32::new(0);
static ALLOC: AtomicU32 = AtomicU32::new(0);
static STRING_ADDRS: std::sync::OnceLock<Vec<(u32, u32)>> = std::sync::OnceLock::new();

/// Where our string `id` was written at install, once it was: a screen
/// whose text changes while it is up rewrites it there
/// (`list_class::Rows::live_text`), within the room its placeholder took.
pub fn string_address(id: u32) -> Option<u32> {
    STRING_ADDRS
        .get()?
        .iter()
        .find(|(i, _)| *i == id)
        .map(|(_, a)| *a)
}
/// The Xbox Live menu tag `custom_games::Menu::extend_tag` last extended (0:
/// none yet). A map load replaces the tag, and the next screen build
/// extends the new one before a pane is set.
static LIVE_TAG: AtomicU32 = AtomicU32::new(0);
/// Live menu item widgets of ours: one set per open screen, this many
/// slots, each of up to [`LIVE_MAX_EXTRA`] items.
const LIVE_ITEM_SLOTS: u32 = 8;
const LIVE_MAX_EXTRA: u32 = custom_games::MAX_EXTRA_ITEMS;

/// In place of a row-chosen call (`ret 8`): open the screen of `loader`
/// for `mask` through `0x149f1e`, then return to the caller.
fn open_screen(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mask: u32,
    loader: u32,
    note: &'static str,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    cpu.esp = cpu.esp.wrapping_add(12); // return address and the two arguments
    push(&mut mem, cpu, loader)?;
    push(&mut mem, cpu, mask)?;
    eprintln!("{note}");
    ctx.call_guest(cpu, &mut mem, video_screen::OPEN_SCREEN, move |_, _| {
        Ok(TransferAbi::ret(ret_pc))
    })
}

/// In place of a row-chosen call (`ret 8`): show the game's one-button
/// dialog for `error` to the players in `mask`, as the game shows its own
/// (`0x19b527(1, error, 4, mask, 0, 0)`: no press or update callback, so the
/// button just closes it), then return to the caller.
fn open_dialog(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mask: u32,
    error: u32,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    cpu.esp = cpu.esp.wrapping_add(12); // return address and the two arguments
    for v in [0, 0, mask, 4, error, 1] {
        push(&mut mem, cpu, v)?;
    }
    ctx.call_guest(cpu, &mut mem, custom_games::OK_DIALOG, move |_, _| {
        Ok(TransferAbi::ret(ret_pc))
    })
}

fn original(rt: &runtime::Runtime, addr: u32) -> Option<Arc<OriginalHandler>> {
    let o = rt.resolve_original(addr);
    if o.is_none() {
        eprintln!("layouts: no generated entry at {addr:#x}; hook skipped");
    }
    o.map(Arc::new)
}

/// Leaves the Xbox Live menu's cursor on `row` for the way back
/// (`custom_games::remember_row`).
fn remember(ctx: &mut ExecCtx, row: i16) {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    if !custom_games::remember_row(&mut *mem, row) {
        eprintln!("custom games: the live menu's cursor row could not be set");
    }
}

/// Run the original at `addr` through `shadow`, then `then` with the caller's
/// return address. The wrapper pops the return address first so the original
/// sees its own arguments.
fn call_then(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    shadow: u32,
    then: impl FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<(), GuestFault> + Send + 'static,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    cpu.esp = cpu.esp.wrapping_add(4);
    ctx.call_guest(cpu, &mut mem, shadow, move |cpu, ctx| {
        then(cpu, ctx)?;
        Ok(TransferAbi::ret(ret_pc))
    })
}

fn push(mem: &mut Guest, cpu: &mut CpuAbi, v: u32) -> Result<(), GuestFault> {
    cpu.esp = cpu.esp.wrapping_sub(4);
    mem.set_u32(cpu.esp, v)
}

/// Says `<name> active` on stderr the first time it is called with `seen`.
fn announce(seen: &AtomicBool, name: &str) {
    if !seen.load(Ordering::Relaxed) && !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{name} active");
    }
}

/// Install every hook: the button layouts unless `H2_NO_LAYOUTS` is set,
/// and the rows of `model` (`crate::video_screen::model`): the Controller
/// Speaker row on Controls, the Video row on Settings and the screen it opens,
/// and the camera hook when the Video screen has the Field of View row.
/// Every row's count, text and press come from the model. A missing
/// original (an image with other entries) skips that hook and says so.
pub fn install(
    rt: &runtime::Runtime,
    registry: &mut feature::Registry,
    model: Option<Arc<Model>>,
    theater: Option<crate::theater_ui::Theater>,
) -> anyhow::Result<()> {
    let layouts = std::env::var_os("H2_NO_LAYOUTS").is_none();
    let has = |list, kind| {
        model
            .as_ref()
            .is_some_and(|m| m.index_of(list, &kind).is_some())
    };
    let with_speaker = has(List::Controls, RowKind::Setting(PAD_SPEAKER.key));
    // Any row of ours on the Controls list (the speaker's, the Steam
    // Controller's): its text and press hooks.
    let with_controls = model.as_ref().is_some_and(|m| {
        m.count(List::Controls, speaker_row::VANILLA_ROWS) > speaker_row::VANILLA_ROWS
    });
    let with_fov = has(video_screen::LIST, RowKind::Setting(FOV.key));
    let with_video = has(List::Settings, RowKind::Open(video_screen::SCREEN));
    // The Voice screen, and our rows on the pause menus (crate::pause_menu).
    let with_voice = model
        .as_ref()
        .is_some_and(|m| !m.rows(video_screen::VOICE_LIST).is_empty());
    let with_pause = model
        .as_ref()
        .is_some_and(|m| !m.rows(List::GameSettings).is_empty());
    let with_custom_games = custom_games::row_enabled();
    let with_theater = theater.is_some();
    let with_live_menu = with_custom_games || with_theater;
    let live_menu = move || custom_games::Menu {
        custom_games: with_custom_games,
        coop: with_custom_games && custom_games::coop_row_enabled(),
        theater: with_theater,
    };
    let with_skulls = model
        .as_ref()
        .is_some_and(|m| skulls_menu::enabled(m.store()));
    // The campaign picker (docs/halo1/campaign-picker.md), when a second
    // campaign is installed.
    let with_campaigns = crate::campaign_menu::class().is_some();
    // The Halo 1 options' texts (crate::h1_options): the campaign page's
    // option rows and the Game Setup row and screen, wherever settings are.
    let with_h1_options = model.is_some();
    if !layouts
        && !with_speaker
        && !with_controls
        && !with_video
        && !with_voice
        && !with_pause
        && !with_live_menu
        && !with_skulls
        && !with_campaigns
        && !with_h1_options
    {
        return Ok(());
    }
    let model = model.unwrap_or_else(|| {
        let store = Arc::new(settings::Store::defaults(&[]));
        Arc::new(Model::new(store, Vec::new(), Vec::new(), 0..0).expect("an empty model"))
    });
    let store = Arc::clone(model.store());
    let speaker_on = with_speaker.then(|| store.handle::<bool>(PAD_SPEAKER.key));
    // Our rows at the end of the pause menu's Settings (crate::pause_menu),
    // fixed now: the hooks run with guest memory held.
    let pause_rows = if with_pause { crate::pause_menu::settings_rows(&model) } else { Vec::new() };
    // The Settings dialog's hooks serve the speaker row, ours, or both.
    let with_small = with_speaker || !pause_rows.is_empty();
    let fov = with_fov.then(|| store.handle::<u32>(FOV.key));
    let extra_rows =
        model.count(List::Controls, speaker_row::VANILLA_ROWS) - speaker_row::VANILLA_ROWS;
    // The Live menu's (tag copies, item widgets, pane word) in our data.
    let mut live = (0u32, 0u32, 0u32);
    {
        let mut mem = rt.memory();
        mem.map_zeroed(DATA_BASE, DATA_SIZE, Permissions::RW)?;
        mem.map_zeroed(SHADOW_BASE, 0x1000, Permissions::RX)?;
        if extra_rows > 0 {
            mem.map_zeroed(EXTRA_BASE, EXTRA_SIZE, Permissions::RW)?;
        }
        let mut served = served_strings(layouts, with_speaker);
        served.extend(model.strings().iter().map(|(i, t)| (*i, t.as_str())));
        if with_live_menu {
            served.extend(custom_games::strings());
        }
        if with_skulls {
            served.extend(skulls_menu::strings());
        }
        if with_h1_options {
            served.extend(crate::h1_options::strings());
        }
        served.extend(crate::campaign_menu::strings());
        let variant_page = crate::variant_page::strings();
        served.extend(variant_page.iter().map(|(i, t)| (*i, t.as_str())));
        let (addrs, used) = write_strings(&mut *mem, DATA_BASE, &served)
            .ok_or_else(|| anyhow::anyhow!("layouts: strings"))?;
        let mut alloc = (used + 0xff) & !0xff;
        if with_live_menu {
            // The Live menu's tag copies, item widgets and pane word: fixed
            // places, reused each time a map load brings the tag back.
            live = (
                alloc,
                alloc + custom_games::TAG_SPACE,
                alloc
                    + custom_games::TAG_SPACE
                    + LIVE_ITEM_SLOTS * LIVE_MAX_EXTRA * speaker_row::ITEM_SIZE,
            );
            alloc = live.2 + 0x100;
        }
        ALLOC.store(alloc, Ordering::SeqCst);
        let _ = STRING_ADDRS.set(addrs);
    }
    let mut shadow = SHADOW_BASE;
    let mut next_shadow = || {
        shadow += 4;
        shadow - 4
    };
    let layout_hooks = [
        ACTION_TABLE_BUILD,
        ROW_TEXT,
        ROW_SELECTED,
        INITIAL_ROW,
        PANE_SET,
        PROFILE_ROWS,
    ];
    let shared_hooks = [LIST_DATA_NEW, SCREEN_TAG];
    let text_hooks = [TEXT_SET];
    let video_hooks = [PANE_SET];
    let speaker_hooks = [
        speaker_row::SMALL_CTOR,
        speaker_row::SMALL_ROW_TEXT,
        speaker_row::SMALL_ROW_CHOSEN,
    ];
    let fov_hooks = [fov_row::PLAYER_FOV];
    let controls_hooks = [speaker_row::ADD_ITEMS];
    let live_menu_hooks = [
        custom_games::ROW_TEXT,
        custom_games::ROW_CHOSEN,
        custom_games::LIST_LAYOUT,
        PANE_SET,
    ];
    let custom_games_hooks = [
        custom_games::ERROR_LOOKUP,
        custom_games::BROWSER_EVENT,
        custom_games::BROWSER_BACK_LOADER,
        custom_games::LIVE_MENU_LOADER,
    ];
    let skulls_hooks = [PANE_SET];
    // Resolve every original first: `resolve_original` takes the dispatch read
    // lock, which `edit_dispatch` holds for writing, so doing it inside the
    // closure deadlocks.
    let wanted: Vec<u32> = shared_hooks
        .into_iter()
        .chain(layout_hooks.into_iter().filter(|_| layouts))
        .chain(speaker_hooks.into_iter().filter(|_| with_small))
        .chain(fov_hooks.into_iter().filter(|_| with_fov))
        .chain(video_hooks.into_iter().filter(|_| with_video))
        .chain(text_hooks.into_iter().filter(|_| layouts))
        // Also for our classes' taller lists (`list_class::extra_items`).
        .chain(controls_hooks)
        .chain(live_menu_hooks.into_iter().filter(|_| with_live_menu))
        .chain(custom_games_hooks.into_iter().filter(|_| with_custom_games))
        .chain(skulls_hooks.into_iter().filter(|_| with_skulls))
        .collect();
    let mut originals: std::collections::HashMap<u32, Arc<OriginalHandler>> = wanted
        .into_iter()
        .filter_map(|a| original(rt, a).map(|o| (a, o)))
        .collect();
    rt.edit_dispatch(|d| {
        // Layout table: original, then ours for bytes 4 to 6.
        if let Some(o) = originals.remove(&ACTION_TABLE_BUILD) {
            let sh = alias(d, &mut next_shadow, &o, ACTION_TABLE_BUILD);
            let seen = AtomicBool::new(false);
            d.register_override(ACTION_TABLE_BUILD, "layouts:table", move |cpu, ctx, _| {
                announce(&seen, "layouts: action table");
                let player = cpu.eax;
                call_then(cpu, ctx, sh, move |_, ctx| {
                    let mut mem = ctx.runtime().memory();
                    apply_action_table(&mut *mem, player);
                    Ok(())
                })
            });
        }
        // List size: the count is EAX (see `grown_count`).
        if let Some(o) = originals.remove(&LIST_DATA_NEW) {
            let seen = AtomicBool::new(false);
            let grew_layouts = AtomicBool::new(false);
            let grew_controls = AtomicBool::new(false);
            let model = Arc::clone(&model);
            let grow_pause = !pause_rows.is_empty();
            d.register_override(LIST_DATA_NEW, "layouts:list", move |cpu, ctx, _| {
                announce(&seen, "layouts: list constructor");
                let (n, name) = {
                    let mem = ctx.runtime().memory();
                    (grown_count(&*mem, cpu.esp, cpu.eax, layouts, with_speaker), mem.u32(cpu.esp + 4).unwrap_or(0))
                };
                let n = video_screen::list_count(&model, name, n);
                if with_video && video_screen::settings_third_item(name, n) {
                    // Slot 19 reports a third item widget for our row.
                    ctx.runtime().memory().set_u8(cpu.esi + video_screen::SETTINGS_THIRD_ITEM, 1)?;
                }
                let n = live_menu().count(name, n);
                // The pause menu's Settings gets our rows when its hooks
                // are on.
                let n = if grow_pause { crate::pause_menu::list_count(&model, name, n) } else { n };
                // The variant editor's categories get the multiplayer
                // Skulls row and the More Options row.
                let n = crate::variant_page::variant_count(
                    name,
                    n,
                    with_skulls && crate::skulls_mp_row::row_on(),
                    crate::variant_page::row_on(),
                );
                // One of our list class screens being built: its own count.
                let n = crate::list_class::array_count(name, n);
                if n != cpu.eax {
                    let flag = if name == LIST_NAME { &grew_layouts } else { &grew_controls };
                    if !flag.swap(true, Ordering::Relaxed) {
                        eprintln!("layouts: list {name:#x} grows from {} to {n} rows", cpu.eax);
                    }
                    cpu.eax = n;
                }
                Ok(o.invoke(cpu, ctx, LIST_DATA_NEW))
            });
        }
        // Row text.
        if let Some(o) = originals.remove(&ROW_TEXT) {
            let seen = AtomicBool::new(false);
            d.register_override(ROW_TEXT, "layouts:row-text", move |cpu, ctx, _| {
                announce(&seen, "layouts: row text");
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let widget = mem.u32(cpu.esp + 4)?;
                let row = mem.u16(widget + 0x70).map_or(0, |v| v as i16);
                let Some(l) = layout_of_row(row).filter(|_| row_listed(&*mem, cpu.ecx, row)) else {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, ROW_TEXT));
                };
                row_texts(cpu, ctx, &mut mem, widget, vec![(0, name_id(l))])
            });
        }
        // Row chosen.
        if let Some(o) = originals.remove(&ROW_SELECTED) {
            let sh = alias(d, &mut next_shadow, &o, ROW_SELECTED);
            let seen = AtomicBool::new(false);
            d.register_override(ROW_SELECTED, "layouts:row-selected", move |cpu, ctx, _| {
                announce(&seen, "layouts: row chosen");
                let (row, listed) = {
                    let mem = ctx.runtime().memory();
                    let row = chosen_row(&*mem, cpu.esp).unwrap_or(0);
                    (row, row_listed(&*mem, cpu.ecx, row))
                };
                call_then(cpu, ctx, sh, move |_, ctx| {
                    if let Some(l) = layout_of_row(row).filter(|_| listed) {
                        ctx.runtime().memory().set_u8(EDIT_LAYOUT, l.preset())?;
                    }
                    Ok(())
                })
            });
        }
        // Initial row.
        if let Some(o) = originals.remove(&INITIAL_ROW) {
            let seen = AtomicBool::new(false);
            d.register_override(INITIAL_ROW, "layouts:initial-row", move |cpu, ctx, _| {
                announce(&seen, "layouts: initial row");
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let byte = mem.u8(EDIT_LAYOUT)?;
                let this = cpu.ecx;
                let Some(l) = Layout::from_preset(byte)
                    .filter(|l| !l.is_retail() && row_listed(&*mem, this + SCREEN_LIST, i16::from(l.preset())))
                else {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, INITIAL_ROW));
                };
                cpu.eax = this + SCREEN_LIST;
                push(&mut mem, cpu, u32::from(l.preset()))?;
                ctx.call_guest(cpu, &mut mem, LIST_SELECT, move |cpu, _| {
                    cpu.ecx = this;
                    Ok(TransferAbi::jump(SCREEN_REFRESH))
                })
            });
        }
        // Pane for a row: ours only on a tag `extend_panes` grew. The tag
        // comes from the screen-tag function, ours included, so a first
        // visit extends it before the pane is set.
        if let Some(o) = originals.remove(&PANE_SET) {
            let sh = alias(d, &mut next_shadow, &o, PANE_SET);
            let seen = AtomicBool::new(false);
            let declined = Arc::new(AtomicBool::new(false));
            d.register_override(PANE_SET, "layouts:pane", move |cpu, ctx, _| {
                announce(&seen, "layouts: pane");
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let screen = cpu.ecx;
                let id = mem.u16(screen + 0x70)? as i16;
                let row = mem.u16(mem.u32(cpu.esp + 4)?)? as i16;
                // The Skulls row in the variant editor has no pane: keep
                // the one shown (thiscall, `ret 4`).
                if with_skulls && crate::skulls_mp::skip_pane(id, row) {
                    return mem.ret(cpu, 4);
                }
                // Nor has the More Options row.
                if crate::variant_page::skip_pane(id, row) {
                    return mem.ret(cpu, 4);
                }
                // Our Video row on the Settings list has none either.
                if with_video && video_screen::skip_settings_pane(id, row) {
                    return mem.ret(cpu, 4);
                }
                // The Xbox Live menu: our row shows our pane, Content
                // Download (moved down one) its own. The row is handed over
                // in our word, not written back through the caller's pointer.
                if with_live_menu && id == custom_games::SCREEN_ID {
                    let tag = LIVE_TAG.load(Ordering::SeqCst);
                    let panes = if tag != 0 && mem.u32(tag + 0x24).ok() == Some(live.0) {
                        mem.u32(tag + 0x20).unwrap_or(0)
                    } else {
                        0
                    };
                    match live_menu().pane_of(row, panes) {
                        custom_games::Pane::Retail => {}
                        custom_games::Pane::Keep => return mem.ret(cpu, 4),
                        custom_games::Pane::Show(p) => {
                            let base = i16::from(mem.u8(screen + 0x5f3)? as i8);
                            mem.set_u16(live.2, p.wrapping_sub(base) as u16)?;
                            mem.set_u32(cpu.esp + 4, live.2)?;
                        }
                    }
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, PANE_SET));
                }
                let added = layouts && id == SCREEN_ID && layout_of_row(row).is_some();
                if !added {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, PANE_SET));
                }
                let eax = cpu.eax;
                cpu.eax = screen;
                let declined = Arc::clone(&declined);
                ctx.call_guest(cpu, &mut mem, SCREEN_TAG, move |cpu, ctx| {
                    let tag = cpu.eax;
                    {
                        let mut mem = ctx.runtime().memory();
                        let panes = if tag == 0 { 0 } else { mem.u32(tag + 0x20).unwrap_or(0) };
                        let p = mem.u32(cpu.esp + 4)?;
                        let row = mem.u16(p)? as i16;
                        let base = mem.u8(screen + 0x5f3)? as i8;
                        match pane_row(row, base, panes) {
                            Some(r) => mem.set_u16(p, r as u16)?,
                            None if !declined.swap(true, Ordering::Relaxed) => {
                                eprintln!("layouts: pane for row {row} left retail (tag has {panes} panes)")
                            }
                            None => {}
                        }
                    }
                    (cpu.ecx, cpu.eax) = (screen, eax);
                    Ok(TransferAbi::jump(sh))
                })
            });
        }
        // The screen's tag: the Button Layout screen gets its added panes
        // once, the Controls screen its seventh visible row.
        if let Some(o) = originals.remove(&SCREEN_TAG) {
            let sh = alias(d, &mut next_shadow, &o, SCREEN_TAG);
            let o2 = Arc::clone(&o);
            let seen = AtomicBool::new(false);
            let grown = Arc::new(AtomicBool::new(false));
            let odd = Arc::new(AtomicBool::new(false));
            let copied = Arc::new(AtomicBool::new(false));
            d.register_override(SCREEN_TAG, "layouts:panes", move |cpu, ctx, _| {
                announce(&seen, "layouts: screen tag");
                let screen = cpu.eax;
                let (id, ours) = {
                    let mem = ctx.runtime().memory();
                    (
                        mem.u16(screen + 0x70).map_or(-1, |v| v as i16),
                        mem.u32(screen).is_ok_and(crate::list_class::has_layout),
                    )
                };
                let wanted = (layouts && id == SCREEN_ID)
                    || (extra_rows > 0 && id == speaker_row::SCREEN_ID)
                    || (with_live_menu && id == custom_games::SCREEN_ID)
                    || ours;
                if !wanted {
                    return Ok(o2.invoke(cpu, ctx, SCREEN_TAG));
                }
                let grown = Arc::clone(&grown);
                let odd = Arc::clone(&odd);
                let copied = Arc::clone(&copied);
                call_then(cpu, ctx, sh, move |cpu, ctx| {
                    // A dialog class of ours is served its copy even when
                    // the lookup found nothing (in a game, where the
                    // Controls tag isn't).
                    if cpu.eax == 0 && !ours {
                        return Ok(());
                    }
                    let mut mem = ctx.runtime().memory();
                    // A screen of our list class with a layout of its own
                    // (the Skulls screens): our copy of its tag.
                    if ours {
                        match crate::list_class::served_tag(&mut *mem, screen, cpu.eax) {
                            Ok(Some(tag)) => {
                                if !copied.swap(true, Ordering::Relaxed) {
                                    eprintln!("list class: a screen of ours gets its own layout (tag copy at {tag:#x})");
                                }
                                cpu.eax = tag;
                            }
                            Ok(None) => {}
                            Err(e) => eprintln!("list class: no layout copy ({e:?}); the retail layout stays"),
                        }
                        return Ok(());
                    }
                    if id == custom_games::SCREEN_ID {
                        // Ours already (our pane array) or freshly extended.
                        if mem.u32(cpu.eax + 0x24).ok() == Some(live.0) {
                            return Ok(());
                        }
                        let menu = live_menu();
                        match menu.extend_tag(&mut *mem, cpu.eax, live.0) {
                            Some(true) => {
                                LIVE_TAG.store(cpu.eax, Ordering::SeqCst);
                                eprintln!("custom games: xbox live menu tag has {} panes and shows {} rows", menu.rows(), menu.visible());
                            }
                            Some(false) => eprintln!("custom games: the xbox live menu tag is not the retail one; left alone"),
                            None => eprintln!("custom games: could not extend the xbox live menu tag"),
                        }
                        return Ok(());
                    }
                    if id == speaker_row::SCREEN_ID {
                        match speaker_row::grow_visible(&mut *mem, cpu.eax, extra_rows) {
                            Some(0) => {}
                            Some(_) if !grown.swap(true, Ordering::Relaxed) => {
                                eprintln!("speaker row: controls list shows {} rows", speaker_row::VANILLA_ROWS + extra_rows)
                            }
                            Some(_) => {}
                            None => eprintln!("speaker row: could not grow the controls list"),
                        }
                        return Ok(());
                    }
                    let mut alloc = ALLOC.load(Ordering::SeqCst);
                    match extend_panes(&mut *mem, cpu.eax, &mut alloc) {
                        Some(true) => {
                            ALLOC.store(alloc, Ordering::SeqCst);
                            eprintln!("layouts: button layout panes extended");
                        }
                        Some(false) => {
                            let panes = mem.u32(cpu.eax + 0x20).unwrap_or(0);
                            if panes != EXTENDED_PANES && !odd.swap(true, Ordering::Relaxed) {
                                eprintln!("layouts: a button layout tag has {panes} panes, not extended; its added rows keep the retail panes");
                            }
                        }
                        None => eprintln!("layouts: could not extend the layout panes"),
                    }
                    Ok(())
                })
            });
        }
        // Value text on the Controls list and the Edit Profile screen: the
        // game gives a layout byte it does not know the empty id 0.
        if let Some(o) = originals.remove(&TEXT_SET) {
            let seen = AtomicBool::new(false);
            d.register_override(TEXT_SET, "layouts:text", move |cpu, ctx, _| {
                announce(&seen, "layouts: text set");
                let pending = PENDING_VALUE.load(Ordering::Relaxed);
                if pending != 0 {
                    let mut mem = ctx.runtime().memory();
                    if mem.u32(cpu.esp + 4)? == 0 {
                        PENDING_VALUE.store(0, Ordering::Relaxed);
                        if let Some(l) = mem.u8(pending).ok().and_then(Layout::from_preset).filter(|l| !l.is_retail()) {
                            mem.set_u32(cpu.esp + 4, name_id(l))?;
                        }
                    }
                }
                Ok(o.invoke(cpu, ctx, TEXT_SET))
            });
        }
        if let Some(o) = originals.remove(&PROFILE_ROWS) {
            let sh = alias(d, &mut next_shadow, &o, PROFILE_ROWS);
            let seen = AtomicBool::new(false);
            d.register_override(PROFILE_ROWS, "layouts:profile-rows", move |cpu, ctx, _| {
                announce(&seen, "layouts: profile rows");
                PENDING_VALUE.store(cpu.ecx + PROFILE_COPY_LAYOUT, Ordering::Relaxed);
                call_then(cpu, ctx, sh, |_, _| {
                    PENDING_VALUE.store(0, Ordering::Relaxed);
                    Ok(())
                })
            });
        }
        // The Controls list's added item widgets, one per added row, and the
        // Xbox Live menu's additional visible item widgets.
        // Our classes' lists laid out taller than six rows get theirs too
        // (`list_class::extra_items`).
        if let Some(o) = originals.remove(&speaker_row::ADD_ITEMS) {
            let sh = alias(d, &mut next_shadow, &o, speaker_row::ADD_ITEMS);
            let seen = AtomicBool::new(false);
            let slots = Arc::new(std::sync::Mutex::new((Vec::new(), 0u32)));
            let live_slots = Arc::new(std::sync::Mutex::new((Vec::new(), 0u32)));
            d.register_override(speaker_row::ADD_ITEMS, "speaker:add-items", move |cpu, ctx, _| {
                announce(&seen, "speaker row: add items");
                let handler = cpu.ecx;
                let slots = Arc::clone(&slots);
                let live_slots = Arc::clone(&live_slots);
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let ret_pc = mem.u32(cpu.esp)?;
                cpu.esp = cpu.esp.wrapping_add(4);
                ctx.call_guest(cpu, &mut mem, sh, move |cpu, ctx| {
                    let rt = Arc::clone(ctx.runtime());
                    let mut mem = rt.memory();
                    let vtable = mem.u32(handler)?;
                    // Our class's lists (Video, Skulls) have another vtable
                    // and keep their six item widgets.
                    let items: Vec<u32> = if extra_rows > 0 && vtable == speaker_row::HANDLER_VTABLE {
                        let slot = {
                            let mut g = slots.lock().unwrap_or_else(|e| e.into_inner());
                            let (table, next) = &mut *g;
                            speaker_row::extra_slot(table, next, handler, EXTRA_BASE, EXTRA_SIZE, extra_rows)
                        };
                        (0..extra_rows).map(|i| slot + i * speaker_row::ITEM_SIZE).collect()
                    } else if with_live_menu && vtable == custom_games::HANDLER_VTABLE {
                        // Extra datums beyond the visible slots scroll normally.
                        let n = live_menu().visible() - custom_games::VANILLA_ITEMS;
                        let mut g = live_slots.lock().unwrap_or_else(|e| e.into_inner());
                        let (table, next) = &mut *g;
                        let size = LIVE_ITEM_SLOTS * LIVE_MAX_EXTRA * speaker_row::ITEM_SIZE;
                        let slot = speaker_row::extra_slot(table, next, handler, live.1, size, n);
                        (0..n).map(|i| slot + i * speaker_row::ITEM_SIZE).collect()
                    } else {
                        let ours = crate::list_class::extra_items(handler, vtable);
                        if ours.is_empty() {
                            return Ok(TransferAbi::ret(ret_pc));
                        }
                        ours
                    };
                    let player = u32::from(mem.u16(handler + 8)?);
                    let saved = (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp);
                    let finish: Finish = Box::new(move |cpu| {
                        (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp) = saved;
                        Ok(TransferAbi::ret(ret_pc))
                    });
                    add_items(cpu, ctx, &mut mem, handler, vtable, player, items, finish)
                })
            });
        }
        // The smaller Controls screen (the pause menu's Settings): after the
        // constructor, the speaker's datum (value 6) and one per row of ours
        // (crate::pause_menu), at the end.
        if let (Some(o), true) = (originals.remove(&speaker_row::SMALL_CTOR), with_small) {
            let sh = alias(d, &mut next_shadow, &o, speaker_row::SMALL_CTOR);
            let seen = AtomicBool::new(false);
            let values: Vec<u16> = with_speaker
                .then_some(speaker_row::SMALL_VALUE)
                .into_iter()
                .chain(pause_rows.iter().map(|r| r.value))
                .collect();
            d.register_override(speaker_row::SMALL_CTOR, "speaker:small-ctor", move |cpu, ctx, _| {
                announce(&seen, "small controls built");
                let this = cpu.esi;
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let ret_pc = mem.u32(cpu.esp)?;
                cpu.esp = cpu.esp.wrapping_add(4);
                let values = values.clone();
                ctx.call_guest(cpu, &mut mem, sh, move |cpu, ctx| {
                    let array = ctx.runtime().memory().u32(this + 0x70)?;
                    add_small_datums(cpu, ctx, array, this, values, ret_pc)
                })
            });
        }
        if let (Some(o), true) = (originals.remove(&speaker_row::SMALL_ROW_TEXT), with_small) {
            let seen = AtomicBool::new(false);
            let (s, rows) = (speaker_on.clone(), pause_rows.clone());
            d.register_override(speaker_row::SMALL_ROW_TEXT, "speaker:small-row-text", move |cpu, ctx, _| {
                let on = s.as_ref().map(|s| s.get());
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let text = on
                    .and_then(|on| speaker_row::small_row_text(&*mem, cpu.ecx, cpu.esp, on))
                    .or_else(|| {
                        crate::pause_menu::text_row(&*mem, cpu.ecx, cpu.esp, &rows)
                            .map(|r| (speaker_row::LABEL_TEXT_INDEX, r.label))
                    });
                if let Some(text) = text {
                    announce(&seen, "small controls: row text of ours");
                    let item = mem.u32(cpu.esp + 4)?;
                    return row_texts(cpu, ctx, &mut mem, item, vec![text]);
                }
                drop(mem);
                Ok(o.invoke(cpu, ctx, speaker_row::SMALL_ROW_TEXT))
            });
        }
        if let (Some(o), true) = (originals.remove(&speaker_row::SMALL_ROW_CHOSEN), with_small) {
            let store = Arc::clone(&store);
            let seen = AtomicBool::new(false);
            let rows = pause_rows.clone();
            d.register_override(speaker_row::SMALL_ROW_CHOSEN, "speaker:small-row-chosen", move |cpu, ctx, _| {
                let (speaker, ours) = {
                    let mem = ctx.runtime().memory();
                    (
                        with_speaker && speaker_row::small_is_ours(&*mem, cpu.ecx, cpu.esp),
                        crate::pause_menu::chosen_row(&*mem, cpu.ecx, cpu.esp, &rows),
                    )
                };
                if speaker {
                    announce(&seen, "speaker row: small row chosen");
                    // Through the store: its change callback applies the
                    // speaker (guest memory is not held here).
                    match store.step(PAD_SPEAKER.key) {
                        Ok(o) => eprintln!("{}", o.describe(&PAD_SPEAKER)),
                        Err(e) => eprintln!("pad speaker: {e}"),
                    }
                    return ctx.runtime().memory().ret(cpu, 8);
                }
                if let Some(row) = ours {
                    return open_from_small(cpu, ctx, row);
                }
                Ok(o.invoke(cpu, ctx, speaker_row::SMALL_ROW_CHOSEN))
            });
        }
        // The first-person camera's field of view: the original, then ours.
        if let (Some(o), Some(s)) = (originals.remove(&fov_row::PLAYER_FOV), fov.clone()) {
            let sh = alias(d, &mut next_shadow, &o, fov_row::PLAYER_FOV);
            let seen = AtomicBool::new(false);
            d.register_override(fov_row::PLAYER_FOV, "fov:camera", move |cpu, ctx, _| {
                announce(&seen, "fov: camera");
                let user = cpu.eax;
                // One atomic load: safe with guest memory held.
                let target = fov_row::target(s.get());
                call_then(cpu, ctx, sh, move |cpu, ctx| {
                    let lo: [u8; 4] = cpu.xmm[0][..4].try_into().unwrap_or_default();
                    let result = f32::from_bits(u32::from_le_bytes(lo));
                    let mut mem = ctx.runtime().memory();
                    if user == 0 {
                        // Player one's zoom, for direct touch look.
                        let z = fov_row::magnification(result, fov_row::unzoomed_fov(&*mem, user));
                        input::touch_look::set_zoom(z);
                    }
                    let out = fov_row::apply(&mut *mem, user, result, target);
                    drop(mem);
                    cpu.xmm[0][..4].copy_from_slice(&out.to_bits().to_le_bytes());
                    Ok(())
                })
            });
        }
        // The extended Xbox Live menu retains the retail rows. Our rows
        // have separate text and actions, including the Theater library.
        if let Some(o) = originals.remove(&custom_games::ROW_TEXT) {
            let seen = AtomicBool::new(false);
            d.register_override(custom_games::ROW_TEXT, "custom-games:row-text", move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut mem = rt.memory();
                let row = video_screen::text_row(&*mem, cpu.esp);
                let menu = live_menu();
                let Some(id) = row.and_then(|r| menu.row_text_id(r)) else {
                    drop(mem);
                    return Ok(o.invoke(cpu, ctx, custom_games::ROW_TEXT));
                };
                announce(&seen, "custom games: row text");
                // The Mods row drawn: its "N new" wants a fresh list (the
                // reader asks at most every few seconds; nothing waits).
                if row.map(|r| menu.row_kind(r)) == Some(custom_games::Row::Download)
                    && custom_games::mods_row_enabled()
                    && let Some(repo) = crate::mods_screen::repo()
                {
                    repo.want_list();
                }
                let item = mem.u32(cpu.esp + 4)?;
                row_texts(cpu, ctx, &mut mem, item, vec![(0, id)])
            });
        }
        // The visible Live menu rows, closer together than the skin's
        // item height so they fit where retail's four do: the layout, then
        // each row moved up to the pitch (`custom_games::repitch`). Every
        // other list goes straight to the original.
        if let Some(o) = originals.remove(&custom_games::LIST_LAYOUT) {
            let sh = alias(d, &mut next_shadow, &o, custom_games::LIST_LAYOUT);
            let told = Arc::new(AtomicBool::new(false));
            d.register_override(custom_games::LIST_LAYOUT, "custom-games:row-pitch", move |cpu, ctx, _| {
                let handler = cpu.esi;
                let live = ctx.runtime().memory().u32(handler).ok() == Some(custom_games::HANDLER_VTABLE);
                if !live {
                    return Ok(o.invoke(cpu, ctx, custom_games::LIST_LAYOUT));
                }
                let told = Arc::clone(&told);
                call_then(cpu, ctx, sh, move |_, ctx| {
                    let mut mem = ctx.runtime().memory();
                    let rows = speaker_row::children(&*mem, handler).map_or(0, |c| c.len() as u32);
                    let pitch = custom_games::pitch(rows);
                    match custom_games::repitch(&mut *mem, handler, custom_games::HANDLER_VTABLE, pitch) {
                        Some(n) if n > 0 && !told.swap(true, Ordering::Relaxed) => {
                            eprintln!("custom games: xbox live menu rows {pitch} apart ({rows} rows, {n} moved)")
                        }
                        None => eprintln!("custom games: the xbox live menu's rows could not be moved"),
                        _ => {}
                    }
                    Ok(())
                })
            });
        }
        if let Some(o) = originals.remove(&custom_games::ROW_CHOSEN) {
            let seen = AtomicBool::new(false);
            let sh = alias(d, &mut next_shadow, &o, custom_games::ROW_CHOSEN);
            d.register_override(custom_games::ROW_CHOSEN, "custom-games:row-chosen", move |cpu, ctx, _| {
                let (row, mask) = {
                    let mem = ctx.runtime().memory();
                    (chosen_row(&*mem, cpu.esp), video_screen::player_mask(&*mem, cpu.esp))
                };
                let kind = row.map_or(custom_games::Row::Retail, |r| live_menu().row_kind(r));
                if kind == custom_games::Row::Theater {
                    let (Some(theater), Some(mask), Some(loader)) = (
                        theater.as_ref(),
                        mask,
                        crate::list_class::loader_of(crate::theater_ui::FILMS),
                    ) else {
                        return ctx.runtime().memory().ret(cpu, 8);
                    };
                    let player = {
                        let rt = Arc::clone(ctx.runtime());
                        crate::skulls_menu::profile_of(&*rt.memory(), mask.trailing_zeros())
                    };
                    theater.open(mask.trailing_zeros(), player);
                    return open_screen(cpu, ctx, mask, loader, "theater: opening Films");
                }
                if !with_custom_games {
                    return Ok(o.invoke(cpu, ctx, custom_games::ROW_CHOSEN));
                }
                // The playlist screen OptiMatch's press opens lists the
                // clanker hoppers only after the Clankermatch row's press.
                crate::clankermatch::set_active(kind == custom_games::Row::Clankermatch);
                // Any other Live menu row ends hosting a co-op campaign
                // (crate::coop_live); no guest memory is held here.
                if kind != custom_games::Row::Coop {
                    crate::coop_live::drop_coop_mode();
                    if let Some(lobby) = crate::coop_live::lobby() {
                        lobby.leave();
                    }
                }
                if kind == custom_games::Row::Retail {
                    return Ok(o.invoke(cpu, ctx, custom_games::ROW_CHOSEN));
                }
                // Clankermatch and the moved Custom Game: the retail press
                // of their retail row (the press record rewritten, as the
                // co-op row does below), then the cursor row left on ours
                // (the retail press stored the retail row in it).
                if let Some(retail) = kind.retail_press() {
                    let shown = row.unwrap_or(retail);
                    {
                        let rt = Arc::clone(ctx.runtime());
                        let mut mem = rt.memory();
                        let record = mem.u32(cpu.esp.wrapping_add(8))?;
                        mem.set_u16(record, retail as u16)?;
                    }
                    if kind == custom_games::Row::Clankermatch {
                        eprintln!(
                            "clankermatch: Live menu row {shown} runs OptiMatch's press (row {retail})"
                        );
                    }
                    return call_then(cpu, ctx, sh, move |_, ctx| {
                        remember(ctx, shown);
                        Ok(())
                    });
                }
                announce(&seen, "custom games: row chosen");
                let Some(mask) = mask else {
                    eprintln!("custom games: no local player for the press; nothing opened");
                    return ctx.runtime().memory().ret(cpu, 8);
                };
                if kind == custom_games::Row::Download {
                    let mods = custom_games::mods_row_enabled()
                        .then(crate::mods_screen::repo)
                        .flatten()
                        .zip(crate::list_class::loader_of(crate::mods_screen::SCREEN));
                    if let Some((repo, loader)) = mods {
                        repo.opened();
                        return open_screen(cpu, ctx, mask, loader, "mods: opening the mods screen");
                    }
                    eprintln!("custom games: showing the map pack dialog");
                    return open_dialog(cpu, ctx, mask, custom_games::MAPPACK_ERROR);
                }
                if kind == custom_games::Row::Coop && !crate::coop_live::list_ui() {
                    // The game's own lobby: the Custom Game row's path
                    // (0x231001: a Live squad session, then the lobby mode),
                    // whose mode call becomes co-op (crate::coop_live).
                    let Some(lobby) = crate::coop_live::lobby() else {
                        eprintln!("coop-live: the co-op hooks are off; nothing opened");
                        return ctx.runtime().memory().ret(cpu, 8);
                    };
                    lobby.open_retail();
                    let rt = Arc::clone(ctx.runtime());
                    {
                        let mut mem = rt.memory();
                        let record = mem.u32(cpu.esp.wrapping_add(8))?;
                        mem.set_u16(record, custom_games::RETAIL_CUSTOM_GAME_ROW as u16)?;
                    }
                    eprintln!(
                        "coop-live: Live menu row {} opens the custom game lobby (row {}) in co-op mode",
                        row.unwrap_or(-1),
                        custom_games::RETAIL_CUSTOM_GAME_ROW
                    );
                    // The retail press stores its row 2 (now Clankermatch's)
                    // for the cursor: ours goes over it.
                    let shown = row.unwrap_or(custom_games::COOP_ROW);
                    return call_then(cpu, ctx, sh, move |_, ctx| {
                        remember(ctx, shown);
                        Ok(())
                    });
                }
                if kind == custom_games::Row::Coop {
                    remember(ctx, row.unwrap_or(custom_games::COOP_ROW));
                    let screen = crate::coop_live::lobby().map(|l| l.open_from_menu());
                    let Some(loader) = screen.and_then(crate::list_class::loader_of) else {
                        eprintln!("coop-live: the co-op screens are off; nothing opened");
                        return ctx.runtime().memory().ret(cpu, 8);
                    };
                    let note = if screen == Some(crate::coop_live::JOIN) {
                        "coop-live: opening the join screen"
                    } else {
                        "coop-live: opening the co-op screen"
                    };
                    eprintln!(
                        "coop-live: Live menu row {} opens screen {:?}, loader {loader:#x}",
                        row.unwrap_or(-1),
                        screen.unwrap_or("?")
                    );
                    return open_screen(cpu, ctx, mask, loader, note);
                }
                // The browser's row is past the rows the menu restores its
                // cursor to; `custom_games::LiveMenuCursor` restores it.
                remember(ctx, row.unwrap_or(custom_games::ROW));
                open_screen(cpu, ctx, mask, custom_games::LIVE_BROWSER_LOADER, "custom games: opening the browser")
            });
        }
        // Back from the Live browser: its event handler always opens the
        // System Link screen (`0x252433`) with a back request. While a
        // Live-mode browser handles an event, that loader call goes to the
        // Live menu's instead. A LAN browser never sets the flag.
        if let (Some(event), Some(syslink), Some(live_menu)) = (
            originals.remove(&custom_games::BROWSER_EVENT),
            originals.remove(&custom_games::BROWSER_BACK_LOADER),
            originals.remove(&custom_games::LIVE_MENU_LOADER),
        ) {
            static LIVE_BACK: AtomicBool = AtomicBool::new(false);
            let sh = alias(d, &mut next_shadow, &event, custom_games::BROWSER_EVENT);
            d.register_override(custom_games::BROWSER_EVENT, "custom-games:browser-event", move |cpu, ctx, _| {
                if !custom_games::browser_is_live(&*ctx.runtime().memory(), cpu.ecx) {
                    return Ok(event.invoke(cpu, ctx, custom_games::BROWSER_EVENT));
                }
                LIVE_BACK.store(true, Ordering::SeqCst);
                call_then(cpu, ctx, sh, |_, _| {
                    LIVE_BACK.store(false, Ordering::SeqCst);
                    Ok(())
                })
            });
            d.register_override(custom_games::BROWSER_BACK_LOADER, "custom-games:browser-back", move |cpu, ctx, _| {
                let redirect = LIVE_BACK.swap(false, Ordering::SeqCst) && {
                    let rt = Arc::clone(ctx.runtime());
                    let mut mem = rt.memory();
                    let request = mem.u32(cpu.esp + 4)?;
                    custom_games::back_loader(&mut *mem, request).is_some()
                };
                if !redirect {
                    return Ok(syslink.invoke(cpu, ctx, custom_games::BROWSER_BACK_LOADER));
                }
                eprintln!("custom games: back from the live browser to the xbox live menu");
                Ok(live_menu.invoke(cpu, ctx, custom_games::LIVE_MENU_LOADER))
            });
        }
        // The dialog error lookup: our Map Pack dialog's title and message.
        if let Some(o) = originals.remove(&custom_games::ERROR_LOOKUP) {
            let sh = alias(d, &mut next_shadow, &o, custom_games::ERROR_LOOKUP);
            d.register_override(custom_games::ERROR_LOOKUP, "custom-games:dialog-text", move |cpu, ctx, _| {
                let (entry, error) = (cpu.esi, ctx.runtime().memory().u32(cpu.esp + 4)?);
                if error != custom_games::MAPPACK_ERROR {
                    return Ok(o.invoke(cpu, ctx, custom_games::ERROR_LOOKUP));
                }
                call_then(cpu, ctx, sh, move |_, ctx| {
                    if !custom_games::answer_error(&mut *ctx.runtime().memory(), entry, error) {
                        eprintln!("custom games: the map pack dialog's text could not be set");
                    }
                    Ok(())
                })
            });
        }
    });
    // The Video screen: our own list class, opened by the Settings row.
    if with_video {
        // Opened from the pause menus too: one line a row in a game.
        let class = crate::list_class::ListClass::new(
            "video-screen",
            video_screen::SCREEN,
            Arc::clone(&model),
        )
        .lines();
        match registry.add(&class) {
            Ok(true) => {}
            Ok(false) => eprintln!("video screen: features.video-screen is off"),
            Err(e) => eprintln!("video screen: {e}"),
        }
        // Its row on the Settings list, as data on the plan.
        let rows = crate::retail_rows::Extension {
            id: "settings-rows",
            title: "Our rows on the Settings list",
            list: List::Settings,
            text: &feature::points::SETTINGS_ROW_TEXT,
            chosen: &feature::points::SETTINGS_ROW_CHOSEN,
            label_only: true,
            model: Arc::clone(&model),
        };
        registry
            .add(&rows)
            .map_err(|e| anyhow::anyhow!("settings rows: {e}"))?;
    }
    // The Voice screen: our own list class, opened from the pause menus.
    if with_voice {
        // Opened from the pause menus too: one line a row in a game.
        let class = crate::list_class::ListClass::new(
            "voice-screen",
            video_screen::VOICE_SCREEN,
            Arc::clone(&model),
        )
        .lines();
        match registry.add(&class) {
            Ok(true) => {}
            Ok(false) => eprintln!("voice screen: features.voice-screen is off"),
            Err(e) => eprintln!("voice screen: {e}"),
        }
    }
    // The Skulls page: our own list class, opened after the difficulty when
    // the pressing profile has a skull (a plan hook on the difficulty
    // press), with the level select's layout.
    if with_skulls {
        let screen = Arc::new(skulls_menu::SkullsScreen::new(Arc::clone(model.store())));
        let (block, line) =
            skulls_menu::description_block(std::env::var("H2_SKULLS_TEXT_RECT").ok().as_deref());
        if let Some(line) = line {
            eprintln!("{line}");
        }
        let (layout, picture, lines) = skulls_menu::placement(|k| std::env::var(k).ok());
        for line in lines {
            eprintln!("{line}");
        }
        let (flow, line) = crate::skulls_screen::PageFlow::from_env();
        if let Some(line) = line {
            eprintln!("{line}");
        }
        let flow = Arc::new(flow);
        let class = crate::skulls_screen::class(
            Arc::clone(&screen),
            block,
            layout,
            picture,
            Arc::clone(&flow),
        );
        match registry.add(&class) {
            Ok(true) => {}
            Ok(false) => {
                flow.set_on(false);
                eprintln!(
                    "skulls menu: features.skulls-screen is off; the difficulty starts as retail"
                )
            }
            Err(e) => {
                flow.set_on(false);
                eprintln!("skulls menu: {e}")
            }
        }
        registry
            .add(&crate::skulls_screen::DifficultyPage {
                screen: Arc::clone(&screen),
                flow,
                store: Arc::clone(model.store()),
            })
            .map_err(|e| anyhow::anyhow!("skulls page: {e}"))?;
        // The multiplayer screen, an instance and class of its own, opened
        // from the game variant editor (docs/game/skulls.md "In multiplayer").
        let mp = Arc::new(skulls_menu::SkullsScreen::multiplayer(Arc::clone(
            model.store(),
        )));
        match registry.add(&crate::skulls_mp_row::mp_class(
            Arc::clone(&mp),
            block,
            layout,
            picture,
        )) {
            Ok(true) => {}
            Ok(false) => eprintln!("skulls menu: features.skulls-mp-screen is off"),
            Err(e) => eprintln!("skulls menu: {e}"),
        }
        registry
            .add(&crate::skulls_mp_row::VariantRow(mp))
            .map_err(|e| anyhow::anyhow!("skulls variant row: {e}"))?;
    }
    if let Some(class) = crate::campaign_menu::class() {
        match registry.add(&class) {
            Ok(true) => {}
            Ok(false) => eprintln!("campaigns: features.campaigns-screen is off; the level select opens as retail"),
            Err(e) => eprintln!("campaigns: {e}"),
        }
    }
    // The Controls list's row text and row chosen, on the plan: our rows
    // there (the speaker row, the Steam Controller's) as data, and the
    // layouts' value text around the original.
    if with_controls {
        let rows = crate::retail_rows::Extension {
            id: "controls-rows",
            title: "Our rows on the Controls list",
            list: List::Controls,
            text: &feature::points::CONTROLS_ROW_TEXT,
            chosen: &feature::points::CONTROLS_ROW_CHOSEN,
            label_only: false,
            model: Arc::clone(&model),
        };
        registry
            .add(&rows)
            .map_err(|e| anyhow::anyhow!("controls rows: {e}"))?;
    }
    if layouts {
        registry
            .add(&LayoutValueText)
            .map_err(|e| anyhow::anyhow!("layouts: {e}"))?;
    }
    // The retail Controls row text, ported (crate::controls_text); our
    // rows' hooks and the layouts' value pair run around it.
    registry
        .add(&crate::controls_text::ControlsText)
        .map_err(|e| anyhow::anyhow!("controls text: {e}"))?;
    // Our strings: a source for the locale lookup (the Rust one serves it;
    // with that off, the plan's locale bridge does).
    registry
        .add(&StringsFeature)
        .map_err(|e| anyhow::anyhow!("layouts: {e}"))?;
    Ok(())
}

/// The Controls row text's layout value: while the original draws a row,
/// an empty value text stands for the edited profile's layout byte (the
/// text hook names an added layout). Inside our rows' hook at that point.
pub struct LayoutValueText;

impl feature::Feature for LayoutValueText {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("layouts-value", "Added layouts' names on the Controls list")
            .doc("docs/input/controller-layouts.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        b.raw()
            .before(
                &feature::points::CONTROLS_ROW_TEXT,
                "layout value pending",
                |_| {
                    PENDING_VALUE.store(EDIT_LAYOUT, Ordering::Relaxed);
                    Ok(feature::Flow::Continue)
                },
            )
            .after_feature("controls-rows");
        b.raw().after(
            &feature::points::CONTROLS_ROW_TEXT,
            "layout value done",
            |_| {
                PENDING_VALUE.store(0, Ordering::Relaxed);
                Ok(())
            },
        );
        b.snapshot(feature::SnapshotPolicy::forbid_while(layout_value_live));
        Ok(())
    }
}

/// Why `layouts-value` can't be snapshotted now: a Controls row text is
/// being drawn, between its before and after hooks.
pub fn layout_value_live() -> Option<String> {
    layout_value_reason(PENDING_VALUE.load(Ordering::Relaxed))
}

fn layout_value_reason(pending: u32) -> Option<String> {
    (pending != 0).then(|| "a Controls row text is being drawn".into())
}

/// The strings our rows and layouts show (`ui-strings`), served from guest
/// memory written at install.
pub struct StringsFeature;

impl feature::Feature for StringsFeature {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("ui-strings", "Our menu strings").doc("docs/game/settings-menu.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        b.locale().source("our strings", |id| {
            if !is_ours(id) {
                return None;
            }
            STRING_ADDRS
                .get()?
                .iter()
                .find(|(i, _)| *i == id)
                .map(|(_, a)| *a)
        });
        // The strings are written and their addresses fixed at install.
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Register an override at a shadow address that runs the original at `addr`.
fn alias(
    d: &mut Dispatch,
    next: &mut impl FnMut() -> u32,
    o: &Arc<OriginalHandler>,
    addr: u32,
) -> u32 {
    let sh = next();
    let o = Arc::clone(o);
    d.register_override(sh, "layouts:original", move |cpu, ctx, _| {
        Ok(o.invoke(cpu, ctx, addr))
    });
    sh
}

type Finish = Box<dyn FnOnce(&mut CpuAbi) -> Result<TransferAbi, GuestFault> + Send>;

/// The pause menu's Settings: adds a datum with each of `values` to the
/// rows' array, one guest call each, then returns from the constructor
/// (EAX the list) to `ret_pc`.
fn add_small_datums(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    array: u32,
    this: u32,
    mut values: Vec<u16>,
    ret_pc: u32,
) -> Result<TransferAbi, GuestFault> {
    if values.is_empty() {
        cpu.eax = this;
        return Ok(TransferAbi::ret(ret_pc));
    }
    let value = values.remove(0);
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    cpu.edx = array;
    ctx.call_guest(cpu, &mut mem, speaker_row::DATUM_NEW, move |cpu, ctx| {
        let handle = cpu.eax;
        if !speaker_row::set_datum_value(&mut *ctx.runtime().memory(), array, handle, value) {
            eprintln!("small controls: no room for the row with value {value}");
        }
        add_small_datums(cpu, ctx, array, this, values, ret_pc)
    })
}

/// A press on our row of the pause menu's Settings: opens its screen on
/// the Settings screen's own window and slot (the request on the stack,
/// handed to the class's loader), then returns as the row chosen does
/// (`ret 8`).
fn open_from_small(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    row: crate::pause_menu::SettingsRow,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let loader = crate::list_class::loader_of(row.screen);
    let request = loader.and_then(|l| crate::pause_menu::open_request(&*mem, cpu.ecx, cpu.esp, l));
    let (Some(loader), Some(request)) = (loader, request) else {
        eprintln!("pause menu: {} not opened (its class is off, or no player or stack)", row.screen);
        return mem.ret(cpu, 8);
    };
    eprintln!("pause menu: opening {} from Settings", row.screen);
    let ret_pc = mem.u32(cpu.esp)?;
    // Pop the return and the two arguments; the request below the stack.
    cpu.esp = cpu.esp.wrapping_add(12);
    let saved = cpu.esp;
    cpu.esp = cpu.esp.wrapping_sub(0x20);
    let at = cpu.esp;
    mem.write(at, request)?;
    cpu.esp = cpu.esp.wrapping_sub(4);
    mem.set_u32(cpu.esp, at)?;
    ctx.call_guest(cpu, &mut mem, loader, move |cpu, _| {
        cpu.esp = saved;
        Ok(TransferAbi::ret(ret_pc))
    })
}

/// In place of a row text function (`ret 8`, item widget first): set each
/// `(text index, string id)` of the item by the same two calls the game
/// makes: find the item's text widget (this = item, arguments 6, index, 0),
/// then set its text. Registers the original preserves (ESI, EDI) are
/// restored at the end.
fn row_texts(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
    widget: u32,
    texts: Vec<(u32, u32)>,
) -> Result<TransferAbi, GuestFault> {
    let ret_pc = mem.u32(cpu.esp)?;
    cpu.esp = cpu.esp.wrapping_add(12); // return address and the two arguments
    let (esi, edi) = (cpu.esi, cpu.edi);
    let finish: Finish = Box::new(move |cpu| {
        cpu.esi = esi;
        cpu.edi = edi;
        Ok(TransferAbi::ret(ret_pc))
    });
    next_text(cpu, ctx, mem, widget, texts, finish)
}

fn next_text(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
    widget: u32,
    mut texts: Vec<(u32, u32)>,
    finish: Finish,
) -> Result<TransferAbi, GuestFault> {
    if texts.is_empty() {
        return finish(cpu);
    }
    let (index, id) = texts.remove(0);
    // The game pushes 0, index, 6, so 6 is on top.
    for v in [0, index, 6] {
        push(mem, cpu, v)?;
    }
    cpu.ecx = widget;
    ctx.call_guest(cpu, mem, TEXT_WIDGET_GET, move |cpu, ctx| {
        let text = cpu.eax;
        let rt = Arc::clone(ctx.runtime());
        let mut mem = rt.memory();
        if text == 0 {
            return next_text(cpu, ctx, &mut mem, widget, texts, finish);
        }
        cpu.edi = text;
        cpu.esi = widget;
        push(&mut mem, cpu, id)?;
        ctx.call_guest(cpu, &mut mem, TEXT_SET, move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            next_text(cpu, ctx, &mut mem, widget, texts, finish)
        })
    })
}

/// Link each of `items` as a child of the list `handler` (of handler vtable
/// `vtable`: Controls, or the Xbox Live menu), in order, after the retail
/// item widgets: build an item first (zeroed, the item
/// constructor, then the local player) when it is not a child yet, else just
/// move it back to the end (the original re-appends the retail items).
#[allow(clippy::too_many_arguments)]
fn add_items(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mem: &mut Guest,
    handler: u32,
    vtable: u32,
    player: u32,
    mut items: Vec<u32>,
    finish: Finish,
) -> Result<TransferAbi, GuestFault> {
    if items.is_empty() {
        return finish(cpu);
    }
    let item = items.remove(0);
    let Some(build) = speaker_row::item_needed(&*mem, handler, vtable, item) else {
        return finish(cpu);
    };
    let link = move |cpu: &mut CpuAbi, ctx: &mut ExecCtx, mem: &mut Guest| {
        cpu.eax = item;
        cpu.esi = handler;
        ctx.call_guest(cpu, mem, speaker_row::ADD_CHILD, move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            add_items(cpu, ctx, &mut mem, handler, vtable, player, items, finish)
        })
    };
    if !build {
        return link(cpu, ctx, mem);
    }
    mem.write_bytes(item, &[0; speaker_row::ITEM_SIZE as usize], 1)?;
    cpu.ecx = item;
    ctx.call_guest(cpu, mem, speaker_row::ITEM_CTOR, move |cpu, ctx| {
        let rt = Arc::clone(ctx.runtime());
        let mut mem = rt.memory();
        push(&mut mem, cpu, player)?;
        cpu.ecx = item;
        ctx.call_guest(cpu, &mut mem, speaker_row::SET_PLAYER, move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            link(cpu, ctx, &mut mem)
        })
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn snapshots_wait_while_a_controls_row_text_is_drawn() {
        assert_eq!(super::layout_value_reason(0), None);
        assert!(super::layout_value_reason(super::EDIT_LAYOUT).is_some());
    }
    use super::*;
    use crate::ui::Chosen;
    use std::collections::BTreeMap;

    /// A sparse byte map standing in for guest memory.
    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    fn put(f: &mut Fake, addr: u32, bytes: &[u8]) {
        assert!(f.wr(addr, bytes));
    }

    #[test]
    fn rows_four_to_six_are_ours_and_zero_to_three_are_not() {
        for r in 0..4 {
            assert_eq!(layout_of_row(r), None);
        }
        assert_eq!(layout_of_row(4), Some(Layout::BumperJumper));
        assert_eq!(layout_of_row(5), Some(Layout::Recon));
        assert_eq!(layout_of_row(6), Some(Layout::WalkieTalkie));
        assert_eq!(layout_of_row(7), None);
        assert_eq!(layout_of_row(-1), None);
    }

    #[test]
    fn only_the_layout_list_grows() {
        assert_eq!(list_count(LIST_NAME, 4), 7);
        assert_eq!(list_count(LIST_NAME, 2), 2);
        assert_eq!(list_count(0x1234, 4), 4);
    }

    #[test]
    fn the_table_is_rewritten_for_added_layouts_only() {
        for byte in 0u8..=255 {
            let mut f = Fake::default();
            let settings = SETTINGS_BASE + SETTINGS_STRIDE * 2 + 4;
            let table = TABLE_BASE + TABLE_STRIDE * 2 + TABLE_SLOTS;
            put(&mut f, settings, &[byte]);
            put(&mut f, table, &[0xaa; 16]);
            let wrote = apply_action_table(&mut f, 2);
            let mut got = [0; 16];
            assert!(f.rd(table, &mut got));
            match Layout::from_preset(byte).filter(|l| !l.is_retail()) {
                Some(l) => {
                    assert!(wrote, "{byte}");
                    assert_eq!(got, l.action_map());
                }
                None => {
                    // Retail layouts and unknown bytes: exactly what the
                    // original left there.
                    assert!(!wrote, "{byte}");
                    assert_eq!(got, [0xaa; 16], "{byte}");
                }
            }
        }
    }

    #[test]
    fn bumper_jumper_puts_jump_on_white_in_slot_zero() {
        let mut f = Fake::default();
        put(&mut f, SETTINGS_BASE + 4, &[4]);
        put(&mut f, TABLE_BASE + TABLE_SLOTS, &[0; 16]);
        assert!(apply_action_table(&mut f, 0));
        assert_eq!(f.u8(TABLE_BASE + TABLE_SLOTS), Some(Button::White.index()));
    }

    #[test]
    fn pane_rows_leave_retail_alone_and_map_ours_past_the_retail_panes() {
        assert_eq!(EXTENDED_PANES, 14);
        for base in [0i8, 4] {
            for row in 0..4 {
                assert_eq!(pane_row(row, base, EXTENDED_PANES), None);
            }
        }
        // Standard diagram: panes 8, 9, 10. Legacy diagram: 11, 12, 13.
        for (k, row) in [4i16, 5, 6].into_iter().enumerate() {
            assert_eq!(
                i16::from(0i8) + pane_row(row, 0, EXTENDED_PANES).unwrap(),
                8 + k as i16
            );
            assert_eq!(
                i16::from(4i8) + pane_row(row, 4, EXTENDED_PANES).unwrap(),
                11 + k as i16
            );
        }
    }

    #[test]
    fn panes_are_only_moved_on_a_tag_we_extended() {
        // A tag left at its eight retail panes (or any other count, or no
        // tag at all) has nothing at 8 to 13: every row keeps its pane.
        for panes in [0, 1, VANILLA_PANES, 13, 15, u32::MAX] {
            for base in [0i8, 4] {
                for row in -1..10 {
                    assert_eq!(
                        pane_row(row, base, panes),
                        None,
                        "row {row} base {base} panes {panes}"
                    );
                }
            }
        }
        // And on the extended tag every pane it hands out exists.
        for base in [0i8, 4] {
            for row in 4..7 {
                let pane = i16::from(base) + pane_row(row, base, EXTENDED_PANES).unwrap();
                assert!((0..EXTENDED_PANES as i16).contains(&pane));
            }
        }
    }

    /// A list handler at `0x9000` whose datum array holds `rows` rows.
    fn listed(rows: u32) -> (Fake, u32) {
        let mut f = Fake::default();
        let (handler, array) = (0x9000u32, 0xa000u32);
        put(&mut f, handler + 0x70, &array.to_le_bytes());
        put(&mut f, array + 0x20, &rows.to_le_bytes());
        (f, handler)
    }

    #[test]
    fn row_hooks_act_only_on_rows_the_list_has() {
        // The grown list: all seven rows are its own.
        let (f, h) = listed(7);
        assert_eq!(list_rows(&f, h), Some(7));
        for row in 0..7 {
            assert!(row_listed(&f, h, row));
        }
        assert!(!row_listed(&f, h, 7));
        assert!(!row_listed(&f, h, -1));
        // A list that kept four rows (another screen's, or the list hook
        // skipped): rows 4 to 6 are not there, so no hook acts on them.
        let (f, h) = listed(4);
        for row in 4..7 {
            assert!(!row_listed(&f, h, row), "row {row}");
        }
        assert!(row_listed(&f, h, 3));
        // Unreadable handler or array, or a negative count: nothing is ours.
        assert!(!row_listed(&Fake::default(), 0x9000, 4));
        let (mut f, h) = listed(0);
        put(&mut f, 0xa000 + 0x20, &(-1i32).to_le_bytes());
        assert_eq!(list_rows(&f, h), None);
        assert!(!row_listed(&f, h, 0));
    }

    #[test]
    fn the_initial_row_reads_the_screens_own_list() {
        // `0x2b463d` hands `this + 0x614` to the list select; the guard reads
        // that list's rows.
        let (f, h) = listed(7);
        let screen = h - SCREEN_LIST;
        for l in ADDED {
            assert!(row_listed(&f, screen + SCREEN_LIST, i16::from(l.preset())));
        }
        let (f, h) = listed(4);
        for l in ADDED {
            assert!(!row_listed(&f, h, i16::from(l.preset())));
        }
    }

    #[test]
    fn the_action_table_is_left_alone_past_the_fourth_player() {
        for player in [4u32, 5, 0xff, u32::MAX] {
            let mut f = Fake::default();
            // Even with a layout byte of 4 readable where that player's
            // settings would be (wrapping included), nothing is written.
            put(
                &mut f,
                SETTINGS_BASE.wrapping_add(SETTINGS_STRIDE.wrapping_mul(player)) + 4,
                &[4],
            );
            let before = f.0.clone();
            assert!(!apply_action_table(&mut f, player), "player {player}");
            assert_eq!(f.0, before);
        }
        for player in 0..MAX_PLAYERS {
            let mut f = Fake::default();
            put(&mut f, SETTINGS_BASE + SETTINGS_STRIDE * player + 4, &[4]);
            put(
                &mut f,
                TABLE_BASE + TABLE_STRIDE * player + TABLE_SLOTS,
                &[0; 16],
            );
            assert!(apply_action_table(&mut f, player), "player {player}");
        }
    }

    #[test]
    fn our_ids_are_recognised_and_distinct() {
        let all = strings();
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        assert!(!is_ours(0x0700_0001));
        assert!(!is_ours(0));
        assert!(!is_ours(0xffff_ffff));
    }

    #[test]
    fn strings_are_written_nul_terminated_and_addressable() {
        for (layouts, speaker) in [(true, true), (true, false), (false, true)] {
            let mut f = Fake::default();
            let all = served_strings(layouts, speaker);
            let (addrs, end) = write_strings(&mut f, 0x1000, &all).unwrap();
            assert_eq!(addrs.len(), all.len());
            assert_eq!(
                end - 0x1000,
                all.iter().map(|(_, t)| t.len() as u32 + 1).sum::<u32>()
            );
            for (id, a) in addrs {
                assert!(is_ours(id));
                let text = all.iter().find(|(i, _)| *i == id).unwrap().1;
                let mut b = vec![0; text.len() + 1];
                assert!(f.rd(a, &mut b));
                assert_eq!(&b[..text.len()], text.as_bytes());
                assert_eq!(b[text.len()], 0);
            }
        }
        assert_eq!(served_strings(true, false).len(), 6);
        assert_eq!(
            served_strings(false, true),
            [
                (speaker_row::SMALL_ON_ID, speaker_row::SMALL_ON_TEXT),
                (speaker_row::SMALL_OFF_ID, speaker_row::SMALL_OFF_TEXT),
            ]
        );
        let mut ids: Vec<u32> = served_strings(true, true).iter().map(|(i, _)| *i).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 8);
    }

    /// A datum array constructor call: name at [esp+4], element size 4 at
    /// [esp+8] (both list screens push 4).
    fn list_call(name: u32) -> Fake {
        let mut f = Fake::default();
        put(&mut f, 0x1004, &name.to_le_bytes());
        put(&mut f, 0x1008, &4u32.to_le_bytes());
        f
    }

    #[test]
    fn the_list_count_is_eax_not_the_element_size() {
        // The layout list: count 4 in EAX, element size 4 at [esp+8]. Only
        // EAX grows, so the element size stays as the game set it.
        let f = list_call(LIST_NAME);
        assert_eq!(grown_count(&f, 0x1000, 4, true, true), 7);
        assert_eq!(f.u32(0x1008), Some(4));
        // The Controls list: count 6; the model adds its rows after
        // (`video_screen::list_count`).
        let f = list_call(speaker_row::LIST_NAME);
        assert_eq!(grown_count(&f, 0x1000, 6, true, true), 6);
        // The smaller Controls screen's list: count 6, one more too.
        let f = list_call(speaker_row::SMALL_LIST_NAME);
        assert_eq!(grown_count(&f, 0x1000, 6, false, true), 7);
        assert_eq!(grown_count(&f, 0x1000, 6, true, false), 6);
        assert_eq!(
            grown_count(&list_call(LIST_NAME), 0x1000, 4, false, true),
            4
        );
        // An element size that happens to match does not count.
        let f = list_call(speaker_row::LIST_NAME);
        assert_eq!(grown_count(&f, 0x1000, 4, true, true), 4);
        // Any other list, or unreadable memory: unchanged.
        assert_eq!(grown_count(&list_call(0x1234), 0x1000, 4, true, true), 4);
        assert_eq!(grown_count(&list_call(0x1234), 0x1000, 6, true, true), 6);
        assert_eq!(grown_count(&Fake::default(), 0x1000, 4, true, true), 4);
    }

    #[test]
    fn the_chosen_row_is_the_second_argument() {
        // 0x2c6567 reads `[esp+8]` at entry; the first argument is an
        // unrelated pointer.
        let mut f = Fake::default();
        put(&mut f, 0x1004, &0x3000u32.to_le_bytes());
        put(&mut f, 0x3000, &1u16.to_le_bytes());
        put(&mut f, 0x1008, &0x4000u32.to_le_bytes());
        put(&mut f, 0x4000, &5u16.to_le_bytes());
        assert_eq!(chosen_row(&f, 0x1000), Some(5));
        assert_eq!(
            layout_of_row(chosen_row(&f, 0x1000).unwrap()),
            Some(Layout::Recon)
        );
        assert_eq!(chosen_row(&Fake::default(), 0x1000), None);
    }

    #[test]
    fn relabeling_moves_actions_to_their_new_buttons() {
        // Bumper Jumper: the slot at A (jump by default) shows swap grenades,
        // the White slot (flashlight) shows jump, the Black slot shows melee.
        use Action::*;
        assert_eq!(relabel(Layout::BumperJumper, Jump), SwitchGrenades);
        assert_eq!(relabel(Layout::BumperJumper, Flashlight), Jump);
        assert_eq!(relabel(Layout::BumperJumper, SwitchGrenades), Melee);
        assert_eq!(relabel(Layout::BumperJumper, Melee), ActionReload);
        assert_eq!(relabel(Layout::BumperJumper, ActionReload), Flashlight);
        // Untouched slots keep their label, and Default relabels to itself.
        for l in ADDED {
            assert_eq!(relabel(l, Fire), Fire);
            assert_eq!(relabel(l, Crouch), Crouch);
        }
        for a in Action::GAMEPLAY {
            assert_eq!(relabel(Layout::Default, a), a);
        }
        // A relabel is a permutation of the slots.
        for l in ADDED {
            let mut seen = std::collections::HashSet::new();
            for a in Action::GAMEPLAY {
                assert!(seen.insert(relabel(l, a)), "{l:?}");
            }
        }
    }

    /// A tag with 8 panes of 13 texts, text sids `base + pane*100 + i`.
    fn fake_tag(f: &mut Fake) -> u32 {
        let (wgit, panes, texts) = (0x10_0000u32, 0x11_0000u32, 0x12_0000u32);
        put(f, wgit + 4, &(SCREEN_ID as u16).to_le_bytes());
        put(f, wgit + 0x20, &VANILLA_PANES.to_le_bytes());
        put(f, wgit + 0x24, &panes.to_le_bytes());
        for p in 0..VANILLA_PANES {
            let pane = panes + p * PANE_SIZE;
            let t = texts + p * 13 * TEXT_SIZE;
            put(f, pane, &[p as u8; PANE_SIZE as usize]);
            put(f, pane + 0x1c, &13u32.to_le_bytes());
            put(f, pane + 0x20, &t.to_le_bytes());
            for i in 0..13 {
                // Every pane has the same label per slot, like the retail
                // default panes; the help text differs.
                let sid = if i == 0 { 0x900 + p } else { 0x500 + i };
                put(f, t + i * TEXT_SIZE, &[0xee; TEXT_SIZE as usize]);
                put(f, t + i * TEXT_SIZE + 0x24, &sid.to_le_bytes());
            }
        }
        wgit
    }

    #[test]
    fn panes_are_extended_once_and_retail_panes_are_untouched() {
        let mut f = Fake::default();
        let wgit = fake_tag(&mut f);
        let old = f.u32(wgit + 0x24).unwrap();
        let mut before = vec![0; (8 * PANE_SIZE) as usize];
        assert!(f.rd(old, &mut before));
        let mut alloc = 0x20_0000;
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(true));
        assert_eq!(f.u32(wgit + 0x20), Some(14));
        let new = f.u32(wgit + 0x24).unwrap();
        // The first eight panes are byte for byte the originals.
        let mut after = vec![0; (8 * PANE_SIZE) as usize];
        assert!(f.rd(new, &mut after));
        assert_eq!(before, after);
        // Bumper Jumper (pane 8): help text ours, jump's slot (slot 9, sid
        // 0x509) shows swap grenades (slot 10's sid 0x50a) and so on.
        let texts = f.u32(new + 8 * PANE_SIZE + 0x20).unwrap();
        let sid = |i: u32| f.u32(texts + i * TEXT_SIZE + 0x24).unwrap();
        assert_eq!(sid(0), help_id(Layout::BumperJumper));
        assert_eq!(sid(9), 0x50a); // A slot: swap grenades
        assert_eq!(sid(11), 0x509); // White slot: jump
        assert_eq!(sid(10), 0x508); // Black slot: melee
        assert_eq!(sid(8), 0x506); // B slot: reload
        assert_eq!(sid(6), 0x50b); // X slot: flashlight
        assert_eq!(sid(5), 0x505); // fire stays
        // Legacy-diagram clones come after the standard ones.
        let legacy = f.u32(new + 11 * PANE_SIZE + 0x20).unwrap();
        assert_eq!(f.u32(legacy + 0x24), Some(help_id(Layout::BumperJumper)));
        // A second call changes nothing.
        let end = alloc;
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(false));
        assert_eq!(alloc, end);
    }

    #[test]
    fn other_screens_and_odd_tags_are_left_alone() {
        let mut f = Fake::default();
        let wgit = fake_tag(&mut f);
        put(&mut f, wgit + 4, &39u16.to_le_bytes());
        let mut alloc = 0x20_0000;
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(false));
        put(&mut f, wgit + 4, &(SCREEN_ID as u16).to_le_bytes());
        put(&mut f, wgit + 0x20, &5u32.to_le_bytes());
        assert_eq!(extend_panes(&mut f, wgit, &mut alloc), Some(false));
        assert_eq!(alloc, 0x20_0000);
    }

    #[test]
    fn choosing_the_rows_saves_settings_toml_under_the_data_root() {
        use crate::setting_defs::{ALL, FPS, RENDER_SCALE};
        use crate::speaker_setting::SpeakerSetting;
        use settings::{Store, Value};
        // `h2 play`'s data root (`$TMPDIR/h2boot-data` by default) is where
        // the settings land, as `<data>/settings.toml`.
        let data = std::env::temp_dir().join(format!("h2-rows-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let path = settings::path_in(&data);
        assert_eq!(path, data.join("settings.toml"));
        assert!(!path.exists());
        let load = || Arc::new(Store::load(ALL, Some(&path), &|_| None, &[]).0);

        let store = load();
        let speaker = SpeakerSetting::new(Arc::clone(&store));
        let model = video_screen::model(Arc::clone(&store), video_screen::Switches::ALL).unwrap();
        // Choosing the speaker row (row 6) on Controls toggles and saves.
        let row = model.row(List::Controls, speaker_row::ROW).unwrap();
        assert!(matches!(model.press(row), Chosen::Stepped(_)));
        assert!(!speaker.enabled());
        // On the Video screen each row steps and saves, keeping the rest.
        for r in 0..3 {
            let row = model.row(video_screen::LIST, r).unwrap();
            assert!(matches!(model.press(row), Chosen::Stepped(_)));
        }
        // A retail Controls row saves nothing new.
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(model.row(List::Controls, 5), None);
        assert_eq!(model.row(video_screen::LIST, 7), None);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        // The next run reads them all back.
        let next = load();
        assert_eq!(next.get(PAD_SPEAKER.key), Some(Value::Bool(false)));
        assert_eq!(next.get(FOV.key), Some(Value::Int(75)));
        assert_eq!(next.get(FPS.key), Some(Value::Int(60)));
        assert_eq!(next.get(RENDER_SCALE.key), Some(Value::Int(2)));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn install_returns_on_a_runtime_without_the_game() {
        // `resolve_original` inside `edit_dispatch` deadlocks on the dispatch
        // lock; run the install on its own thread so a regression fails the
        // test instead of hanging it.
        let rt = Arc::new(runtime::Runtime::new(Guest::new(), Dispatch::default()));
        let (tx, rx) = std::sync::mpsc::channel();
        let r = Arc::clone(&rt);
        std::thread::spawn(move || {
            let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
            let model = video_screen::model(store, video_screen::Switches::ALL).unwrap();
            let mut features = feature::Registry::new(feature::Switches::defaults());
            let ok = install(&r, &mut features, Some(Arc::new(model)), None).is_ok();
            let installed = features.build(&feature::Base).unwrap().install(&r).unwrap();
            tx.send((ok, installed.off_reason("locale").map(str::to_owned)))
                .ok();
        });
        let (ok, strings_off) = rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap();
        assert!(ok);
        assert_eq!(
            strings_off.as_deref(),
            Some("LOCALE_FIND has no generated original")
        );
        // Nothing generated, so every hook was skipped and ours own no entry.
        rt.edit_dispatch(|d| {
            assert!(d.override_name(ACTION_TABLE_BUILD).is_none());
            assert!(d.override_name(feature::points::LOCALE_FIND.base).is_none());
        });
    }
}
