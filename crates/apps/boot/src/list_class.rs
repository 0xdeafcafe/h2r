//! Our own list screen class (docs/engine/feature-engine.md "UI step b"): a screen
//! of the `ui` model with no retail function patched for it.
//!
//! **What it is.** A Controls-shaped screen (the Controls tag's layout,
//! built by the Controls constructor) whose screen and list handler point
//! at our copies of the Controls vtables, with these entries of ours (async
//! entries at arena addresses, docs/formats/ui.md "Screen and list classes"):
//!
//! - **loader**: what `0x149f1e` is handed. It does what the Controls loader
//!   does (allocate, construct, push), and between construct and push it
//!   adopts the screen: our vtables, our row chosen in the press record, and
//!   the rows trimmed to the model's.
//! - **row text** (list slot 20): the model's label and value for the item's
//!   row, and, for a class with a description, the focused row's
//!   description when the focus moved.
//! - **row chosen** (the press record's function): the model's press.
//! - **set up pane** (screen slot 18): the retail one, then our title on the
//!   header text widget and, for a class with a description, its text widget
//!   (made by the pane builder's own text step from a block of ours) and,
//!   for a class with a picture, a bitmap widget made by the pane builder's
//!   bitmap step from a copy of a retail screen's bitmap block.
//! - **loader slot** (screen slot 26): our loader, so the screen manager
//!   re-opens it as ours.
//!
//! Nothing here tells our screens apart from retail ones at run time: the
//! retail functions never see them. The rows' datum array is built at our
//! count: while our loader runs the Controls constructor, the datum array
//! constructor's hook (`layout_glue`, `0x1a480d`) asks [`array_count`].
//! Past six rows the list scrolls through the Controls list's six item
//! widgets.
//!
//! **In a game** the Controls tag (screen 39,
//! `game_shell\settings_screen\player_profile\controller_settings`) is
//! not loaded: it is in `mainmenu` only. A screen built on it there has no
//! widgets, a blank screen (the pause menu's Video and Voice rows opened
//! one). So while the UI's game kind ([`UI_GAME_KIND`]) is not the main
//! menu, the loader runs the Controls constructor's own steps on screen
//! 200's tag instead ([`construct_on`]): the in-game Controller Settings
//! (`mp_menu\controller_settings`), in `shared`, `single_player_shared`
//! and every level. Its list's items have one text each, so a class with
//! [`ListClass::lines`] draws each row as one line, label and value
//! ([`Rows::line`]), from text slots of its own.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use engine::DatumHandle;
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points::{self, CONTROLS_LIST_VTABLE, CONTROLS_SCREEN_VTABLE};
use engine::view::ui::{self, Item, Screen, list_slot, screen_slot};
use runtime::guest::Permissions;

use crate::ui::{Chosen, List, Model};

/// The Controls list's row chosen, which the press record holds until we
/// adopt the screen.
pub const CONTROLS_ROW_CHOSEN: u32 = 0x002c_8719;

/// Screen vtable slot 10: the event handler (thiscall, the event, `ret 4`;
/// the Controls one is the generic `0x2300ea`, which closes the screen on
/// B or Back and hands anything else to the children).
pub const SCREEN_EVENT_SLOT: u32 = 10;
/// An event's kind word (`+0`) for a button, and its button (`+8`): the
/// generic handler takes 1 (B) and 13 (Back); the squad settings screen
/// opens a dialog on 2 (X, `0x2bbed1`); 12 is Start in the same order (A,
/// B, X, Y, black, white, triggers, d-pad, Start, Back).
pub const BUTTON_EVENT: u32 = 5;
pub const X_BUTTON: u32 = 2;
pub const START_BUTTON: u32 = 12;
/// The buttons that run a class's action.
pub const ACTION_BUTTONS: [u32; 2] = [X_BUTTON, START_BUTTON];
/// The direction events' kind words: the controller update (`0x1907d6`,
/// through `0x190a3d`) raises 1 up, 2 left, 3 down and 4 right, from the
/// d-pad (button word 8 to 11) or the left stick past `0x7332` (button word
/// 16), first when pushed and then every 250 ms while held. The list
/// handler (`0x24c024`) moves its focus on 1 and 3 and hands 2 and 4 on.
/// Buttons (kind 5) are raised only on the frame a button goes down
/// (its held count is 1); holding one raises nothing more.
pub const UP_EVENT: u32 = 1;
pub const LEFT_EVENT: u32 = 2;
pub const DOWN_EVENT: u32 = 3;
pub const RIGHT_EVENT: u32 = 4;
/// Screen vtable slot 0: the destructor (thiscall, a flags word, `ret 4`),
/// which the stack calls when a closed screen's out transition ends, just
/// before it frees the memory (`0x235233`).
pub const SCREEN_DESTRUCTOR_SLOT: u32 = 0;

/// Told when a screen of the class is destroyed, with its address: async,
/// as an [`Action`] is, and may call the guest (the stack is freeing the
/// screen; its memory is still there).
pub type OnFree = Action;

/// A step of a screen's own, run on a press of a row whose press is
/// [`Chosen::Act`] or on X or Start ([`ACTION_BUTTONS`]): async, given the
/// screen.
pub type Action = Arc<
    dyn Fn(
            Cx,
            u32,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EntryError>> + Send>>
        + Send
        + Sync,
>;

/// Our entries and vtable copies, once built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Entries {
    pub loader: u32,
    pub row_text: u32,
    pub row_chosen: u32,
    pub setup_pane: u32,
    pub loader_slot: u32,
    /// Screen slot 10 of ours (a class with an action or a sideways step),
    /// else 0.
    pub event: u32,
    /// Screen slot 0 of ours (a class told of frees), else 0.
    pub destructor: u32,
    /// Our copy of the Controls screen vtable, then of the list's.
    pub screen_vtable: u32,
    pub list_vtable: u32,
}

/// What a screen of our class shows. Every call is made with no guest
/// memory held (the settings store may be read or written).
pub trait Rows: Send + Sync {
    /// The title's string id.
    fn title(&self) -> u32;
    /// The rows when the screen opens.
    fn count(&self) -> usize;
    /// Reserve room for a list that starts with one waiting row and grows
    /// in place when its contents arrive. Existing row handles stay valid.
    fn growing_capacity(&self) -> Option<usize> {
        None
    }
    /// `(text index, string id)` for each text of row `row`.
    fn texts(&self, row: i16) -> Vec<(u32, u32)>;
    /// A reserved list can remove trailing rows when its filter changes.
    fn shrink_growing_rows(&self) -> bool {
        false
    }
    /// A press on row `row`.
    fn press(&self, row: i16) -> Chosen;
    /// The description's string id for row `row`, for a class with a
    /// description text ([`ListClass::description`]).
    fn description(&self, _row: i16) -> Option<u32> {
        None
    }
    /// A focus- or state-dependent footer slot, refreshed with the description.
    fn focused_footer(&self, _row: Option<i16>) -> Option<u32> {
        None
    }
    /// The picture's tint (red, green, blue multipliers) for row `row`, for
    /// a class with a picture ([`ListClass::picture`]); `None` leaves it.
    fn tint(&self, _row: i16) -> Option<[f32; 3]> {
        None
    }
    /// The picture's bitmap for row `row`, a `bitm` datum handle of the
    /// loaded map, for a class with a picture ([`ListClass::picture`]);
    /// `None` leaves it. The bitmap widget reads its block's reference
    /// (`[widget + 0x70] + 0x1c`) at each draw (`0x2b05bb`, `0x2b0b26`),
    /// so writing it there changes the picture, as the level select's
    /// picture changes with its focused level.
    fn picture(&self, _row: i16) -> Option<u32> {
        None
    }
    /// Restore the picture block's original bitmap when a row has no picture.
    fn restore_missing_picture(&self) -> bool {
        false
    }
    /// The image of its bitmap the picture shows, set once as the screen
    /// opens ([`set_image`]), for a class with a picture
    /// ([`ListClass::picture`]): a page's own icon in Quick Options'
    /// game type picture (`crate::ui_kit::dialog::QUICK_PICTURE`). The
    /// widget's constructor starts it at image 0. `None` leaves it.
    fn picture_image(&self) -> Option<u16> {
        None
    }
    /// Text that changes while the screen is up, asked at each row text:
    /// bytes to write to guest memory first (the strings the ids this
    /// class shows are served from), and whether the description must be
    /// set again though the focus did not move.
    fn live_text(&self) -> LiveText {
        LiveText::default()
    }
    /// Left (`right` false) or right on row `row`, for a class with
    /// sideways steps ([`ListClass::sideways`]): what to log, `None` when
    /// the row has nothing to step.
    fn sideways(&self, _row: i16, _right: bool) -> Option<String> {
        None
    }
    /// Writes the texts that change at run time (`crate::coop_live`'s) into
    /// guest memory, before a row's texts are set and when the screen
    /// opens. Most screens' texts are written once at install.
    fn write_texts(&self, _m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        Ok(())
    }
    /// For the log when the screen opens with `count` rows: its title and
    /// rows as text, with their string ids. Called with no guest memory
    /// held.
    fn describe(&self, _count: usize) -> Option<String> {
        None
    }
    /// Row `row` as one line (label and value), for an in-game screen
    /// whose items have one text ([`ListClass::lines`]); `None` draws
    /// [`Rows::texts`].
    fn line(&self, _row: i16) -> Option<String> {
        None
    }
    /// The string id of the class's panel text `i` ([`ListClass::panel`]):
    /// set when the screen opens and again whenever the description is;
    /// `None` leaves it empty.
    fn panel_text(&self, _i: usize) -> Option<u32> {
        None
    }
}

/// See [`Rows::live_text`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveText {
    pub writes: Vec<(u32, Vec<u8>)>,
    pub redraw: bool,
}

/// What a class's own event hook did with an event ([`ListClass::buttons`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Taken: the screen's children never see it.
    Handled,
    /// Goes on as before (B and Back close, the rest reach the list).
    Pass,
}

/// The UI's game kind byte (`0x14954a` switches on it): 0 a campaign game,
/// 1 a multiplayer game, [`MAIN_MENU_KIND`] the main menu.
pub const UI_GAME_KIND: u32 = 0x0054_d5a8;
pub const MAIN_MENU_KIND: u8 = 2;
/// The screen whose tag a screen of ours is built on: the Controls screen
/// in the main menu, the in-game Controller Settings in a game (module
/// docs).
pub const CONTROLS_SCREEN_ID: u32 = 39;
pub const INGAME_SCREEN_ID: u32 = 200;
/// The generic screen constructor (`0x230451`: screen in EAX, stack: the
/// screen id, the request's two words, the player, the list handler;
/// `ret 0x14`), and the Controls list's (`0x2c84b8`: list in ESI, the
/// player; `ret 4`). The Controls constructor (`0x2b7752`) is these two
/// with the screen vtable stored between.
pub const GENERIC_SCREEN_CTOR: u32 = 0x0023_0451;
pub const CONTROLS_LIST_CTOR: u32 = 0x002c_84b8;

/// The screen id a screen of ours is built on, for the UI's game kind.
pub fn base_screen_id(game_kind: u8) -> u32 {
    if game_kind == MAIN_MENU_KIND {
        CONTROLS_SCREEN_ID
    } else {
        INGAME_SCREEN_ID
    }
}

/// Whether the UI is in a game (not the main menu): screens of ours are
/// built on [`INGAME_SCREEN_ID`] there.
pub fn ingame(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(m.u8(UI_GAME_KIND)? != MAIN_MENU_KIND)
}

/// The Controls constructor's steps (`0x2b7752`) on screen `id`'s tag:
/// the generic screen constructor, the Controls screen vtable, the
/// Controls list constructor. With `id` 39 it is the Controls
/// constructor.
pub async fn construct_on(
    cx: &Cx,
    screen: u32,
    id: u32,
    a: u32,
    b: u32,
    player: u32,
) -> Result<(), EntryError> {
    let list = screen.wrapping_add(ui::LIST);
    cx.call(
        GuestCall::new(GENERIC_SCREEN_CTOR)
            .eax(screen)
            .edi(screen)
            .esi(list)
            .args([id, a, b, player, list]),
    )
    .await?;
    cx.write(|m| m.set_u32(screen, CONTROLS_SCREEN_VTABLE.base))?;
    cx.call(
        GuestCall::new(CONTROLS_LIST_CTOR)
            .esi(list)
            .edi(screen)
            .args([player]),
    )
    .await?;
    Ok(())
}

/// One line a row, for the in-game items' one text ([`ListClass::lines`]):
/// ids of the `ui lines` family, [`LINES_PER_CLASS`] a class, two a row
/// (a row's text changes id when it changes, so the widget sets it again),
/// UTF-8 in [`LINE_LEN`]-byte slots of the class's own memory.
const LINES: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("ui lines");
pub const LINES_PER_CLASS: u32 = 0x20;
pub const LINE_LEN: u32 = 0x80;
/// The classes with lines, in the order they took their ids.
static LINE_CLASSES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

/// Bytes to write at a guest address: a line's text and its NUL.
pub type LineWrite = (u32, Vec<u8>);

/// A class's line slots.
pub struct Lines {
    /// Its first id.
    first: u32,
    /// Its slots' guest memory (0 before build).
    at: AtomicU32,
    /// Each row's last text and which of its two ids shows it.
    shown: Mutex<Vec<Option<(String, bool)>>>,
}

impl Lines {
    /// The lines of class `id`; `None` once the family is used up.
    pub fn of(id: &'static str) -> Option<Lines> {
        let mut c = LINE_CLASSES.lock().unwrap_or_else(|e| e.into_inner());
        let n = match c.iter().position(|x| *x == id) {
            Some(n) => n,
            None => {
                c.push(id);
                c.len() - 1
            }
        } as u32;
        ((n + 1) * LINES_PER_CLASS <= LINES.len).then(|| Lines {
            first: LINES.base + n * LINES_PER_CLASS,
            at: AtomicU32::new(0),
            shown: Mutex::new(Vec::new()),
        })
    }
    /// Where id `id`'s text is, if it is one of the class's.
    pub fn address(&self, id: u32) -> Option<u32> {
        let at = self.at.load(Ordering::SeqCst);
        (at != 0 && id >= self.first && id - self.first < LINES_PER_CLASS)
            .then(|| at + (id - self.first) * LINE_LEN)
    }
    /// Row `row`'s line: the id to show and, when the text changed, the
    /// bytes to write at that id's slot. `None` past the class's rows.
    pub fn set(&self, row: i16, text: &str) -> Option<(u32, Option<LineWrite>)> {
        let r = usize::try_from(row).ok().filter(|r| *r < (LINES_PER_CLASS / 2) as usize)?;
        let mut shown = self.shown.lock().unwrap_or_else(|e| e.into_inner());
        if shown.len() <= r {
            shown.resize(r + 1, None);
        }
        let (flip, changed) = match &shown[r] {
            Some((t, f)) if t == text => (*f, false),
            Some((_, f)) => (!*f, true),
            None => (false, true),
        };
        shown[r] = Some((text.to_string(), flip));
        let id = self.first + 2 * r as u32 + u32::from(flip);
        let write = changed.then(|| {
            let mut cut = text.len().min(LINE_LEN as usize - 1);
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            let mut b = text.as_bytes()[..cut].to_vec();
            b.push(0);
            (self.address(id).unwrap_or(0), b)
        });
        Some((id, write))
    }
}

/// A class's own handling of every event, given its kind word and button
/// word ([`BUTTON_EVENT`] with a button, or a direction: [`UP_EVENT`],
/// [`LEFT_EVENT`], [`DOWN_EVENT`], [`RIGHT_EVENT`]), asked before the
/// action, the sideways steps and the Controls handler.
pub type Buttons = Arc<dyn Fn(u32, u32) -> Button + Send + Sync>;

/// [`ListClass::behaviour`].
pub struct Behaviour<'a> {
    pub rows: &'a Arc<dyn Rows>,
    pub hook: Option<&'a Buttons>,
    pub action: bool,
    pub action_buttons: &'static [u32],
    pub sideways: bool,
}

/// A picture of ours on the screen: a retail screen's bitmap block, copied
/// (docs/formats/ui.md "Bitmap block") and placed with its top-left at `at`. The
/// block is found at run time through the interface's screen list, so the
/// bitmap reference is the loaded map's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureSpec {
    /// The retail screen whose pane holds the bitmap (`wgit +4`).
    pub screen_id: u32,
    /// Its pane, and the bitmap's index in that pane's bitmaps.
    pub pane: u32,
    pub bitmap: u32,
    /// The image index the block must show, as a check that it is the one
    /// meant.
    pub image: i16,
    /// Top-left, `x, y` (Y up, origin at the screen centre).
    pub at: [i16; 2],
    /// The widget's animation (a 1-based index into the `wigl` animation
    /// collections, docs/formats/ui.md "Animations and transitions"): what
    /// its screen's in and out transitions play on it. Zero is none, and a
    /// widget with none stays fully drawn while its screen animates out:
    /// the skulls page's skull stayed over the difficulty screen's own
    /// coming back in, on B.
    pub animation: u16,
}

/// A bitmap block definition's size.
pub const BITMAP_BLOCK_SIZE: u32 = 0x38;
/// A pane definition's size; its bitmaps block (count, address) at `+0x24`.
pub const PANE_SIZE: u32 = 0x4c;
const PANE_BITMAPS: u32 = 0x24;
/// A `wgit`'s panes block (count, address).
pub const SCREEN_PANES: u32 = 0x20;
/// A bitmap block's animation, image index, top-left point and `bitm`
/// reference (group, then datum handle).
const BITMAP_ANIMATION: usize = 0x04;
const BITMAP_IMAGE: usize = 0x0a;
const BITMAP_POINT: usize = 0x0c;
const BITMAP_GROUP: usize = 0x18;
const BITMAP_HANDLE: usize = 0x1c;
/// A widget's tint: three floats the bitmap draw (`0x2b0565`) multiplies
/// into its colour with the alpha at `+0x64`; the widget header
/// (`0x22e27b`) sets them to 1.
pub const WIDGET_TINT: u32 = 0x28;

/// The bitmap block `spec` names, from the screen tag whose datum handle
/// is `handle` (`SCREEN_TAG_BY_ID`), ready for the pane builder: with
/// `spec.animation`, at `spec.at`. Says why when there is none.
pub fn picture_block(
    m: &dyn GuestMemory,
    handle: u32,
    spec: &PictureSpec,
) -> Result<[u8; BITMAP_BLOCK_SIZE as usize], String> {
    let fault = |e: MemFault| format!("unreadable ({e:?})");
    if handle == u32::MAX {
        return Err(format!(
            "screen {} is not in this interface",
            spec.screen_id
        ));
    }
    let entries = m.u32(points::TAG_ENTRIES.base).map_err(fault)?;
    let tag = m
        .u32(entries.wrapping_add((handle & 0xffff) * 0x10 + 8))
        .map_err(fault)?;
    let (panes, pane_at) = (
        m.u32(tag.wrapping_add(SCREEN_PANES)).map_err(fault)?,
        m.u32(tag.wrapping_add(SCREEN_PANES + 4)).map_err(fault)?,
    );
    if spec.pane >= panes {
        return Err(format!("screen {} has {panes} panes", spec.screen_id));
    }
    let pane = pane_at.wrapping_add(spec.pane * PANE_SIZE);
    let (bitmaps, bitmap_at) = (
        m.u32(pane.wrapping_add(PANE_BITMAPS)).map_err(fault)?,
        m.u32(pane.wrapping_add(PANE_BITMAPS + 4)).map_err(fault)?,
    );
    if spec.bitmap >= bitmaps {
        return Err(format!(
            "pane {} of screen {} has {bitmaps} bitmaps",
            spec.pane, spec.screen_id
        ));
    }
    let mut b = [0u8; BITMAP_BLOCK_SIZE as usize];
    m.read(
        bitmap_at.wrapping_add(spec.bitmap * BITMAP_BLOCK_SIZE),
        &mut b,
    )
    .map_err(fault)?;
    let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let image = i16::from_le_bytes([b[BITMAP_IMAGE], b[BITMAP_IMAGE + 1]]);
    if word(BITMAP_GROUP) != engine::tags::GroupTag::from_chars(*b"bitm").0
        || word(BITMAP_HANDLE) == u32::MAX
        || image != spec.image
    {
        return Err(format!(
            "bitmap {} of pane {} of screen {} is not the one meant (image {image})",
            spec.bitmap, spec.pane, spec.screen_id
        ));
    }
    b[BITMAP_ANIMATION..BITMAP_ANIMATION + 4].fill(0);
    b[BITMAP_ANIMATION..BITMAP_ANIMATION + 2].copy_from_slice(&spec.animation.to_le_bytes());
    b[BITMAP_POINT..BITMAP_POINT + 2].copy_from_slice(&spec.at[0].to_le_bytes());
    b[BITMAP_POINT + 2..BITMAP_POINT + 4].copy_from_slice(&spec.at[1].to_le_bytes());
    Ok(b)
}

/// The list of a screen of ours placed and skinned as another screen's
/// (docs/game/skulls.md "Layout"): our screens are built by the Controls
/// constructor, whose tag puts a full-width two-column list across the
/// screen. The screen-tag function (`0x22f871`, EAX the screen) hands our
/// screens a copy of that tag ([`copy_tag`]) whose one pane has the list
/// block moved and reskinned and only the frame art left of `bitmaps_left_of`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListLayout {
    /// The list skin, an index into the shared UI globals' skins
    /// (`wigl +0x138`; 0 is `ui\list_skins\default\default`).
    pub skin: i16,
    /// The first item's top-left, `x, y` (Y up, screen centre origin).
    pub anchor: [i16; 2],
    /// A retail bitmap of the pane is kept only when its top-left `x` is
    /// less than this: the picture column on the right stays clear.
    pub bitmaps_left_of: i16,
    /// Rows the list lays out before it scrolls (the list block's visible
    /// count, `+6`); 0 keeps the Controls tag's six. The layout (`0x24bc12`)
    /// places one item widget per row up to this count, and the Controls
    /// handler embeds six, so a class asking for more gets the rest as item
    /// widgets of ours ([`extra_items`], linked by the "add items" hook in
    /// `layout_glue`). With six the skulls page's list stopped a third of
    /// the way down the page.
    pub visible: i16,
}

/// Item widgets the Controls list handler embeds (`+0x88`, 0x80 bytes
/// each); a layout's rows past these are item widgets of ours.
pub const EMBEDDED_ITEMS: i16 = 6;

impl ListLayout {
    /// Item widgets of ours a list with this layout needs past the six
    /// embedded ones.
    pub const fn extra_items(&self) -> u32 {
        if self.visible > EMBEDDED_ITEMS {
            (self.visible - EMBEDDED_ITEMS) as u32
        } else {
            0
        }
    }
}

/// Top-left offsets from the list's animated anchor, in UI units.
#[derive(Clone, Copy)]
pub(crate) struct ScrollArrows {
    pub up: [i16; 2],
    pub down: [i16; 2],
}

/// The two calls to UI_QUAD_TO_SCREEN in the stock list draw (0x24bda2).
const SCROLL_UP_RETURN: u32 = 0x0024_beeb;
const SCROLL_DOWN_RETURN: u32 = 0x0024_bfed;

fn place_scroll_arrow(
    m: &mut dyn GuestMemory,
    cpu: &runtime::CpuAbi,
    arrows_for: impl FnOnce(u32) -> Option<ScrollArrows>,
) -> Result<(), MemFault> {
    let up = match m.u32(cpu.esp)? {
        SCROLL_UP_RETURN => true,
        SCROLL_DOWN_RETURN => false,
        _ => return Ok(()),
    };
    // The caller saved the list at EBP-0xc and its animated rectangle at
    // EBP-0x24. List layout stores the anchor at the left and upper edges
    // (rectangle words 1 and 2, respectively).
    let list = m.u32(cpu.ebp.wrapping_sub(0xc))?;
    let Some(arrows) = arrows_for(m.u32(list)?) else {
        return Ok(());
    };
    let [dx, dy] = if up { arrows.up } else { arrows.down };
    let word = |offset| m.u16(cpu.ebp.wrapping_sub(offset)).map(|v| f32::from(v as i16));
    // Keep the viewport adjustment the drawer made for split screen
    // (0x2363d4 writes X at EBP-8 and Y at EBP-4).
    let x = word(0x22)? + f32::from(dx) + word(8)?;
    let y = word(0x20)? + f32::from(dy) + word(4)?;
    let quad = cpu.esi;
    let read = |offset| m.u32(quad.wrapping_add(offset)).map(f32::from_bits);
    // The transform's float quad is left, right, top, bottom.
    let width = read(4)? - read(0)?;
    let height = read(8)? - read(12)?;
    let mut bytes = [0; 16];
    for (dst, value) in bytes.chunks_exact_mut(4).zip([x, x + width, y, y - height]) {
        dst.copy_from_slice(&value.to_le_bytes());
    }
    m.write(quad, &bytes)
}

pub(crate) struct ScrollArrowLayout;

impl hooks::Feature for ScrollArrowLayout {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("list-scroll-arrows", "Position scroll arrows on moved lists")
            .doc("docs/formats/ui.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw().before(&points::UI_QUAD_TO_SCREEN, "place scroll arrows", |c| {
            let mut mem = c.ctx.runtime().memory();
            place_scroll_arrow(&mut *mem, c.cpu, |vtable| {
                LAYOUTS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|layout| layout.list_vtable == vtable)
                    .and_then(|layout| layout.scroll_arrows)
            })
            .map_err(|m| runtime::GuestFault {
                addr: m.addr,
                len: m.len as usize,
                access: if m.write {
                    runtime::guest::Access::Write
                } else {
                    runtime::guest::Access::Read
                },
                reason: runtime::guest::FaultReason::Unmapped,
            })?;
            Ok(hooks::Flow::Continue)
        })
        // It reads the layouts fixed when the lists are set up and edits
        // only the caller's stack frame and the quad it is about to
        // transform: the pipelined render thread runs it too.
        .context_only();
        Ok(())
    }
}

/// Our classes' extra item widgets: by list vtable, how many each list
/// needs, the pool they come from, and which list holds which slot.
struct ExtraItems {
    list_vtable: u32,
    extra: u32,
    pool: u32,
    size: u32,
    slots: Vec<(u32, u32)>,
    next: u32,
}

static EXTRA_ITEMS: Mutex<Vec<ExtraItems>> = Mutex::new(Vec::new());

/// Room for this many screens of a class at once (one closing as another
/// opens), each with its extra items.
const EXTRA_SCREENS: u32 = 2;

/// The item widgets of ours the list handler `handler` (whose vtable is
/// `list_vtable`) gets after its six, in order: empty for a list that is not
/// one of our classes' or needs none. A handler keeps its slot while it
/// lives; slots are reused in turn, as `speaker_row::extra_slot` does.
pub fn extra_items(handler: u32, list_vtable: u32) -> Vec<u32> {
    let mut v = EXTRA_ITEMS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(e) = v.iter_mut().find(|e| e.list_vtable == list_vtable) else {
        return Vec::new();
    };
    let slot = crate::speaker_row::extra_slot(
        &mut e.slots,
        &mut e.next,
        handler,
        e.pool,
        e.size,
        e.extra,
    );
    (0..e.extra)
        .map(|i| slot + i * crate::speaker_row::ITEM_SIZE)
        .collect()
}

/// A `wgit` root's size, and its panes block (count, address).
pub const SCREEN_ROOT_SIZE: u32 = 0x68;
/// A pane's lists block (count, address), and a list block's size, skin
/// (`i16`) and anchor (point).
const PANE_LISTS: u32 = 0x0c;
pub const LIST_BLOCK_SIZE: u32 = 0x18;
const LIST_SKIN: u32 = 0x04;
const LIST_VISIBLE: u32 = 0x06;
const LIST_ANCHOR: u32 = 0x08;
/// Frame bitmaps kept at most.
const KEPT_BITMAPS: u32 = 4;
/// Where a class's tag copy goes in its table page, and the word that says
/// which retail tag it was made from.
const TAG_COPY: u32 = 0x200;
const TAG_SOURCE: u32 = 0x1fc;
/// The copy's size: root, one pane, one list, the kept bitmaps.
pub const TAG_COPY_SIZE: u32 =
    SCREEN_ROOT_SIZE + PANE_SIZE + LIST_BLOCK_SIZE + KEPT_BITMAPS * BITMAP_BLOCK_SIZE;
const _: () = assert!(TAG_SOURCE + 4 <= TAG_COPY && TAG_COPY + TAG_COPY_SIZE <= 0x1000);
const _: () = assert!(FRAME_BLOCK + BITMAP_BLOCK_SIZE <= TAG_SOURCE);

/// Writes at `at` a copy of the retail screen tag at `retail` with one pane
/// (its pane 0): the first list block moved to `layout.anchor` on skin
/// `layout.skin`, and only the bitmaps left of `layout.bitmaps_left_of`.
/// Text blocks and the rest are the retail ones. `None` when the tag has no
/// pane (left as it is).
pub fn copy_tag(
    m: &mut dyn GuestMemory,
    retail: u32,
    at: u32,
    layout: &ListLayout,
) -> Result<Option<u32>, MemFault> {
    let (panes, pane_from) = (
        m.u32(retail + SCREEN_PANES)?,
        m.u32(retail + SCREEN_PANES + 4)?,
    );
    if panes == 0 {
        return Ok(None);
    }
    let copy = |m: &mut dyn GuestMemory, from: u32, to: u32, len: u32| -> Result<(), MemFault> {
        let mut b = vec![0u8; len as usize];
        m.read(from, &mut b)?;
        m.write(to, &b)
    };
    let pane = at + SCREEN_ROOT_SIZE;
    let list = pane + PANE_SIZE;
    let bitmaps = list + LIST_BLOCK_SIZE;
    copy(m, retail, at, SCREEN_ROOT_SIZE)?;
    m.set_u32(at + SCREEN_PANES, 1)?;
    m.set_u32(at + SCREEN_PANES + 4, pane)?;
    copy(m, pane_from, pane, PANE_SIZE)?;
    let (lists, list_from) = (
        m.u32(pane_from + PANE_LISTS)?,
        m.u32(pane_from + PANE_LISTS + 4)?,
    );
    if lists > 0 {
        copy(m, list_from, list, LIST_BLOCK_SIZE)?;
        m.set_u16(list + LIST_SKIN, layout.skin as u16)?;
        if layout.visible > 0 {
            m.set_u16(list + LIST_VISIBLE, layout.visible as u16)?;
        }
        m.set_u16(list + LIST_ANCHOR, layout.anchor[0] as u16)?;
        m.set_u16(list + LIST_ANCHOR + 2, layout.anchor[1] as u16)?;
        m.set_u32(pane + PANE_LISTS, 1)?;
        m.set_u32(pane + PANE_LISTS + 4, list)?;
    }
    let (n, from) = (
        m.u32(pane_from + PANE_BITMAPS)?,
        m.u32(pane_from + PANE_BITMAPS + 4)?,
    );
    let mut kept = 0;
    for i in 0..n.min(64) {
        let b = from + i * BITMAP_BLOCK_SIZE;
        let x = m.u16(b + BITMAP_POINT as u32)? as i16;
        if x < layout.bitmaps_left_of && kept < KEPT_BITMAPS {
            copy(m, b, bitmaps + kept * BITMAP_BLOCK_SIZE, BITMAP_BLOCK_SIZE)?;
            kept += 1;
        }
    }
    m.set_u32(pane + PANE_BITMAPS, kept)?;
    m.set_u32(pane + PANE_BITMAPS + 4, bitmaps)?;
    Ok(Some(at))
}

/// The screen vtable is also the base of the class's table page.
struct RegisteredLayout {
    screen: &'static str,
    screen_vtable: u32,
    list_vtable: u32,
    list: ListLayout,
    scroll_arrows: Option<ScrollArrows>,
}

static LAYOUTS: Mutex<Vec<RegisteredLayout>> = Mutex::new(Vec::new());

/// The kit screen using this layout vtable, for menu inspection.
pub(crate) fn layout_screen(vtable: u32) -> Option<&'static str> {
    LAYOUTS.lock().unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|layout| layout.screen_vtable == vtable)
        .map(|layout| layout.screen)
}

/// Whether `vtable` is the screen vtable of a class with a layout.
pub fn has_layout(vtable: u32) -> bool {
    crate::ui_kit::dialog_tag::by_vtable(vtable).is_some() || layout_screen(vtable).is_some()
}

/// What the screen-tag function returns for `screen`, given that it found
/// `retail` (0 for none): a dialog class's copy of its base dialog
/// whatever was found ([`crate::ui_kit::dialog_tag::served`]), our copy
/// of `retail` for a screen of a class with a layout (made once per
/// retail tag, made again when a map load moves it), else `None`.
pub fn served_tag(
    m: &mut dyn GuestMemory,
    screen: u32,
    retail: u32,
) -> Result<Option<u32>, MemFault> {
    let vtable = m.u32(screen)?;
    if let Some(tag) = crate::ui_kit::dialog_tag::served(m, vtable)? {
        return Ok(Some(tag));
    }
    if retail == 0 {
        return Ok(None);
    }
    let Some(layout) = LAYOUTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|layout| layout.screen_vtable == vtable)
        .map(|layout| layout.list)
    else {
        return Ok(None);
    };
    let at = vtable + TAG_COPY;
    if m.u32(vtable + TAG_SOURCE)? == retail {
        return Ok(Some(at));
    }
    let made = copy_tag(m, retail, at, &layout)?;
    if made.is_some() {
        m.set_u32(vtable + TAG_SOURCE, retail)?;
    }
    Ok(made)
}

/// Sets widget `widget`'s tint.
pub fn set_tint(m: &mut dyn GuestMemory, widget: u32, tint: [f32; 3]) -> Result<(), MemFault> {
    for (i, c) in tint.iter().enumerate() {
        m.set_u32(widget + WIDGET_TINT + 4 * i as u32, c.to_bits())?;
    }
    Ok(())
}

/// A bitmap widget's block definition (`+0x70`, set by its constructor
/// `0x2b01eb`).
pub const WIDGET_BLOCK: u32 = 0x70;
/// A bitmap widget's image index (`0x2b0a14` stores it; the constructor
/// `0x2b01eb` zeroes it, whatever its block says).
pub const WIDGET_IMAGE: u32 = 0x88;
/// A `bitm` tag's image count (`0x2b0a14` checks the index against it).
pub const BITMAP_IMAGES: u32 = 0x44;

/// Sets bitmap widget `widget`'s image to `index`, as `0x2b0a14` does:
/// only when its bitmap is loaded and has that many images. Whether it
/// did.
pub fn set_image(m: &mut dyn GuestMemory, widget: u32, index: u16) -> Result<bool, MemFault> {
    let block = m.u32(widget.wrapping_add(WIDGET_BLOCK))?;
    if block == 0 {
        return Ok(false);
    }
    let datum = m.u32(block.wrapping_add(BITMAP_HANDLE as u32))?;
    if datum == u32::MAX {
        return Ok(false);
    }
    let entries = m.u32(points::TAG_ENTRIES.base)?;
    let tag = m.u32(entries.wrapping_add((datum & 0xffff) * 0x10 + 8))?;
    if tag == 0 || u32::from(index) >= m.u32(tag.wrapping_add(BITMAP_IMAGES))? {
        return Ok(false);
    }
    m.set_u16(widget.wrapping_add(WIDGET_IMAGE), index)?;
    Ok(true)
}

/// Points bitmap widget `widget` at `bitm` datum `datum`: its block's
/// reference ([`BITMAP_HANDLE`]), which the draw reads each frame. The
/// block is our copy ([`picture_block`]), never a tag's.
pub fn set_picture(m: &mut dyn GuestMemory, widget: u32, datum: u32) -> Result<(), MemFault> {
    let block = m.u32(widget + WIDGET_BLOCK)?;
    m.set_u32(block + BITMAP_HANDLE as u32, datum)
}

/// A description text of ours on the screen: a text block definition
/// (docs/formats/ui.md "Text block") the pane builder's text step makes a widget
/// from. Its string starts empty; row text sets the focused row's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextBlock {
    /// Top, left, bottom, right (Y up, origin at the screen centre).
    pub rect: [i16; 4],
    pub font: i16,
    /// Red, green, blue.
    pub colour: [f32; 3],
    /// The widget's animation, as [`PictureSpec::animation`]: zero leaves
    /// the text fully drawn through its screen's out transition.
    pub animation: u16,
}

/// A text block definition's size.
pub const TEXT_BLOCK_SIZE: u32 = 0x2c;

impl TextBlock {
    /// The block's bytes: left-justified (flags bit 0), the larger text
    /// widget (bit 4 clear), its animation, the null string id.
    pub fn bytes(&self) -> [u8; TEXT_BLOCK_SIZE as usize] {
        let mut b = [0u8; TEXT_BLOCK_SIZE as usize];
        b[0..4].copy_from_slice(&1u32.to_le_bytes());
        b[0x4..0x6].copy_from_slice(&self.animation.to_le_bytes());
        b[0xa..0xc].copy_from_slice(&self.font.to_le_bytes());
        for (i, c) in self.colour.iter().enumerate() {
            b[0x10 + 4 * i..0x14 + 4 * i].copy_from_slice(&c.to_le_bytes());
        }
        for (i, v) in self.rect.iter().enumerate() {
            b[0x1c + 2 * i..0x1e + 2 * i].copy_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// A text of a class's own that belongs to no row (a page's body, a
/// table's label or value, a checklist column; [`ListClass::panel`]): a
/// text block definition like [`TextBlock`]'s, right-justified or not and
/// with the retail block's render depth bias, its string set by
/// [`Rows::panel_text`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelText {
    pub block: TextBlock,
    /// Right-justified (flags bit 1), else left (bit 0).
    pub right: bool,
    /// The block's render depth bias (`+0x28`).
    pub bias: i16,
}

impl PanelText {
    /// The block's bytes, as [`TextBlock::bytes`] with its justification
    /// and bias.
    pub fn bytes(&self) -> [u8; TEXT_BLOCK_SIZE as usize] {
        let mut b = self.block.bytes();
        let flags: u32 = if self.right { 2 } else { 1 };
        b[0..4].copy_from_slice(&flags.to_le_bytes());
        b[0x28..0x2a].copy_from_slice(&self.bias.to_le_bytes());
        b
    }
}

/// The description widget of each open screen of a class and the row it
/// shows, so row text sets it only when the focus moved.
#[derive(Default)]
pub struct Shown(Mutex<Vec<ShownEntry>>);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShownEntry {
    screen: u32,
    widget: u32,
    picture: Option<(u32, u32)>, // widget and original bitmap datum
    row: Option<i16>,
    /// The panel texts' widgets, in [`ListClass::panel`]'s order (0 for
    /// one that could not be made).
    panel: Vec<u32>,
}

/// Screens remembered at once; a screen is set up again before it draws, so
/// an older entry is never needed.
const SHOWN_KEPT: usize = 8;

impl Shown {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ShownEntry>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Screen `screen`'s description widget is `widget`, showing nothing.
    pub fn set_widget(&self, screen: u32, widget: u32) {
        let mut v = self.lock();
        v.retain(|e| e.screen != screen);
        v.push(ShownEntry {
            screen,
            widget,
            picture: None,
            row: None,
            panel: Vec::new(),
        });
        if v.len() > SHOWN_KEPT {
            v.remove(0);
        }
    }
    /// Screen `screen`'s picture widget is `widget` (after its description
    /// widget was set).
    fn set_picture(&self, screen: u32, widget: u32, original: u32) {
        if let Some(e) = self.lock().iter_mut().find(|e| e.screen == screen) {
            e.picture = Some((widget, original));
        }
    }
    /// Screen `screen`'s panel text widgets are `widgets`.
    pub fn set_panel(&self, screen: u32, widgets: Vec<u32>) {
        if let Some(e) = self.lock().iter_mut().find(|e| e.screen == screen) {
            e.panel = widgets;
        }
    }
    /// Screen `screen`'s panel text widgets.
    pub fn panel(&self, screen: u32) -> Vec<u32> {
        self.lock()
            .iter()
            .find(|e| e.screen == screen)
            .map(|e| e.panel.clone())
            .unwrap_or_default()
    }
    /// Screen `screen`'s picture widget.
    pub fn picture(&self, screen: u32) -> Option<u32> {
        self.picture_block(screen).map(|(widget, _)| widget)
    }
    fn picture_block(&self, screen: u32) -> Option<(u32, u32)> {
        self.lock()
            .iter()
            .find(|e| e.screen == screen)
            .and_then(|e| e.picture)
    }
    /// Screen `screen`'s description widget.
    pub fn widget(&self, screen: u32) -> Option<u32> {
        self.lock()
            .iter()
            .find(|e| e.screen == screen)
            .map(|e| e.widget)
    }
    /// Row `row` is focused on `screen`: its description widget when that is
    /// another row than it shows (it shows `row` from now), else `None`.
    pub fn focus(&self, screen: u32, row: Option<i16>) -> Option<u32> {
        let mut v = self.lock();
        let e = v.iter_mut().find(|e| e.screen == screen)?;
        if e.row == row {
            return None;
        }
        e.row = row;
        Some(e.widget)
    }
}

/// Where the description's text block goes in the class's table page.
const DESCRIPTION_BLOCK: u32 = 0x100;
/// Where the picture's bitmap block goes, after the text block.
const PICTURE_BLOCK: u32 = 0x140;
const _: () = assert!(PICTURE_BLOCK >= DESCRIPTION_BLOCK + TEXT_BLOCK_SIZE);
/// Where the frame's bitmap block goes, after the picture's.
const FRAME_BLOCK: u32 = PICTURE_BLOCK + BITMAP_BLOCK_SIZE;
/// Where [`push`] builds its request (0x20 bytes), at the end of the page.
const PUSH_BLOCK: u32 = 0x1000 - 0x20;
const _: () = assert!(PUSH_BLOCK >= FRAME_BLOCK + BITMAP_BLOCK_SIZE);
/// Where a class's panel art blocks go ([`ListClass::panel`]), then its
/// text blocks: past the tag copy, before the push request.
const PANEL_BLOCKS: u32 = 0x400;
const _: () = assert!(PANEL_BLOCKS >= TAG_COPY + TAG_COPY_SIZE);
/// Panel art and texts a class has at most.
pub const MAX_PANEL_ART: usize = 4;
pub const MAX_PANEL_TEXTS: usize = 40;
const PANEL_TEXT_BLOCKS: u32 = PANEL_BLOCKS + MAX_PANEL_ART as u32 * BITMAP_BLOCK_SIZE;
const _: () = assert!(PANEL_TEXT_BLOCKS + MAX_PANEL_TEXTS as u32 * TEXT_BLOCK_SIZE <= PUSH_BLOCK);

/// Where a class's description block goes, the block, and where to keep
/// each screen's widget.
#[derive(Clone)]
pub struct Description {
    pub at: u32,
    pub block: TextBlock,
    pub shown: Arc<Shown>,
    /// A picture beside it, its block written at `picture_at`.
    pub picture: Option<PictureSpec>,
    pub picture_at: u32,
    /// Frame art of another retail screen for the list, its block written
    /// at `frame_at` ([`ListClass::frame`]).
    pub frame: Option<PictureSpec>,
    pub frame_at: u32,
    /// The class's panel ([`ListClass::panel`]): its art, its texts, the
    /// rows that name the texts' strings, and where their blocks go.
    pub panel: Panel,
    pub panel_at: u32,
    pub rows: Option<Arc<dyn Rows>>,
}

/// A class's own art and texts that belong to no row ([`ListClass::panel`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Panel {
    /// Retail bitmap blocks, made widgets before the texts (backings).
    pub art: Vec<PictureSpec>,
    pub texts: Vec<PanelText>,
}

/// A screen of the `ui` model.
pub struct ModelRows {
    pub model: Arc<Model>,
    pub screen: &'static str,
}

impl Rows for ModelRows {
    fn title(&self) -> u32 {
        self.model.title_id(self.screen).unwrap_or(0)
    }
    fn count(&self) -> usize {
        self.model.rows(List::Screen(self.screen)).len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        self.model
            .row(List::Screen(self.screen), row)
            .map(|r| self.model.texts(r).to_vec())
            .unwrap_or_default()
    }
    fn press(&self, row: i16) -> Chosen {
        self.model
            .row(List::Screen(self.screen), row)
            .map_or(Chosen::Nothing, |r| self.model.press(r))
    }
    fn line(&self, row: i16) -> Option<String> {
        self.model
            .row(List::Screen(self.screen), row)
            .map(|r| self.model.line(r))
    }
}

/// The loaders of the classes built so far, by screen id, so an `Open` row
/// (on a screen of ours or on a retail list) finds the screen it names.
static LOADERS: Mutex<BTreeMap<&'static str, u32>> = Mutex::new(BTreeMap::new());

/// The loader of our screen `screen`, once its class is built.
pub fn loader_of(screen: &str) -> Option<u32> {
    LOADERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(screen)
        .copied()
}

/// The rows our loader is building a screen with (0: no loader of ours is
/// running the Controls constructor).
static BUILDING: AtomicU32 = AtomicU32::new(0);

/// The count the datum array named `name` is built with, `count` being what
/// the game asks for: while one of our loaders runs the Controls
/// constructor, the Controls rows' array gets the screen's row count. No
/// list destructor frees the array at `list + 0x70` (`0x2b7563`, `0x2376c8`,
/// `0x1474e8`, `0x22e34b` read 2026-11-16), so it is sized when it is made
/// rather than replaced after.
pub fn array_count(name: u32, count: u32) -> u32 {
    match BUILDING.load(Ordering::SeqCst) {
        n if n != 0 && name == crate::speaker_row::LIST_NAME => n,
        _ => count,
    }
}

/// Why a screen class of ours can't be snapshotted now: one of our loaders
/// is running the Controls constructor ([`BUILDING`]). The rest is fixed at
/// build (`LAYOUTS`, `LOADERS`), parked in continuations the machine check
/// reports (the async entries), or set up again before a screen draws
/// (`Shown`).
pub fn building_live() -> Option<String> {
    building_reason(BUILDING.load(Ordering::SeqCst))
}

fn building_reason(building: u32) -> Option<String> {
    (building != 0).then(|| "one of our screens is being built".into())
}

/// Clears [`BUILDING`] when the constructor call ends, however it ends.
struct Building;

impl Building {
    fn start(rows: usize) -> Self {
        BUILDING.store(u32::try_from(rows).unwrap_or(0), Ordering::SeqCst);
        Building
    }
}

impl Drop for Building {
    fn drop(&mut self) {
        BUILDING.store(0, Ordering::SeqCst);
    }
}

/// A screen of ours as our own list class. See the module docs.
pub struct ListClass {
    /// The feature id (`features.<id>` switches it).
    pub id: &'static str,
    /// The screen id an `Open` row names.
    pub screen: &'static str,
    rows: Arc<dyn Rows>,
    description: Option<TextBlock>,
    picture: Option<PictureSpec>,
    frame: Option<PictureSpec>,
    layout: Option<ListLayout>,
    scroll_arrows: Option<ScrollArrows>,
    action: Option<Action>,
    buttons: Option<Buttons>,
    sideways: bool,
    on_free: Option<OnFree>,
    pane_ready: Option<Action>,
    footer: Option<u32>,
    action_buttons: &'static [u32],
    lines: bool,
    loader: Arc<AtomicU32>,
    dialog: Option<crate::ui_kit::dialog::DialogBase>,
    panel: Panel,
}

impl ListClass {
    /// Attach dynamic widgets once the retail pane and its children exist.
    pub fn pane_ready(mut self, action: Action) -> Self {
        self.pane_ready = Some(action);
        self
    }
    /// The model's screen `screen`.
    pub fn new(id: &'static str, screen: &'static str, model: Arc<Model>) -> Self {
        Self::with_rows(id, screen, Arc::new(ModelRows { model, screen }))
    }
    /// A screen whose rows come from elsewhere.
    pub fn with_rows(id: &'static str, screen: &'static str, rows: Arc<dyn Rows>) -> Self {
        ListClass {
            id,
            screen,
            rows,
            description: None,
            picture: None,
            frame: None,
            layout: None,
            scroll_arrows: None,
            action: None,
            buttons: None,
            sideways: false,
            on_free: None,
            pane_ready: None,
            footer: None,
            action_buttons: &ACTION_BUTTONS,
            lines: false,
            loader: Arc::new(AtomicU32::new(0)),
            dialog: None,
            panel: Panel::default(),
        }
    }
    /// Opened in a game too (the pause menu's Video and Voice): there its
    /// rows draw as one line each ([`Rows::line`], module docs).
    pub fn lines(mut self) -> Self {
        self.lines = true;
        self
    }
    /// With a description text: the focused row's [`Rows::description`].
    pub fn description(mut self, block: TextBlock) -> Self {
        self.description = Some(block);
        self
    }
    /// Shown as a dialog of rows built from the retail dialog `base`
    /// (docs/game/ui-builder.md "The Dialog kind"): its tag copied with
    /// the list placed by the class's layout (the base's own list when it
    /// has none), opened over the screen beneath on the dialogs' window,
    /// and closed by B itself ([`crate::ui_kit::dialog_tag`]).
    pub fn dialog(mut self, base: crate::ui_kit::dialog::DialogBase) -> Self {
        self.dialog = Some(base);
        self
    }
    /// The dialog base, if the class is a dialog.
    pub fn dialog_base(&self) -> Option<crate::ui_kit::dialog::DialogBase> {
        self.dialog
    }
    /// With a picture beside the description, tinted per focused row
    /// ([`Rows::tint`]). Only a class with a description shows one.
    pub fn picture(mut self, spec: PictureSpec) -> Self {
        self.picture = Some(spec);
        self
    }
    /// With frame art for its list: a retail screen's bitmap block placed
    /// as `spec` says, as [`ListClass::picture`]'s is, but never tinted.
    /// A moved list takes the frame of the screen whose layout it copies
    /// (the skulls page: the level select's bracket). Only a class with a
    /// description shows one.
    pub fn frame(mut self, spec: PictureSpec) -> Self {
        self.frame = Some(spec);
        self
    }
    /// Place the stock scroll arrows at these offsets. The list skin
    /// still supplies their bitmaps and the engine decides when to show them.
    pub(crate) fn scroll_arrows(mut self, arrows: ScrollArrows) -> Self {
        self.scroll_arrows = Some(arrows);
        self
    }
    /// With art and texts of its own that belong to no row: retail
    /// backings (made before the texts) and text blocks whose strings
    /// [`Rows::panel_text`] names, set when the screen opens and again
    /// with the description (a mod's page: docs/game/maps.md "A mod's
    /// page"). Only a class with a description shows them. At most
    /// [`MAX_PANEL_ART`] and [`MAX_PANEL_TEXTS`].
    pub fn panel(mut self, panel: Panel) -> Self {
        assert!(
            panel.art.len() <= MAX_PANEL_ART && panel.texts.len() <= MAX_PANEL_TEXTS,
            "{}: a panel of {} art and {} texts",
            self.id,
            panel.art.len(),
            panel.texts.len()
        );
        self.panel = panel;
        self
    }
    /// The class's panel.
    pub fn panel_of(&self) -> &Panel {
        &self.panel
    }
    /// With its list placed and skinned by `layout` ([`ListLayout`]).
    pub fn layout(mut self, layout: ListLayout) -> Self {
        self.layout = Some(layout);
        self
    }
    /// With an action: what a row whose press is [`Chosen::Act`] runs, and
    /// X and Start ([`ACTION_BUTTONS`]) too.
    pub fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }
    /// The buttons that run the action besides A on an action row:
    /// [`ACTION_BUTTONS`] (X and Start) unless set. The skulls page takes
    /// none: A on its Start row goes on, as the game's own menus do.
    pub fn action_buttons(mut self, buttons: &'static [u32]) -> Self {
        self.action_buttons = buttons;
        self
    }
    /// With buttons of its own, asked first on each button event (screen
    /// slot 10): the sign-in prompt's d-pad digits.
    pub fn buttons(mut self, buttons: Buttons) -> Self {
        self.buttons = Some(buttons);
        self
    }
    /// With sideways steps: left and right on the focused row go to
    /// [`Rows::sideways`] (the Controls list does nothing with them).
    /// The rows, the event hook, whether there is an action, the buttons
    /// that run it, and whether left and right step rows: what an event
    /// does, for the offline page ([`crate::ui_kit::page::ListPage`]).
    pub fn behaviour(&self) -> Behaviour<'_> {
        Behaviour {
            rows: &self.rows,
            hook: self.buttons.as_ref(),
            action: self.action.is_some(),
            action_buttons: self.action_buttons,
            sideways: self.sideways,
        }
    }

    pub fn sideways(mut self) -> Self {
        self.sideways = true;
        self
    }
    /// Told when a screen of the class is destroyed (after the Controls
    /// destructor has run, before the stack frees it).
    pub fn on_free(mut self, f: OnFree) -> Self {
        self.on_free = Some(f);
        self
    }
    /// With `id` (a string id) as the footer's button legend ([`FOOTER`]),
    /// the game's place for one, in place of the pane's own.
    pub fn footer(mut self, id: u32) -> Self {
        self.footer = Some(id);
        self
    }
    /// The loader address to open it with (`0x149f1e(mask, loader)`), once
    /// built; 0 before.
    pub fn loader(&self) -> u32 {
        self.loader.load(Ordering::SeqCst)
    }
}

/// Writes our vtables: the Controls ones as the game has them, with our
/// slots in. Once: a screen adopted later finds them written.
pub fn write_vtables(m: &mut dyn GuestMemory, e: &Entries) -> Result<(), MemFault> {
    if m.u32(e.screen_vtable)? != 0 {
        return Ok(());
    }
    for (from, to, n) in [
        (
            CONTROLS_SCREEN_VTABLE.base,
            e.screen_vtable,
            screen_slot::COUNT,
        ),
        (CONTROLS_LIST_VTABLE.base, e.list_vtable, list_slot::COUNT),
    ] {
        for i in 0..n {
            let v = ui::slot(m, from, i)?;
            m.set_u32(to + 4 * i, v)?;
        }
    }
    m.set_u32(e.screen_vtable + 4 * screen_slot::SETUP_PANE, e.setup_pane)?;
    m.set_u32(e.screen_vtable + 4 * screen_slot::LOADER, e.loader_slot)?;
    m.set_u32(e.list_vtable + 4 * list_slot::ROW_TEXT, e.row_text)?;
    if e.event != 0 {
        m.set_u32(e.screen_vtable + 4 * SCREEN_EVENT_SLOT, e.event)?;
    }
    if e.destructor != 0 {
        m.set_u32(e.screen_vtable + 4 * SCREEN_DESTRUCTOR_SLOT, e.destructor)?;
    }
    Ok(())
}

/// What adopting a freshly constructed Controls screen did.
#[derive(Debug, PartialEq, Eq)]
pub enum Adopted {
    /// Ours now, showing `rows` rows.
    Ours { rows: usize },
    /// Not what the Controls constructor builds; left as it is.
    NotControls(String),
}

/// Makes the Controls screen at `screen` ours: vtables, row chosen, and the
/// rows trimmed to `rows`. Rows cannot be added here: the array is built
/// with the count the loader asked for ([`array_count`]), and a screen
/// opened some other way keeps what its constructor made. The loaded byte
/// is set as the Controls loader sets it.
pub fn adopt(
    m: &mut dyn GuestMemory,
    screen: u32,
    e: &Entries,
    rows: usize,
) -> Result<Adopted, MemFault> {
    let s = Screen::at(screen);
    let list = s.list();
    let press = list.press(ui::CONTROLS_PRESS);
    let (sv, lv, pf, pt) = (
        s.vtable(m)?,
        list.vtable(m)?,
        press.function(m)?,
        press.target(m)?,
    );
    if sv != CONTROLS_SCREEN_VTABLE.base
        || lv != CONTROLS_LIST_VTABLE.base
        || pf != CONTROLS_ROW_CHOSEN
        || pt != list.addr
    {
        return Ok(Adopted::NotControls(format!(
            "screen vtable {sv:#x}, list vtable {lv:#x}, press {pf:#x} on {pt:#x}"
        )));
    }
    write_vtables(m, e)?;
    s.set_vtable(m, e.screen_vtable)?;
    list.set_vtable(m, e.list_vtable)?;
    press.set_function(m, e.row_chosen)?;
    let array = list.row_array(m)?;
    let live = array.live(m, i32::MAX)?;
    for (h, _) in live.iter().skip(rows) {
        array.array.delete(m, *h)?;
    }
    s.set_loaded(m, 1)?;
    Ok(Adopted::Ours {
        rows: rows.min(live.len()),
    })
}

/// Append rows to the reserved array without invalidating the focused handle.
fn grow_rows(m: &mut dyn GuestMemory, screen: Screen, count: usize) -> Result<(), MemFault> {
    let rows = screen.list().row_array(m)?.array;
    let active = rows.header_field(m, engine::datum::layout::ACTIVE_COUNT)?;
    for _ in active.max(0) as usize..count {
        if rows.new_datum(m)?.is_none() {
            break;
        }
    }
    Ok(())
}

/// The row an item shows.
fn item_row(m: &dyn GuestMemory, item: u32) -> Result<Option<i16>, MemFault> {
    Ok(Item::at(item)
        .row(m)?
        .map(|h: DatumHandle| h.index() as i16))
}

/// Says `what` on stderr once per `seen`.
fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

/// The loader: the Controls loader's steps with the adoption in between.
/// Stdcall, the request, `ret 4`; the screen in EAX (0 when allocation
/// failed, where the retail loader would write through null).
pub async fn load(
    cx: Cx,
    e: Entries,
    rows: Arc<dyn Rows>,
    name: &'static str,
) -> Result<Ret, EntryError> {
    // The rows are counted with no guest memory held.
    let count = rows.count();
    let described = rows.describe(count);
    cx.write(|m| rows.write_texts(m))?;
    let capacity = rows.growing_capacity().unwrap_or(count).max(count);
    let rows = count;
    let request = cx.arg(0)?;
    // A dialog's tag copy is made before its pane is, from its base found
    // in this interface.
    if let Some(d) = crate::ui_kit::dialog_tag::by_vtable(e.screen_vtable) {
        crate::ui_kit::dialog_tag::prepare(&cx, &d, name).await?;
    }
    let screen = cx
        .call(GuestCall::new(points::UI_ALLOC.base).args([ui::CONTROLS_SCREEN_SIZE]))
        .await?
        .eax;
    if screen == 0 {
        eprintln!("{name}: no memory for the screen; not opened");
        return Ok(Ret::new(0, 4));
    }
    let (a, b, player) = cx.read(|m| -> Result<_, MemFault> {
        Ok((
            m.u32(request + 4)?,
            m.u32(request + 8)?,
            u32::from(m.u16(request + 2)?),
        ))
    })?;
    // Unreadable (no game kind byte): the main menu's.
    let base = cx.read(|m| m.u8(UI_GAME_KIND).map_or(CONTROLS_SCREEN_ID, base_screen_id));
    {
        let _building = Building::start(capacity);
        if base == CONTROLS_SCREEN_ID {
            cx.call(
                GuestCall::new(points::CONTROLS_SCREEN_CTOR.base)
                    .edi(screen)
                    .args([a, b, player]),
            )
            .await?;
        } else {
            eprintln!("{name}: in a game: built on screen {base}'s tag");
            construct_on(&cx, screen, base, a, b, player).await?;
        }
    }
    match cx.write(|m| adopt(m, screen, &e, rows))? {
        Adopted::Ours { rows } => {
            eprintln!("{name}: opened with {rows} rows (our list class)");
            if let Some(d) = &described {
                eprintln!("{name}: {d}");
            }
        }
        Adopted::NotControls(why) => {
            eprintln!(
                "{name}: the Controls constructor built something else ({why}); shown as Controls"
            )
        }
    }
    cx.call(
        GuestCall::new(points::SCREEN_PUSH.base)
            .edi(screen)
            .ebx(request),
    )
    .await?;
    Ok(Ret::new(screen, 4))
}

/// Sets each `(text index, string id)` on the item widget `item`'s text
/// widget of that index (`ITEM_TEXT`, then `TEXT_SET`), as row text does.
pub async fn draw_texts(cx: &Cx, item: u32, texts: Vec<(u32, u32)>) -> Result<(), EntryError> {
    for (index, id) in texts {
        let text = cx
            .call(
                GuestCall::new(points::ITEM_TEXT.base)
                    .ecx(item)
                    .args([6, index, 0]),
            )
            .await?
            .eax;
        if text != 0 {
            cx.call(
                GuestCall::new(points::TEXT_SET.base)
                    .edi(text)
                    .esi(item)
                    .args([id]),
            )
            .await?;
        }
    }
    Ok(())
}

/// The player mask of a list callback whose first argument is the player
/// chain (`[[chain]]+4` the local player index): `1 << index`, or `None`
/// past the four local players.
pub fn player_mask(m: &dyn GuestMemory, chain: u32) -> Result<Option<u32>, MemFault> {
    let record = m.u32(chain)?;
    let index = m.u32(record.wrapping_add(4))?;
    Ok((index < 4).then(|| 1 << index))
}

/// Opens our screen `screen` for the player of `chain` (`0x149f1e(mask,
/// loader)`). Says why when it cannot.
pub async fn open(cx: &Cx, chain: u32, screen: &str) -> Result<(), EntryError> {
    // The value picker is `low_level_settings` in the main menu and the
    // large dialog in a game (`crate::ui_kit::picker`).
    let screen = if screen == crate::ui_kit::picker::NAME && cx.read(ingame).unwrap_or(false) {
        crate::ui_kit::picker::GAME_NAME
    } else {
        screen
    };
    let Some(loader) = loader_of(screen) else {
        eprintln!("ui: {screen} is not a screen of ours (or its class is off); not opened");
        return Ok(());
    };
    let Some(mask) = cx.read(|m| player_mask(m, chain))? else {
        eprintln!("ui: no local player for the press; {screen} not opened");
        return Ok(());
    };
    if let Some(d) = crate::ui_kit::dialog_tag::by_loader(loader) {
        eprintln!("ui: opening {screen} over the screen beneath (the dialogs' window)");
        return crate::ui_kit::dialog_tag::open_over(cx, &d, mask).await;
    }
    eprintln!("ui: opening {screen}");
    cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([mask, loader]))
        .await?;
    Ok(())
}

/// Opens our screen `screen` above `over` on `over`'s own window and slot,
/// for the player of `chain`: the loader is handed a request built as the
/// Game Setup dialog's own rows build theirs (window `+0x74`, slot `+0x78`;
/// `crate::skulls_mp::open_request`). Says why when it cannot.
pub async fn push(cx: &Cx, chain: u32, over: u32, screen: &str) -> Result<(), EntryError> {
    let Some(loader) = loader_of(screen) else {
        eprintln!("ui: {screen} is not a screen of ours (or its class is off); not opened");
        return Ok(());
    };
    let Some(mask) = cx.read(|m| player_mask(m, chain))? else {
        eprintln!("ui: no local player for the press; {screen} not opened");
        return Ok(());
    };
    if let Some(d) = crate::ui_kit::dialog_tag::by_loader(loader) {
        eprintln!("ui: opening {screen} over the screen beneath (the dialogs' window)");
        return crate::ui_kit::dialog_tag::open_over(cx, &d, mask).await;
    }
    let (window, slot) = crate::ui_kit::lifecycle::window_slot(cx, over)?;
    let request = crate::skulls_mp::open_request(mask as u16, window, slot, loader);
    let at = PUSH_REQUEST.load(Ordering::SeqCst);
    if at == 0 {
        eprintln!("ui: no request memory yet; {screen} not opened");
        return Ok(());
    }
    eprintln!("ui: opening {screen} on window {window}, slot {slot}");
    cx.write(|m| m.write(at, &request))?;
    cx.call(GuestCall::new(loader).args([at])).await?;
    Ok(())
}

/// Guest memory for [`push`]'s request (0x20 bytes), set at install
/// ([`set_push_request`]).
static PUSH_REQUEST: AtomicU32 = AtomicU32::new(0);

/// Where [`push`] builds its request: 0x20 bytes of guest memory of ours.
pub fn set_push_request(at: u32) {
    PUSH_REQUEST.store(at, Ordering::SeqCst);
}

/// Row text (list slot 20): thiscall on the list, the item and a word,
/// `ret 8`. Each of the row's texts goes to the item's text widget of that
/// index. With `shown` (a class with a description), the focused row's
/// description goes to the screen's description widget when the focus has
/// moved since the last draw.
pub async fn row_text(
    cx: Cx,
    rows: Arc<dyn Rows>,
    shown: Option<Arc<Shown>>,
    lines: Option<Arc<Lines>>,
) -> Result<Ret, EntryError> {
    let item = cx.arg(0)?;
    let live = rows.live_text();
    if !live.writes.is_empty() {
        cx.write(|m| -> Result<(), MemFault> {
            for (at, bytes) in &live.writes {
                m.write(*at, bytes)?;
            }
            Ok(())
        })?;
    }
    if live.redraw && rows.growing_capacity().is_some() {
        let count = rows.count();
        let screen = Screen::at(cx.entry().ecx.wrapping_sub(ui::LIST));
        cx.write(|m| grow_rows(m, screen, count))?;
        if rows.shrink_growing_rows() {
            let list = cx.entry().ecx;
            let focused = cx.call(GuestCall::new(points::LIST_FOCUSED_ROW.base).eax(list)).await?.eax;
            if focused != u32::MAX && usize::from(DatumHandle(focused).index()) >= count {
                // Clamp before removing the datum the list cursor refers to.
                cx.call(GuestCall::new(0x0024_c591).eax(list).args([count.saturating_sub(1) as u32])).await?;
            }
            cx.write(|m| {
                let rows = screen.list().row_array(m)?;
                for (handle, _) in rows.live(m, i32::MAX)?.into_iter().skip(count) {
                    rows.array.delete(m, handle)?;
                }
                Ok::<_, MemFault>(())
            })?;
        }
    }
    let row = cx.read(|m| item_row(m, item))?;
    // The rows read the settings store; no guest memory is held here.
    let texts = row.map(|r| rows.texts(r)).unwrap_or_default();
    // In a game the items have one text: the row as one line.
    let line = match (&lines, row) {
        (Some(l), Some(r)) if cx.read(ingame).unwrap_or(false) => {
            rows.line(r).and_then(|t| l.set(r, &t))
        }
        _ => None,
    };
    let texts = match line {
        Some((id, write)) => {
            if let Some((at, bytes)) = write.filter(|(at, _)| *at != 0) {
                cx.write(|m| m.write(at, &bytes))?;
            }
            vec![(0, id)]
        }
        None => texts,
    };
    cx.write(|m| rows.write_texts(m))?;
    draw_texts(&cx, item, texts).await?;
    if let Some(shown) = shown {
        let list = cx.entry().ecx;
        let screen = list.wrapping_sub(ui::LIST);
        let focused = cx
            .call(GuestCall::new(points::LIST_FOCUSED_ROW.base).eax(list))
            .await?
            .eax;
        let focused = (focused != u32::MAX).then(|| DatumHandle(focused).index() as i16);
        let moved = shown.focus(screen, focused);
        let widget = moved.or_else(|| live.redraw.then(|| shown.widget(screen)).flatten());
        if let Some(widget) = widget {
            // `TEXT_SET` copies the string as it sets it (`0x253b1a` reads
            // it through `0x230134` into the widget), so a slot of ours
            // set here is written before it is named.
            let description = focused.and_then(|r| rows.description(r));
            let footer = rows.focused_footer(focused);
            cx.write(|m| rows.write_texts(m))?;
            if let Some(id) = footer {
                cx.call(
                    GuestCall::new(points::TEXT_SET.base)
                        .edi(screen.wrapping_add(FOOTER))
                        .esi(screen)
                        .args([id]),
                )
                .await?;
            }
            set_panel_texts(&cx, screen, &shown, rows.as_ref()).await?;
            if let Some(id) = description {
                cx.call(
                    GuestCall::new(points::TEXT_SET.base)
                        .edi(widget)
                        .esi(screen)
                        .args([id]),
                )
                .await?;
            }
            if let (Some(picture), Some(tint), Some(_)) = (
                shown.picture(screen),
                focused.and_then(|r| rows.tint(r)),
                moved,
            ) {
                cx.write(|m| set_tint(m, picture, tint))?;
            }
            if let Some((picture, original)) = shown.picture_block(screen) {
                let datum = focused
                    .and_then(|r| rows.picture(r))
                    .or_else(|| rows.restore_missing_picture().then_some(original));
                if let Some(datum) = datum {
                    cx.write(|m| set_picture(m, picture, datum))?;
                }
            }
        }
    }
    Ok(Ret::new(0, 8))
}

/// Row chosen (the press record's function): thiscall, the player chain and
/// a pointer to the row, `ret 8`. A setting row steps; an open row opens
/// the screen of ours it names.
pub async fn row_chosen(
    cx: Cx,
    rows: Arc<dyn Rows>,
    action: Option<Action>,
) -> Result<Ret, EntryError> {
    let (chain, at) = (cx.arg(0)?, cx.arg(1)?);
    let row = cx.read(|m| m.u16(at))? as i16;
    // The store is written with no guest memory held.
    match rows.press(row) {
        Chosen::Stepped(msg) => eprintln!("{msg}"),
        Chosen::Open(id) => open(&cx, chain, id).await?,
        Chosen::Push(id) => {
            let screen = cx.entry().ecx.wrapping_sub(ui::LIST);
            push(&cx, chain, screen, id).await?
        }
        Chosen::Act => {
            if let Some(action) = action {
                let screen = cx.entry().ecx.wrapping_sub(ui::LIST);
                action(cx.clone(), screen).await?;
            }
        }
        Chosen::Retail {
            line,
            calls,
            loader,
        } => {
            eprintln!("{line}");
            open_retail(&cx, chain, &calls, loader).await?
        }
        Chosen::Nothing => {}
    }
    Ok(Ret::new(0, 8))
}

/// Where an event goes on a class of ours once its own hook passed it
/// ([`event_with`]); pure, so the offline page
/// ([`crate::ui_kit::page::ListPage`]) takes the same path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// X or Start (`action_buttons`): the class's action.
    Action,
    /// Left or right: the focused row's [`Rows::sideways`], and on to the
    /// Controls handler when it has nothing to step.
    Sideways { right: bool },
    /// The Controls handler: up and down move the focus, A presses the
    /// row, B and Back close.
    Controls,
}

/// [`Route`] for an event's kind and button words on a class with (or
/// without) an action, its action buttons, and sideways steps.
pub fn route(
    kind: u32,
    button: u32,
    action: bool,
    action_buttons: &[u32],
    sideways: bool,
) -> Route {
    if action && kind == BUTTON_EVENT && action_buttons.contains(&button) {
        Route::Action
    } else if sideways && (kind == LEFT_EVENT || kind == RIGHT_EVENT) {
        Route::Sideways {
            right: kind == RIGHT_EVENT,
        }
    } else {
        Route::Controls
    }
}

/// Screen slot 10 of a class with an action, sideways steps or an event
/// hook of its own: the hook first ([`Button::Handled`]: AL 1), then X or
/// Start runs the action, and left or right ([`LEFT_EVENT`],
/// [`RIGHT_EVENT`]) on the focused row its [`Rows::sideways`] (AL 1,
/// handled); every other event goes to `original`, the Controls class's
/// handler. Thiscall, the event, `ret 4`.
pub async fn event(
    cx: Cx,
    original: u32,
    action: Option<Action>,
    buttons: Option<Buttons>,
    sideways: Option<Arc<dyn Rows>>,
) -> Result<Ret, EntryError> {
    event_with(cx, original, action, &ACTION_BUTTONS, buttons, sideways).await
}

/// [`event`] with the buttons that run the action given.
pub async fn event_with(
    cx: Cx,
    original: u32,
    action: Option<Action>,
    action_buttons: &[u32],
    buttons: Option<Buttons>,
    sideways: Option<Arc<dyn Rows>>,
) -> Result<Ret, EntryError> {
    let screen = cx.entry().ecx;
    let ev = cx.arg(0)?;
    let (kind, button) =
        cx.read(|m| -> Result<_, MemFault> { Ok((m.u32(ev)?, m.u32(ev + 8)?)) })?;
    if let Some(buttons) = &buttons
        && buttons(kind, button) == Button::Handled
    {
        return Ok(Ret::new(1, 4));
    }
    // A screen whose window can't be read is no dialog of ours.
    if cx.read(|m| {
        crate::ui_kit::dialog_tag::closes_itself(m, screen, kind, button).unwrap_or(false)
    }) {
        crate::ui_kit::dialog_tag::close(&cx, screen).await?;
        return Ok(Ret::new(1, 4));
    }
    let routed = route(
        kind,
        button,
        action.is_some(),
        action_buttons,
        sideways.is_some(),
    );
    if let (Route::Action, Some(action)) = (routed, action) {
        action(cx.clone(), screen).await?;
        return Ok(Ret::new(1, 4));
    }
    if let (Route::Sideways { right }, Some(rows)) = (routed, sideways) {
        let focused = cx
            .call(GuestCall::new(points::LIST_FOCUSED_ROW.base).eax(screen.wrapping_add(ui::LIST)))
            .await?
            .eax;
        let row = (focused != u32::MAX).then(|| DatumHandle(focused).index() as i16);
        // The rows write the store with no guest memory held.
        if let Some(line) = row.and_then(|r| rows.sideways(r, right)) {
            eprintln!("{line}");
            return Ok(Ret::new(1, 4));
        }
    }
    let r = cx
        .call(GuestCall::new(original).ecx(screen).args([ev]))
        .await?;
    Ok(Ret::new(r.eax, 4))
}

/// The focused row of our screen `screen`, as the sideways steps read it
/// (`LIST_FOCUSED_ROW` on its list): for an [`Action`] that acts on the
/// focused row (the Mods list's X, `crate::mods_screen`).
pub async fn focused_row(cx: &Cx, screen: u32) -> Result<Option<i16>, EntryError> {
    let focused = cx
        .call(GuestCall::new(points::LIST_FOCUSED_ROW.base).eax(screen.wrapping_add(ui::LIST)))
        .await?
        .eax;
    Ok((focused != u32::MAX).then(|| DatumHandle(focused).index() as i16))
}

/// Screen slot 0 of a class told of frees: the Controls destructor
/// (`original`), then `on_free` with the screen. Thiscall, flags, `ret 4`.
pub async fn destructor(cx: Cx, original: u32, on_free: OnFree) -> Result<Ret, EntryError> {
    let screen = cx.entry().ecx;
    let flags = cx.arg(0)?;
    let r = cx
        .call(GuestCall::new(original).ecx(screen).args([flags]))
        .await?;
    on_free(cx.clone(), screen).await?;
    Ok(Ret::new(r.eax, 4))
}

/// Makes `calls` (stdcall, their arguments read now), then opens the retail
/// screen whose loader is `loader` for the pressing player.
pub async fn open_retail(
    cx: &Cx,
    chain: u32,
    calls: &[(u32, Vec<crate::ui::GuestArg>)],
    loader: u32,
) -> Result<(), EntryError> {
    let Some(mask) = cx.read(|m| player_mask(m, chain))? else {
        eprintln!("ui: no local player for the press; screen {loader:#x} not opened");
        return Ok(());
    };
    for (addr, args) in calls {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(match *a {
                crate::ui::GuestArg::Value(v) => v,
                crate::ui::GuestArg::Load(at) => cx.read(|m| m.u32(at))?,
            });
        }
        cx.call(GuestCall::new(*addr).args(values)).await?;
    }
    cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([mask, loader]))
        .await?;
    Ok(())
}

/// Screen slot 18: the retail pane set-up, then our title on the header,
/// then our description text widget (written at `description.at`, made by
/// the pane builder's text step). Thiscall, one argument, `ret 4`; its EAX
/// is the retail one's.
pub async fn setup_pane(
    cx: Cx,
    title: u32,
    description: Option<Description>,
    name: &'static str,
) -> Result<Ret, EntryError> {
    setup_pane_footer(cx, title, None, description, name).await
}

/// The screen's second built-in text widget, the button legend in the
/// footer band: made by the header build (`0x22ff53`, `lea ebx, [esi +
/// 0x338]`) in the header layout's second rectangle (`wigl +0x178`), its
/// string the pane's button key. Retail screens show "(A) SELECT  (B)
/// BACK" there, the glyphs U+E100 and U+E101.
pub const FOOTER: u32 = 0x338;

/// [`setup_pane`], then `footer` (a string id) on the footer legend.
pub async fn setup_pane_footer(
    cx: Cx,
    title: u32,
    footer: Option<u32>,
    description: Option<Description>,
    name: &'static str,
) -> Result<Ret, EntryError> {
    let screen = cx.entry().ecx;
    let arg = cx.arg(0)?;
    let r = cx
        .call(
            GuestCall::new(points::SCREEN_SETUP_PANE.base)
                .ecx(screen)
                .args([arg]),
        )
        .await?;
    cx.call(
        GuestCall::new(points::TEXT_SET.base)
            .edi(Screen::at(screen).header())
            .esi(screen)
            .args([title]),
    )
    .await?;
    if let Some(id) = footer {
        cx.call(
            GuestCall::new(points::TEXT_SET.base)
                .edi(screen.wrapping_add(FOOTER))
                .esi(screen)
                .args([id]),
        )
        .await?;
    }
    if let Some(d) = description {
        cx.write(|m| m.write(d.at, &d.block.bytes()))?;
        let widget = cx
            .call(
                GuestCall::new(points::PANE_TEXT_ADD.base)
                    .ebx(screen)
                    .args([0, d.at]),
            )
            .await?
            .eax;
        if widget == 0 {
            eprintln!("{name}: no description text (no memory for the widget)");
        } else {
            d.shown.set_widget(screen, widget);
            if let Some(spec) = d.picture {
                add_picture(&cx, screen, &spec, d.picture_at, Some(&d.shown), name).await?;
                let image = d.rows.as_ref().and_then(|r| r.picture_image());
                if let (Some(picture), Some(image)) = (d.shown.picture(screen), image) {
                    if !cx.write(|m| set_image(m, picture, image))? {
                        once(
                            &PICTURE_MISSING,
                            &format!("{name}: the picture has no image {image}"),
                        );
                    }
                }
            }
            if let Some(spec) = d.frame {
                add_picture(&cx, screen, &spec, d.frame_at, None, name).await?;
            }
            add_panel(&cx, screen, &d, name).await?;
        }
    }
    Ok(Ret::new(r.eax, 4))
}

/// The class's panel ([`ListClass::panel`]): its art as bitmap widgets
/// (backings first), then each text block a text widget with its string
/// set ([`Rows::panel_text`], the rows' texts written first).
async fn add_panel(cx: &Cx, screen: u32, d: &Description, name: &str) -> Result<(), EntryError> {
    if d.panel.art.is_empty() && d.panel.texts.is_empty() {
        return Ok(());
    }
    for (i, spec) in d.panel.art.iter().enumerate() {
        let at = d.panel_at + i as u32 * BITMAP_BLOCK_SIZE;
        add_picture(cx, screen, spec, at, None, name).await?;
    }
    if let Some(rows) = &d.rows {
        cx.write(|m| rows.write_texts(m))?;
    }
    let texts_at = d.panel_at + MAX_PANEL_ART as u32 * BITMAP_BLOCK_SIZE;
    let mut widgets = Vec::with_capacity(d.panel.texts.len());
    for (i, t) in d.panel.texts.iter().enumerate() {
        let at = texts_at + i as u32 * TEXT_BLOCK_SIZE;
        cx.write(|m| m.write(at, &t.bytes()))?;
        let widget = cx
            .call(
                GuestCall::new(points::PANE_TEXT_ADD.base)
                    .ebx(screen)
                    .args([0, at]),
            )
            .await?
            .eax;
        if widget == 0 {
            once(
                &PICTURE_MISSING,
                &format!("{name}: no panel text {i} (no memory for the widget)"),
            );
        }
        widgets.push(widget);
    }
    d.shown.set_panel(screen, widgets);
    if let Some(rows) = &d.rows {
        set_panel_texts(cx, screen, &d.shown, rows.as_ref()).await?;
    }
    Ok(())
}

/// Sets each panel text widget of `screen` to its string.
async fn set_panel_texts(
    cx: &Cx,
    screen: u32,
    shown: &Shown,
    rows: &dyn Rows,
) -> Result<(), EntryError> {
    let texts: Vec<(u32, u32)> = shown
        .panel(screen)
        .into_iter()
        .enumerate()
        .filter_map(|(i, widget)| rows.panel_text(i).filter(|_| widget != 0).map(|id| (widget, id)))
        .collect();
    // Written before `TEXT_SET` copies them (see `row_text`).
    if !texts.is_empty() {
        cx.write(|m| rows.write_texts(m))?;
    }
    for (widget, id) in texts {
        cx.call(
            GuestCall::new(points::TEXT_SET.base)
                .edi(widget)
                .esi(screen)
                .args([id]),
        )
        .await?;
    }
    Ok(())
}

/// Said once per run: the picture could not be made (the line says why).
static PICTURE_MISSING: AtomicBool = AtomicBool::new(false);

/// The picture (or the frame): its block found through the screen list,
/// copied to `at`, and made a bitmap widget of `screen` by the pane
/// builder's bitmap step; a picture's widget is kept in `shown` to tint.
async fn add_picture(
    cx: &Cx,
    screen: u32,
    spec: &PictureSpec,
    at: u32,
    shown: Option<&Shown>,
    name: &str,
) -> Result<(), EntryError> {
    let handle = cx
        .call(GuestCall::new(points::SCREEN_TAG_BY_ID.base).args([spec.screen_id]))
        .await?
        .eax;
    let block = match cx.read(|m| picture_block(m, handle, spec)) {
        Ok(b) => b,
        Err(why) => {
            once(&PICTURE_MISSING, &format!("{name}: no picture: {why}"));
            return Ok(());
        }
    };
    cx.write(|m| m.write(at, &block))?;
    let widget = cx
        .call(
            GuestCall::new(points::PANE_BITMAP_ADD.base)
                .esi(screen)
                .args([at]),
        )
        .await?
        .eax;
    if widget == 0 {
        once(
            &PICTURE_MISSING,
            &format!("{name}: no picture (no memory for the widget)"),
        );
    } else if let Some(shown) = shown {
        let original =
            u32::from_le_bytes(block[BITMAP_HANDLE..BITMAP_HANDLE + 4].try_into().unwrap());
        shown.set_picture(screen, widget, original);
    }
    Ok(())
}

impl hooks::Feature for ListClass {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(self.id, "A screen of ours as our own list class")
            .doc("docs/engine/feature-engine.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let rows = Arc::clone(&self.rows);
        let title = rows.title();
        let tables = b.guest_alloc(0x1000, Permissions::RW)?;
        // The request [`push`] builds lives in the class's own table page:
        // a page of its own, allocated only by the first class a process
        // ever built, moved every later allocation of that plan by a page
        // and left other plans pointing at memory they never allocated.
        set_push_request(tables + PUSH_BLOCK);
        let name = self.id;
        let shown = self.description.map(|_| Arc::new(Shown::default()));
        // The loader needs every address, so the entries are made first and
        // the loader reads them from a cell filled at the end.
        let cell = Arc::new(std::sync::OnceLock::<Entries>::new());
        let loader = {
            let cell = Arc::clone(&cell);
            let rows = Arc::clone(&rows);
            b.entry("loader", move |cx| -> EntryFuture {
                let e = *cell.get().expect("entries are set at build");
                Box::pin(load(cx, e, Arc::clone(&rows), name))
            })?
        };
        let lines = match self.lines.then(|| Lines::of(self.id)).flatten() {
            Some(l) => {
                let l = Arc::new(l);
                let at = b.guest_alloc(LINES_PER_CLASS * LINE_LEN, Permissions::RW)?;
                l.at.store(at, Ordering::SeqCst);
                let me = Arc::clone(&l);
                b.locale().source("ui lines", move |id| me.address(id));
                Some(l)
            }
            None => None,
        };
        let row_text_at = {
            let (rows, shown) = (Arc::clone(&rows), shown.clone());
            let seen = Arc::new(AtomicBool::new(false));
            b.entry("row text", move |cx| -> EntryFuture {
                once(&seen, &format!("{name}: row text"));
                Box::pin(row_text(cx, Arc::clone(&rows), shown.clone(), lines.clone()))
            })?
        };
        let row_chosen_at = {
            let (rows, action) = (Arc::clone(&rows), self.action.clone());
            b.entry("row chosen", move |cx| -> EntryFuture {
                Box::pin(row_chosen(cx, Arc::clone(&rows), action.clone()))
            })?
        };
        // The Controls class's event handler, read from its vtable when
        // first needed (the class's own slot replaces it in our copy).
        let sideways = self.sideways.then(|| Arc::clone(&rows));
        let event_at = if self.action.is_some()
            || sideways.is_some()
            || self.buttons.is_some()
            || self.dialog.is_some()
        {
            let (action, buttons) = (self.action.clone(), self.buttons.clone());
            let action_buttons = self.action_buttons;
            b.entry("event", move |cx| -> EntryFuture {
                let (action, buttons, sideways) =
                    (action.clone(), buttons.clone(), sideways.clone());
                Box::pin(async move {
                    let original =
                        cx.read(|m| ui::slot(m, CONTROLS_SCREEN_VTABLE.base, SCREEN_EVENT_SLOT))?;
                    event_with(cx, original, action, action_buttons, buttons, sideways).await
                })
            })?
        } else {
            0
        };
        let destructor_at = match self.on_free.clone() {
            Some(on_free) => b.entry("destructor", move |cx| -> EntryFuture {
                let on_free = Arc::clone(&on_free);
                Box::pin(async move {
                    let original = cx.read(|m| {
                        ui::slot(m, CONTROLS_SCREEN_VTABLE.base, SCREEN_DESTRUCTOR_SLOT)
                    })?;
                    destructor(cx, original, on_free).await
                })
            })?,
            None => 0,
        };

        let description = self
            .description
            .zip(shown)
            .map(|(block, shown)| Description {
                at: tables + DESCRIPTION_BLOCK,
                block,
                shown,
                picture: self.picture,
                picture_at: tables + PICTURE_BLOCK,
                frame: self.frame,
                frame_at: tables + FRAME_BLOCK,
                panel: self.panel.clone(),
                panel_at: tables + PANEL_BLOCKS,
                rows: Some(Arc::clone(&rows)),
            });
        let footer = self.footer;
        let pane_ready = self.pane_ready.clone();
        let setup_pane_at = b.entry("set up pane", move |cx| -> EntryFuture {
            let (description, ready) = (description.clone(), pane_ready.clone());
            Box::pin(async move {
                let screen = cx.entry().ecx;
                let ret = setup_pane_footer(cx.clone(), title, footer, description, name).await?;
                if let Some(ready) = ready {
                    ready(cx, screen).await?;
                }
                Ok(ret)
            })
        })?;
        let loader_slot = b.entry("loader slot", move |_| -> EntryFuture {
            Box::pin(async move { Ok(Ret::new(loader, 0)) })
        })?;
        let e = Entries {
            loader,
            row_text: row_text_at,
            row_chosen: row_chosen_at,
            setup_pane: setup_pane_at,
            loader_slot,
            event: event_at,
            destructor: destructor_at,
            screen_vtable: tables,
            list_vtable: tables + 4 * screen_slot::COUNT,
        };
        cell.set(e).ok();
        self.loader.store(loader, Ordering::SeqCst);
        let layout = match self.dialog {
            Some(base) => {
                let list = self
                    .layout
                    .unwrap_or_else(|| crate::ui_kit::dialog_tag::base_layout(base, rows.count()));
                crate::ui_kit::dialog_tag::register(crate::ui_kit::dialog_tag::Registered {
                    name: self.screen,
                    vtable: tables,
                    page: b.guest_alloc(0x1000, Permissions::RW)?,
                    loader,
                    base,
                    list,
                });
                Some(list)
            }
            None => self.layout,
        };
        {
            let mut layouts = LAYOUTS.lock().unwrap_or_else(|e| e.into_inner());
            layouts.retain(|layout| layout.screen_vtable != e.screen_vtable);
            layouts.extend(layout.map(|list| RegisteredLayout {
                screen: self.screen,
                screen_vtable: e.screen_vtable,
                list_vtable: e.list_vtable,
                list,
                scroll_arrows: self.scroll_arrows,
            }));
        }
        if let Some(layout) = layout {
            let extra = layout.extra_items();
            if extra > 0 {
                let size = (EXTRA_SCREENS * extra * crate::speaker_row::ITEM_SIZE + 0xfff) & !0xfff;
                let pool = b.guest_alloc(size, Permissions::RW)?;
                let mut x = EXTRA_ITEMS.lock().unwrap_or_else(|e| e.into_inner());
                x.retain(|x| x.list_vtable != e.list_vtable);
                x.push(ExtraItems {
                    list_vtable: e.list_vtable,
                    extra,
                    pool,
                    size,
                    slots: Vec::new(),
                    next: 0,
                });
            }
        }
        LOADERS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.screen, loader);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(building_live));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::{ALL, FPS};

    #[test]
    fn snapshots_wait_while_one_of_our_screens_is_built() {
        assert_eq!(building_reason(0), None);
        assert!(building_reason(6).is_some());
    }
    use crate::video_screen::{self, Switches};
    use engine::entry::{GuestCall as Gc, Regs, testing};
    use engine::testing::FakeGuest;
    use engine::view::fake;
    use settings::Store;

    const SCREEN: u32 = 0x0010_0000;
    const REQUEST: u32 = 0x0020_0000;
    const STACK: u32 = 0x0030_0800;
    const ROWS: u32 = 0x0040_0000;
    const TABLES: u32 = 0x0050_0000;

    fn entries() -> Entries {
        Entries {
            loader: 0xa0f0_0000,
            row_text: 0xa0f0_0010,
            row_chosen: 0xa0f0_0020,
            setup_pane: 0xa0f0_0030,
            loader_slot: 0xa0f0_0040,
            event: 0,
            destructor: 0,
            screen_vtable: TABLES,
            list_vtable: TABLES + 0x70,
        }
    }

    fn video_rows() -> Arc<dyn Rows> {
        Arc::new(ModelRows {
            model: model(),
            screen: video_screen::SCREEN,
        })
    }

    /// One row that opens `target`.
    struct Opens(&'static str);
    impl Rows for Opens {
        fn title(&self) -> u32 {
            0
        }
        fn count(&self) -> usize {
            1
        }
        fn texts(&self, _: i16) -> Vec<(u32, u32)> {
            Vec::new()
        }
        fn press(&self, _: i16) -> Chosen {
            Chosen::Open(self.0)
        }
    }

    /// A press on row 0 by local player `player` of a screen whose row opens
    /// `target`: the calls made.
    fn press_open(target: &'static str, player: u32) -> Vec<Gc> {
        let mut g = guest();
        g.put_u32(STACK + 4, 0x0071_0000)
            .put_u32(0x0071_0000, 0x0072_0000)
            .put_u32(0x0072_0004, player);
        g.put_u32(STACK + 8, 0x0070_0000).put_u16(0x0070_0000, 0);
        let f =
            move |cx| -> EntryFuture { Box::pin(row_chosen(cx, Arc::new(Opens(target)), None)) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| Regs::default());
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        run.calls
    }

    #[test]
    fn an_open_row_opens_another_screen_of_ours_for_the_pressing_player() {
        LOADERS
            .lock()
            .unwrap()
            .insert("test-open-target", 0xa0f0_5000);
        assert_eq!(
            press_open("test-open-target", 2),
            [Gc::new(points::OPEN_SCREEN.base).args([1 << 2, 0xa0f0_5000])]
        );
        assert_eq!(press_open("test-open-missing", 0), [], "not ours");
        assert_eq!(press_open("test-open-target", 4), [], "no local player");
    }

    #[test]
    fn a_built_class_is_found_by_its_screen_id() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let class = ListClass::with_rows("test-found", "test-found-screen", Arc::new(Opens("x")));
        r.add(&class).unwrap();
        assert_eq!(loader_of("test-found-screen"), Some(class.loader()));
        assert_eq!(loader_of("test-never-built"), None);
    }

    fn model() -> Arc<Model> {
        Arc::new(video_screen::model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap())
    }

    /// The retail vtables (synthetic slot values) and a stack.
    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        for i in 0..screen_slot::COUNT {
            g.put_u32(CONTROLS_SCREEN_VTABLE.base + 4 * i, 0x5000 + i);
        }
        for i in 0..list_slot::COUNT {
            g.put_u32(CONTROLS_LIST_VTABLE.base + 4 * i, 0x6000 + i);
        }
        g.map(TABLES, 0x1000).map(STACK - 0x800, 0x1000);
        g
    }

    #[test]
    fn scroll_arrows_clear_the_level_select_bracket() {
        use crate::skulls_menu::{FRAME, FRAME_SIZE, LIST_LAYOUT, PICTURE_BOX, SCROLL_ARROWS};

        let mut g = guest();
        let frame = STACK + 0x80;
        let quad = frame - 0x34;
        let vt = entries().list_vtable;
        let [anchor_x, anchor_y] = LIST_LAYOUT.anchor;
        g.put_u32(TABLES, vt)
            .put_u32(frame - 0xc, TABLES)
            .put_u16(frame - 0x22, anchor_x as u16)
            .put_u16(frame - 0x20, anchor_y as u16);
        let cpu = runtime::CpuAbi {
            esp: STACK,
            ebp: frame,
            esi: quad,
            ..Default::default()
        };
        let frame_right = f32::from(FRAME.at[0]) + f32::from(FRAME_SIZE[0]);
        let picture_left = f32::from(PICTURE_BOX[1]);
        let read_quad = |g: &FakeGuest| {
            [0, 4, 8, 12].map(|offset| f32::from_bits(g.u32(quad + offset).unwrap()))
        };
        let arrows_for = |vtable| (vtable == vt).then_some(SCROLL_ARROWS);
        for (return_pc, top, expected) in [
            (SCROLL_UP_RETURN, 154.0_f32, [9.0, 36.0, 176.0, 159.0]),
            (SCROLL_DOWN_RETURN, 86.0, [9.0, 36.0, -142.0, -159.0]),
        ] {
            g.put_u32(STACK, return_pc);
            let stock = [-245.0_f32, -218.0, top, top - 17.0];
            for (offset, value) in [0, 4, 8, 12].into_iter().zip(stock) {
                g.put_u32(quad + offset, value.to_bits());
            }
            g.put_u32(TABLES, CONTROLS_LIST_VTABLE.base);
            place_scroll_arrow(&mut g, &cpu, arrows_for).unwrap();
            assert_eq!(read_quad(&g), stock, "a different list class");
            g.put_u32(TABLES, vt);
            place_scroll_arrow(&mut g, &cpu, arrows_for).unwrap();
            let placed = read_quad(&g);
            assert_eq!(placed, expected);
            assert!(frame_right < placed[0] && placed[1] < picture_left);
        }

        g.put_u16(frame - 8, 20).put_u16(frame - 4, (-16i16) as u16);
        place_scroll_arrow(&mut g, &cpu, arrows_for).unwrap();
        assert_eq!(read_quad(&g), [29.0, 56.0, -158.0, -175.0]);

        g.put_u32(STACK, 0x1234).put_u16(frame - 8, 0);
        place_scroll_arrow(&mut g, &cpu, arrows_for).unwrap();
        assert_eq!(read_quad(&g), [29.0, 56.0, -158.0, -175.0], "another draw");
    }

    /// What the Controls constructor leaves: vtables, the press record, and
    /// `n` rows in the list's datum array.
    fn construct(g: &mut dyn GuestMemory, n: usize) {
        let s = Screen::at(SCREEN);
        s.set_vtable(g, CONTROLS_SCREEN_VTABLE.base).unwrap();
        let list = s.list();
        list.set_vtable(g, CONTROLS_LIST_VTABLE.base).unwrap();
        let press = list.press(ui::CONTROLS_PRESS);
        press.set_target(g, list.addr).unwrap();
        press.set_function(g, CONTROLS_ROW_CHOSEN).unwrap();
        // The ported allocator lays out the array as the game's does.
        let mut f = FakeGuest::new();
        let d = fake::datums::<ui::Row>(&mut f, ROWS, n as i32);
        for _ in 0..n {
            fake::push(&mut f, &d);
        }
        let bytes = f.bytes(ROWS - 0x20, 0x200).unwrap();
        g.write(ROWS - 0x20, &bytes).unwrap();
        list.set_rows(g, ROWS).unwrap();
    }

    #[test]
    fn adopting_points_the_screen_at_our_class_and_trims_the_rows() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 7); // grown by the speaker row's list hook
        let e = entries();
        assert_eq!(adopt(&mut g, SCREEN, &e, 4), Ok(Adopted::Ours { rows: 4 }));
        let s = Screen::at(SCREEN);
        assert_eq!(s.vtable(&g), Ok(TABLES));
        assert_eq!(s.list().vtable(&g), Ok(TABLES + 0x70));
        assert_eq!(s.loaded(&g), Ok(1));
        assert_eq!(
            s.list().press(ui::CONTROLS_PRESS).function(&g),
            Ok(e.row_chosen)
        );
        let live = s.list().row_array(&g).unwrap().live(&g, 16).unwrap();
        assert_eq!(
            live.iter().map(|(h, _)| h.index()).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        // Our slots, the retail ones everywhere else.
        let slot = |vt, i| ui::slot(&g, vt, i).unwrap();
        assert_eq!(slot(TABLES, screen_slot::SETUP_PANE), e.setup_pane);
        assert_eq!(slot(TABLES, screen_slot::LOADER), e.loader_slot);
        assert_eq!(slot(TABLES, 0), 0x5000);
        assert_eq!(slot(TABLES, 27), 0x5000 + 27);
        assert_eq!(slot(TABLES + 0x70, list_slot::ROW_TEXT), e.row_text);
        assert_eq!(slot(TABLES + 0x70, list_slot::ITEM_COUNT), 0x6000 + 19);
        assert_eq!(slot(TABLES + 0x70, list_slot::ADD_ITEMS), 0x6000 + 17);
        assert_eq!(slot(TABLES + 0x70, 21), 0x6000 + 21);
        // A second screen finds the tables written and changes nothing.
        g.put_u32(TABLES + 4, 0xbad);
        write_vtables(&mut g, &e).unwrap();
        assert_eq!(ui::slot(&g, TABLES, 1), Ok(0xbad));
    }

    #[test]
    fn more_rows_than_the_array_holds_are_cut_to_its_size() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 7);
        assert_eq!(
            adopt(&mut g, SCREEN, &entries(), 9),
            Ok(Adopted::Ours { rows: 7 })
        );
    }

    #[test]
    fn a_screen_that_is_not_controls_is_left_alone() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 6);
        Screen::at(SCREEN)
            .list()
            .press(ui::CONTROLS_PRESS)
            .set_function(&mut g, 0x1234)
            .unwrap();
        assert!(matches!(
            adopt(&mut g, SCREEN, &entries(), 4),
            Ok(Adopted::NotControls(_))
        ));
        assert_eq!(
            Screen::at(SCREEN).vtable(&g),
            Ok(CONTROLS_SCREEN_VTABLE.base)
        );
        assert_eq!(g.u32(TABLES), Ok(0), "no tables written");
    }

    /// Loader tests run one at a time: the array count is process-wide.
    static LOADING: Mutex<()> = Mutex::new(());

    /// The loader against fake callees: allocate, construct (the fake builds
    /// what the Controls constructor builds), adopt, push.
    #[test]
    fn the_loader_allocates_constructs_adopts_and_pushes() {
        let _serial = LOADING.lock().unwrap_or_else(|e| e.into_inner());
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        g.put_u16(REQUEST + 2, 0x4)
            .put_u32(REQUEST + 4, 5)
            .put_u32(REQUEST + 8, 4);
        g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
        let e = entries();
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, e, video_rows(), "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let during = std::sync::Mutex::new(None);
        let run = testing::run(&f, g, regs, |c, m| {
            if c.target == points::CONTROLS_SCREEN_CTOR.base {
                *during.lock().unwrap() = Some((
                    array_count(crate::speaker_row::LIST_NAME, 7),
                    array_count(0x1234, 7),
                ));
                construct(m, 7);
            }
            Regs {
                eax: if c.target == points::UI_ALLOC.base {
                    // The fake allocator maps the screen.
                    SCREEN
                } else {
                    0
                },
                ..Default::default()
            }
        });
        let calls: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            calls,
            [
                points::UI_ALLOC.base,
                points::CONTROLS_SCREEN_CTOR.base,
                points::SCREEN_PUSH.base
            ]
        );
        assert_eq!(run.calls[0].args, [0x9b4]);
        assert_eq!(
            run.calls[1],
            Gc::new(points::CONTROLS_SCREEN_CTOR.base)
                .edi(SCREEN)
                .args([5, 4, 4])
        );
        assert_eq!(
            run.calls[2],
            Gc::new(points::SCREEN_PUSH.base).edi(SCREEN).ebx(REQUEST)
        );
        assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
        assert_eq!(Screen::at(SCREEN).vtable(&run.guest), Ok(TABLES));
        // While the constructor ran, the Controls rows' array was asked for
        // our six rows (the Video screen's); any other array, and anything
        // after, as asked.
        assert_eq!(*during.lock().unwrap(), Some((6, 7)));
        assert_eq!(array_count(crate::speaker_row::LIST_NAME, 7), 7);
    }

    /// The pause menu's Video and Voice opened blank in a game: the
    /// Controls tag (screen 39) is in mainmenu only. In a game the loader
    /// runs the Controls constructor's steps on screen 200's tag, the
    /// in-game Controller Settings; in the main menu, the Controls
    /// constructor as before.
    #[test]
    fn in_a_game_the_loader_builds_on_the_in_game_controller_settings() {
        let _serial = LOADING.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(base_screen_id(MAIN_MENU_KIND), CONTROLS_SCREEN_ID);
        assert_eq!(base_screen_id(0), INGAME_SCREEN_ID, "a campaign game");
        assert_eq!(base_screen_id(1), INGAME_SCREEN_ID, "a multiplayer game");
        for (kind, ingame_) in [(0u8, true), (MAIN_MENU_KIND, false)] {
            let mut g = guest();
            g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
                .map(ROWS - 0x20, 0x200)
                .map(UI_GAME_KIND & !0xfff, 0x1000);
            g.put_u8(UI_GAME_KIND, kind);
            g.put_u16(REQUEST + 2, 0x4)
                .put_u32(REQUEST + 4, 5)
                .put_u32(REQUEST + 8, 4);
            g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
            assert_eq!(ingame(&g), Ok(ingame_));
            let e = entries();
            let f = move |cx| -> EntryFuture { Box::pin(load(cx, e, video_rows(), "test")) };
            let regs = Regs {
                esp: STACK,
                ..Default::default()
            };
            let run = testing::run(&f, g, regs, |c, m| {
                if c.target == points::CONTROLS_SCREEN_CTOR.base || c.target == CONTROLS_LIST_CTOR {
                    construct(m, 7);
                }
                Regs {
                    eax: if c.target == points::UI_ALLOC.base { SCREEN } else { 0 },
                    ..Default::default()
                }
            });
            let list = SCREEN + ui::LIST;
            let push = Gc::new(points::SCREEN_PUSH.base).edi(SCREEN).ebx(REQUEST);
            if ingame_ {
                assert_eq!(run.calls.len(), 4, "{:?}", run.calls);
                assert_eq!(
                    run.calls[1],
                    Gc::new(GENERIC_SCREEN_CTOR)
                        .eax(SCREEN)
                        .edi(SCREEN)
                        .esi(list)
                        .args([INGAME_SCREEN_ID, 5, 4, 4, list])
                );
                assert_eq!(
                    run.calls[2],
                    Gc::new(CONTROLS_LIST_CTOR).esi(list).edi(SCREEN).args([4])
                );
                assert_eq!(run.calls[3], push);
            } else {
                let calls: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
                assert_eq!(
                    calls,
                    [points::UI_ALLOC.base, points::CONTROLS_SCREEN_CTOR.base, push.target]
                );
            }
            assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
            // Adopted as ours either way.
            assert_eq!(Screen::at(SCREEN).vtable(&run.guest), Ok(TABLES), "kind {kind}");
        }
    }

    /// The value picker opened as the game opens it, and every string it
    /// names read the way `TEXT_SET` (`0x253b1a`) reads one: from the
    /// slot at the moment of the call. It opened blank on the Clankers
    /// and Halo 1 Options pages (2026-11-19): its class took its title's
    /// slot at build, before any pick, and nothing filled it after; the
    /// description's slot was set after the texts were written, so the
    /// call read it empty. Here the class is "built" first, a row's A
    /// offers its pick, the loader runs (the rows the Controls rows'
    /// array is asked for, and the texts in memory before the
    /// constructor), the pane set-up sets the title, and row text draws
    /// each value and the focused row's description.
    #[test]
    fn the_value_picker_opens_with_its_pick_on_the_games_path() {
        use crate::ui_kit::picker::{self, Pick, Picker};
        use crate::ui_kit::screen::SLOT_LEN;
        let _serial = LOADING.lock().unwrap_or_else(|e| e.into_inner());
        const TEXTS: u32 = 0x0068_0000;
        const ITEM: u32 = 0x0060_0000;
        let p = Arc::new(Picker::new(false));
        p.slot_texts().base.store(TEXTS, Ordering::SeqCst);
        let family = p.slot_texts().family;
        let rows: Arc<dyn Rows> = Arc::clone(&p) as Arc<dyn Rows>;
        // What a `TEXT_SET` of `id` shows: the slot's string as it is now.
        let shown = move |m: &dyn GuestMemory, id: u32| -> String {
            assert!(family.owns(id), "{id:#x} is not the picker's");
            let mut b = vec![0u8; SLOT_LEN as usize];
            m.read(TEXTS + (id - family.base) * SLOT_LEN, &mut b).unwrap();
            let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).into_owned()
        };

        // Built at boot, before any pick: the title's id is taken here.
        let title = rows.title();
        // A on a row of four values, the second held.
        let values = ["25", "50", "75", "100"].map(String::from).to_vec();
        let chosen = Arc::new(Mutex::new(None));
        let set = {
            let chosen = Arc::clone(&chosen);
            Arc::new(move |i: usize| {
                *chosen.lock().unwrap() = Some(i);
                format!("score -> {i}")
            })
        };
        let pick = Pick::new(
            "Score to Win Round:",
            values.clone(),
            1,
            "Points to win a round.",
            set,
        );
        assert_eq!(picker::offer(pick), Some(picker::NAME));

        // The loader.
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200)
            .map(TEXTS, family.len * SLOT_LEN);
        g.put_u16(REQUEST + 2, 0x4)
            .put_u32(REQUEST + 4, 5)
            .put_u32(REQUEST + 8, 4);
        g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
        let e = entries();
        let f = {
            let rows = Arc::clone(&rows);
            move |cx| -> EntryFuture { Box::pin(load(cx, e, Arc::clone(&rows), picker::NAME)) }
        };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let during = std::sync::Mutex::new(None);
        let run = testing::run(&f, g, regs, |c, m| {
            if c.target == points::CONTROLS_SCREEN_CTOR.base {
                let texts: Vec<String> = (0..values.len() as u32)
                    .map(|i| shown(&*m, family.id(3 + i)))
                    .collect();
                *during.lock().unwrap() = Some((
                    array_count(crate::speaker_row::LIST_NAME, 7),
                    shown(&*m, title),
                    texts,
                ));
                construct(m, 7);
            }
            Regs {
                eax: if c.target == points::UI_ALLOC.base { SCREEN } else { 0 },
                ..Default::default()
            }
        });
        assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
        let (asked, title_text, value_texts) = during.lock().unwrap().take().expect("constructed");
        assert_eq!(asked, 4, "the Controls rows' array is asked for the four values");
        assert_eq!(
            title_text, "SCORE TO WIN ROUND",
            "the title is written before the screen is built"
        );
        assert_eq!(value_texts, values);
        let live = Screen::at(SCREEN)
            .list()
            .row_array(&run.guest)
            .unwrap()
            .live(&run.guest, 16)
            .unwrap();
        assert_eq!(live.len(), 4, "a row per value");
        let mut g = run.guest;

        // The pane set-up: the title on the header, as `TEXT_SET` reads it.
        g.put_u32(STACK + 4, 0x55);
        let f = move |cx| -> EntryFuture {
            Box::pin(setup_pane_footer(cx, title, None, None, picker::NAME))
        };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let header = std::sync::Mutex::new(Vec::new());
        let run = testing::run(&f, g, regs, |c, m| {
            if c.target == points::TEXT_SET.base {
                header.lock().unwrap().push((c.edi, shown(&*m, c.args[0])));
            }
            Regs::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 4)));
        assert_eq!(
            *header.lock().unwrap(),
            [(Some(Screen::at(SCREEN).header()), "SCORE TO WIN ROUND".to_string())]
        );
        let mut g = run.guest;

        // Row text for each item, row 0 focused: each value on its item,
        // the description once, as the calls read them.
        let list = SCREEN + ui::LIST;
        let described = Arc::new(Shown::default());
        described.set_widget(SCREEN, 0x9000);
        g.map(ITEM, Item::SIZE);
        let mut drawn = Vec::new();
        for row in 0..values.len() as u32 {
            g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
            Item::at(ITEM)
                .set_row(&mut g, Some(DatumHandle::new(0x8001, row)))
                .unwrap();
            let f = {
                let (rows, described) = (Arc::clone(&rows), Arc::clone(&described));
                move |cx| -> EntryFuture {
                    Box::pin(row_text(cx, Arc::clone(&rows), Some(Arc::clone(&described)), None))
                }
            };
            let regs = Regs {
                esp: STACK,
                ecx: list,
                ..Default::default()
            };
            let sets = std::sync::Mutex::new(Vec::new());
            let run = testing::run(&f, g, regs, |c, m| {
                if c.target == points::TEXT_SET.base {
                    sets.lock().unwrap().push((c.edi, shown(&*m, c.args[0])));
                }
                Regs {
                    eax: match c.target {
                        t if t == points::ITEM_TEXT.base => 0x7000 + c.args[1],
                        t if t == points::LIST_FOCUSED_ROW.base => {
                            DatumHandle::new(0x8001, 0).raw()
                        }
                        _ => 0,
                    },
                    ..Default::default()
                }
            });
            assert_eq!(run.result, Ok(Ret::new(0, 8)));
            drawn.push(sets.into_inner().unwrap());
            g = run.guest;
        }
        let label = Some(0x7000 + crate::ui::LABEL_SLOT);
        assert_eq!(
            drawn[0],
            [
                (label, "25".to_string()),
                (Some(0x9000), "Points to win a round.".to_string())
            ],
            "the first item draws, then the focused row's description"
        );
        for (row, sets) in drawn.iter().enumerate().skip(1) {
            assert_eq!(*sets, [(label, values[row].clone())], "row {row}");
        }

        // A on the third value sets it through the row's setter, and the
        // action closes the picker.
        assert_eq!(rows.press(2), Chosen::Act);
        assert_eq!(*chosen.lock().unwrap(), Some(2));
    }

    /// In a game a row is one line (the in-game items have one text):
    /// label and value in a slot of the class's, its id set on text 0.
    #[test]
    fn in_a_game_a_row_draws_as_one_line() {
        const ITEM: u32 = 0x0060_0000;
        const LINES_AT: u32 = 0x0062_0000;
        let m = model();
        let lines = Arc::new(Lines::of("test-in-game-lines").unwrap());
        lines.at.store(LINES_AT, Ordering::SeqCst);
        let mut g = guest();
        g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
        g.map(ITEM, Item::SIZE)
            .map(LINES_AT, LINES_PER_CLASS * LINE_LEN)
            .map(UI_GAME_KIND & !0xfff, 0x1000);
        g.put_u8(UI_GAME_KIND, 0);
        Item::at(ITEM)
            .set_row(&mut g, Some(DatumHandle::new(0x8001, 1)))
            .unwrap();
        let want = m.line(m.row(video_screen::LIST, 1).unwrap());
        assert!(want.starts_with("Frame Rate: "), "{want}");
        let f = {
            let (m, lines) = (Arc::clone(&m), Arc::clone(&lines));
            move |cx| -> EntryFuture {
                Box::pin(row_text(
                    cx,
                    Arc::new(ModelRows {
                        model: Arc::clone(&m),
                        screen: video_screen::SCREEN,
                    }),
                    None,
                    Some(Arc::clone(&lines)),
                ))
            }
        };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: if c.target == points::ITEM_TEXT.base { 0x7000 + c.args[1] } else { 0 },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let (id, _) = lines.set(1, &want).unwrap();
        assert_eq!(
            run.calls,
            [
                Gc::new(points::ITEM_TEXT.base).ecx(ITEM).args([6, 0, 0]),
                Gc::new(points::TEXT_SET.base).edi(0x7000).esi(ITEM).args([id]),
            ]
        );
        let at = lines.address(id).unwrap();
        let mut bytes = vec![0u8; want.len() + 1];
        run.guest.read(at, &mut bytes).unwrap();
        assert_eq!(&bytes[..want.len()], want.as_bytes());
        assert_eq!(bytes[want.len()], 0);
    }

    /// A row's line keeps its id while its text stays, and takes the
    /// row's other id when it changes, so the text widget sets it again.
    #[test]
    fn a_line_changes_id_when_its_text_changes() {
        let l = Lines::of("test-line-ids").unwrap();
        l.at.store(0x1000_0000, Ordering::SeqCst);
        let (a, w) = l.set(2, "Frame Rate: 30").unwrap();
        assert_eq!(w, Some((l.address(a).unwrap(), b"Frame Rate: 30\0".to_vec())));
        assert_eq!(l.set(2, "Frame Rate: 30").unwrap(), (a, None), "unchanged");
        let (b, w) = l.set(2, "Frame Rate: 60").unwrap();
        assert_ne!(a, b);
        assert!(w.is_some());
        let (c, _) = l.set(2, "Frame Rate: 30").unwrap();
        assert_eq!(c, a, "two ids a row, in turn");
        assert_ne!(l.set(3, "x").unwrap().0, a, "another row");
        assert_eq!(l.set(LINES_PER_CLASS as i16 / 2, "past"), None);
        assert_eq!(l.set(-1, "none"), None);
        assert!(crate::ui_kit::ids::family("ui lines").owns(a));
        assert_eq!(Lines::of("test-line-ids").unwrap().first, l.first, "the same class's ids");
    }

    #[test]
    fn a_screen_of_fifteen_rows_gets_an_array_of_fifteen() {
        let _serial = LOADING.lock().unwrap_or_else(|e| e.into_inner());
        struct Fifteen;
        impl Rows for Fifteen {
            fn title(&self) -> u32 {
                0
            }
            fn count(&self) -> usize {
                15
            }
            fn texts(&self, _: i16) -> Vec<(u32, u32)> {
                Vec::new()
            }
            fn press(&self, _: i16) -> Chosen {
                Chosen::Nothing
            }
        }
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x400);
        g.put_u16(REQUEST + 2, 0)
            .put_u32(REQUEST + 4, 5)
            .put_u32(REQUEST + 8, 4);
        g.put_u32(STACK, 0xdead).put_u32(STACK + 4, REQUEST);
        let e = entries();
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, e, Arc::new(Fifteen), "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, m| {
            if c.target == points::CONTROLS_SCREEN_CTOR.base {
                // The fake constructor builds what the hook would have it
                // build.
                construct(m, array_count(crate::speaker_row::LIST_NAME, 6) as usize);
            }
            Regs {
                eax: if c.target == points::UI_ALLOC.base {
                    SCREEN
                } else {
                    0
                },
                ..Default::default()
            }
        });
        assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
        let live = Screen::at(SCREEN)
            .list()
            .row_array(&run.guest)
            .unwrap()
            .live(&run.guest, 32)
            .unwrap();
        assert_eq!(live.len(), 15);
    }

    #[test]
    fn growing_a_waiting_list_keeps_its_first_row_handle() {
        let mut g = guest();
        g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE)
            .map(ROWS - 0x20, 0x200);
        construct(&mut g, 4);
        adopt(&mut g, SCREEN, &entries(), 1).unwrap();
        let screen = Screen::at(SCREEN);
        let rows = screen.list().row_array(&g).unwrap();
        let first = rows.live(&g, 4).unwrap()[0].0;
        grow_rows(&mut g, screen, 4).unwrap();
        grow_rows(&mut g, screen, 4).unwrap();
        let live = rows.live(&g, 4).unwrap();
        assert_eq!(live.len(), 4);
        assert_eq!(live[0].0, first);
    }

    #[test]
    fn a_failed_allocation_opens_nothing() {
        let mut g = guest();
        g.put_u32(STACK + 4, REQUEST);
        let f = move |cx| -> EntryFuture { Box::pin(load(cx, entries(), video_rows(), "test")) };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| Regs::default());
        assert_eq!(run.calls.len(), 1);
        assert_eq!(run.result, Ok(Ret::new(0, 4)));
    }

    /// Row text draws the model's label and value into the item's text
    /// widgets 0 and 1.
    #[test]
    fn row_text_sets_each_text_of_the_items_row() {
        const ITEM: u32 = 0x0060_0000;
        let m = model();
        let mut g = guest();
        g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
        g.map(ITEM, Item::SIZE);
        Item::at(ITEM)
            .set_row(&mut g, Some(DatumHandle::new(0x8001, 1)))
            .unwrap();
        let list = video_screen::LIST;
        let want = m.texts(m.row(list, 1).unwrap()).to_vec();
        let f = {
            let m = Arc::clone(&m);
            move |cx| -> EntryFuture {
                Box::pin(row_text(
                    cx,
                    Arc::new(ModelRows {
                        model: Arc::clone(&m),
                        screen: video_screen::SCREEN,
                    }),
                    None,
                    None,
                ))
            }
        };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            // Text widget `index` of the item.
            eax: if c.target == points::ITEM_TEXT.base {
                0x7000 + c.args[1]
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let mut expect = Vec::new();
        for (index, id) in want {
            expect.push(
                Gc::new(points::ITEM_TEXT.base)
                    .ecx(ITEM)
                    .args([6, index, 0]),
            );
            expect.push(
                Gc::new(points::TEXT_SET.base)
                    .edi(0x7000 + index)
                    .esi(ITEM)
                    .args([id]),
            );
        }
        assert_eq!(run.calls, expect);
    }

    #[test]
    fn row_chosen_steps_the_setting_and_calls_nothing() {
        let store = Arc::new(Store::defaults(ALL));
        let fps = store.handle::<u32>(FPS.key);
        let m = Arc::new(video_screen::model(Arc::clone(&store), Switches::ALL).unwrap());
        let mut g = guest();
        g.put_u32(STACK + 8, 0x0070_0000).put_u16(0x0070_0000, 1); // Frame Rate
        let f = {
            let m = Arc::clone(&m);
            move |cx| -> EntryFuture {
                Box::pin(row_chosen(
                    cx,
                    Arc::new(ModelRows {
                        model: Arc::clone(&m),
                        screen: video_screen::SCREEN,
                    }),
                    None,
                ))
            }
        };
        let regs = Regs {
            esp: STACK,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| unreachable!("no guest call"));
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        assert_eq!(fps.get(), 60);
    }

    #[test]
    fn the_pane_set_up_runs_the_retail_one_then_sets_our_title() {
        let mut g = guest();
        g.put_u32(STACK + 4, 0x55);
        let f = |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x20ff_f077, None, "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: if c.target == points::SCREEN_SETUP_PANE.base {
                0x99
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(
            run.calls,
            [
                Gc::new(points::SCREEN_SETUP_PANE.base)
                    .ecx(SCREEN)
                    .args([0x55]),
                Gc::new(points::TEXT_SET.base)
                    .edi(SCREEN + 0x80)
                    .esi(SCREEN)
                    .args([0x20ff_f077]),
            ]
        );
        assert_eq!(run.result, Ok(Ret::new(0x99, 4)));
    }

    #[test]
    fn the_class_places_five_entries_and_one_table_page() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let class = ListClass::new("video-screen", video_screen::SCREEN, model());
        assert_eq!(class.loader(), 0);
        r.add(&class).unwrap();
        let plan = r.build(&hooks::Base).unwrap();
        assert_eq!(plan.entries.len(), 5);
        assert_eq!(class.loader(), plan.entries[0].addr);
        let text = plan.text();
        for l in [
            "loader",
            "row text",
            "row chosen",
            "set up pane",
            "loader slot",
        ] {
            assert!(text.contains(&format!("entry video-screen:{l}")), "{text}");
        }
    }

    /// Rows with a description each, for the description tests.
    struct Described;
    impl Rows for Described {
        fn title(&self) -> u32 {
            0x77
        }
        fn count(&self) -> usize {
            15
        }
        fn texts(&self, row: i16) -> Vec<(u32, u32)> {
            if (0..15).contains(&row) {
                vec![(0, 0x100 + row as u32)]
            } else {
                Vec::new()
            }
        }
        fn press(&self, _row: i16) -> Chosen {
            Chosen::Nothing
        }
        fn description(&self, row: i16) -> Option<u32> {
            (0..15).contains(&row).then_some(0x200 + row as u32)
        }
        fn tint(&self, row: i16) -> Option<[f32; 3]> {
            (row == 3).then_some([0.5, 0.25, 1.0])
        }
        fn picture(&self, row: i16) -> Option<u32> {
            (row == 3).then_some(0xe6dd_04bb)
        }
    }

    #[test]
    fn a_text_block_is_laid_out_as_the_pane_builder_reads_it() {
        let b = TextBlock {
            rect: [200, 80, 140, 310],
            font: 1,
            colour: [0.5, 0.25, 1.0],
            animation: 10,
        }
        .bytes();
        assert_eq!(b.len(), 0x2c);
        assert_eq!(
            &b[0..4],
            &1u32.to_le_bytes(),
            "left justified, larger widget"
        );
        assert_eq!(&b[4..6], &10u16.to_le_bytes(), "its animation");
        assert_eq!(&b[0xa..0xc], &1i16.to_le_bytes());
        assert_eq!(&b[0x10..0x14], &0.5f32.to_le_bytes());
        assert_eq!(&b[0x18..0x1c], &1.0f32.to_le_bytes());
        assert_eq!(&b[0x1c..0x1e], &200i16.to_le_bytes());
        assert_eq!(&b[0x22..0x24], &310i16.to_le_bytes());
        assert_eq!(
            &b[0x24..0x28],
            &[0; 4],
            "the null string until a row is focused"
        );
    }

    #[test]
    fn the_description_is_set_only_when_the_focus_moves() {
        let s = Shown::default();
        assert_eq!(s.focus(SCREEN, Some(0)), None, "no widget yet");
        s.set_widget(SCREEN, 0x9000);
        assert_eq!(s.focus(SCREEN, Some(0)), Some(0x9000));
        assert_eq!(s.focus(SCREEN, Some(0)), None);
        assert_eq!(s.focus(SCREEN, Some(3)), Some(0x9000));
        assert_eq!(s.focus(SCREEN, None), Some(0x9000));
        assert_eq!(s.focus(SCREEN + 0x1000, Some(3)), None, "another screen");
        // Set up again (a new screen at the same address): shows nothing.
        s.set_widget(SCREEN, 0x9100);
        assert_eq!(s.focus(SCREEN, Some(3)), Some(0x9100));
        for i in 0..20 {
            s.set_widget(0x10 * i, 1);
        }
        assert_eq!(s.lock().len(), SHOWN_KEPT);
    }

    /// The pane set-up with a description: retail set-up, title, the block
    /// written, then the pane builder's text step on it.
    #[test]
    fn the_pane_set_up_adds_the_description_widget() {
        let mut g = guest();
        g.put_u32(STACK + 4, 0x55);
        let block = TextBlock {
            rect: [1, 2, 3, 4],
            font: 1,
            colour: [1.0; 3],
            animation: 0,
        };
        let shown = Arc::new(Shown::default());
        let d = Description {
            at: TABLES + DESCRIPTION_BLOCK,
            block,
            shown: Arc::clone(&shown),
            picture: None,
            picture_at: 0,
            frame: None,
            frame_at: 0,
            panel: Panel::default(),
            panel_at: 0,
            rows: None,
        };
        let f =
            move |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x77, Some(d.clone()), "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: if c.target == points::PANE_TEXT_ADD.base {
                0x9000
            } else {
                0
            },
            ..Default::default()
        });
        assert_eq!(
            run.calls,
            [
                Gc::new(points::SCREEN_SETUP_PANE.base)
                    .ecx(SCREEN)
                    .args([0x55]),
                Gc::new(points::TEXT_SET.base)
                    .edi(SCREEN + 0x80)
                    .esi(SCREEN)
                    .args([0x77]),
                Gc::new(points::PANE_TEXT_ADD.base)
                    .ebx(SCREEN)
                    .args([0, TABLES + DESCRIPTION_BLOCK]),
            ]
        );
        assert_eq!(
            run.guest.bytes(TABLES + DESCRIPTION_BLOCK, 0x2c).unwrap(),
            block.bytes()
        );
        assert_eq!(shown.focus(SCREEN, Some(2)), Some(0x9000));
    }

    /// Row text on a class with a description: the row's texts, then the
    /// focused row; its description only when the focus moved.
    #[test]
    fn row_text_sets_the_focused_rows_description_once() {
        const ITEM: u32 = 0x0060_0000;
        let list = SCREEN + ui::LIST;
        let shown = Arc::new(Shown::default());
        shown.set_widget(SCREEN, 0x9000);
        let rows: Arc<dyn Rows> = Arc::new(Described);
        let draw = |focused: u32| {
            let mut g = guest();
            g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
            g.map(ITEM, Item::SIZE);
            Item::at(ITEM)
                .set_row(&mut g, Some(DatumHandle::new(0x8001, 9)))
                .unwrap();
            let f = {
                let (s, shown) = (Arc::clone(&rows), Arc::clone(&shown));
                move |cx| -> EntryFuture {
                    Box::pin(row_text(cx, Arc::clone(&s), Some(Arc::clone(&shown)), None))
                }
            };
            let regs = Regs {
                esp: STACK,
                ecx: list,
                ..Default::default()
            };
            testing::run(&f, g, regs, move |c, _| Regs {
                eax: match c.target {
                    t if t == points::ITEM_TEXT.base => 0x7000,
                    t if t == points::LIST_FOCUSED_ROW.base => focused,
                    _ => 0,
                },
                ..Default::default()
            })
        };
        let item_calls = [
            Gc::new(points::ITEM_TEXT.base).ecx(ITEM).args([6, 0, 0]),
            Gc::new(points::TEXT_SET.base)
                .edi(0x7000)
                .esi(ITEM)
                .args([0x109]),
            Gc::new(points::LIST_FOCUSED_ROW.base).eax(list),
        ];
        let run = draw(DatumHandle::new(0x8001, 3).raw());
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let mut want = item_calls.to_vec();
        want.push(
            Gc::new(points::TEXT_SET.base)
                .edi(0x9000)
                .esi(SCREEN)
                .args([0x203]),
        );
        assert_eq!(run.calls, want);
        // The same focus on the next draw: no description call.
        assert_eq!(draw(DatumHandle::new(0x8001, 3).raw()).calls, item_calls);
        // Nothing focused: nothing set; focus back on row 3 sets it again.
        assert_eq!(draw(u32::MAX).calls, item_calls);
        assert_eq!(draw(DatumHandle::new(0x8001, 3).raw()).calls.len(), 4);
    }

    /// Text that changes while the screen is up: written before the row
    /// is drawn, and the description set again with the focus unmoved.
    #[test]
    fn live_text_is_written_and_redraws_the_description() {
        struct Live;
        impl Rows for Live {
            fn title(&self) -> u32 {
                0x77
            }
            fn count(&self) -> usize {
                1
            }
            fn texts(&self, _row: i16) -> Vec<(u32, u32)> {
                vec![(0, 0x100)]
            }
            fn press(&self, _row: i16) -> Chosen {
                Chosen::Nothing
            }
            fn description(&self, _row: i16) -> Option<u32> {
                Some(0x200)
            }
            fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
                (row == Some(0)).then_some(0x201)
            }
            fn live_text(&self) -> LiveText {
                LiveText {
                    writes: vec![(0x0062_0000, b"hi\0".to_vec())],
                    redraw: true,
                }
            }
        }
        const ITEM: u32 = 0x0060_0000;
        let list = SCREEN + ui::LIST;
        let shown = Arc::new(Shown::default());
        shown.set_widget(SCREEN, 0x9000);
        let rows: Arc<dyn Rows> = Arc::new(Live);
        let draw = || {
            let mut g = guest();
            g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
            g.map(ITEM, Item::SIZE);
            g.map(0x0062_0000, 0x10);
            Item::at(ITEM)
                .set_row(&mut g, Some(DatumHandle::new(0x8001, 0)))
                .unwrap();
            let f = {
                let (s, shown) = (Arc::clone(&rows), Arc::clone(&shown));
                move |cx| -> EntryFuture {
                    Box::pin(row_text(cx, Arc::clone(&s), Some(Arc::clone(&shown)), None))
                }
            };
            let regs = Regs {
                esp: STACK,
                ecx: list,
                ..Default::default()
            };
            testing::run(&f, g, regs, move |c, _| Regs {
                eax: match c.target {
                    t if t == points::ITEM_TEXT.base => 0x7000,
                    t if t == points::LIST_FOCUSED_ROW.base => DatumHandle::new(0x8001, 0).raw(),
                    _ => 0,
                },
                ..Default::default()
            })
        };
        let set = Gc::new(points::TEXT_SET.base)
            .edi(0x9000)
            .esi(SCREEN)
            .args([0x200]);
        draw();
        let run = draw();
        assert!(
            run.calls.contains(
                &Gc::new(points::TEXT_SET.base)
                    .edi(SCREEN + FOOTER)
                    .esi(SCREEN)
                    .args([0x201])
            )
        );
        assert_eq!(run.guest.bytes(0x0062_0000, 3).unwrap(), b"hi\0");
        assert_eq!(run.calls.last(), Some(&set), "the focus did not move");
    }

    #[test]
    fn a_class_with_a_description_places_the_same_entries() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let class = ListClass::with_rows("described", "described", Arc::new(Described))
            .description(TextBlock {
                rect: [1, 2, 3, 4],
                font: 1,
                colour: [1.0; 3],
                animation: 0,
            });
        r.add(&class).unwrap();
        let plan = r.build(&hooks::Base).unwrap();
        assert_eq!(plan.entries.len(), 5);
        assert_eq!(class.loader(), plan.entries[0].addr);
    }

    const ENTRIES: u32 = 0x0080_0000;
    const TAG: u32 = 0x0081_0000;
    const PANES: u32 = 0x0082_0000;
    const BITMAPS: u32 = 0x0083_0000;

    fn spec() -> PictureSpec {
        PictureSpec {
            screen_id: 12,
            pane: 3,
            bitmap: 1,
            image: 3,
            at: [56, 150],
            animation: 12,
        }
    }

    /// A screen tag (datum 0x1234_0005) with four panes; pane 3 has two
    /// bitmap blocks, the second showing image 3 of a `bitm`.
    fn screen_tag() -> FakeGuest {
        let mut g = guest();
        g.map(ENTRIES, 0x100)
            .map(TAG, 0x100)
            .map(PANES, 0x200)
            .map(BITMAPS, 0x100)
            .map(points::TAG_ENTRIES.base, 4);
        g.put_u32(points::TAG_ENTRIES.base, ENTRIES)
            .put_u32(ENTRIES + 5 * 0x10 + 8, TAG)
            .put_u32(TAG + 0x20, 4)
            .put_u32(TAG + 0x24, PANES)
            .put_u32(PANES + 3 * 0x4c + 0x24, 2)
            .put_u32(PANES + 3 * 0x4c + 0x28, BITMAPS);
        let b = BITMAPS + 0x38;
        g.put_u32(b + 4, 11)
            .put_u16(b + 0xa, 3)
            .put_u16(b + 0xc, 44)
            .put_u16(b + 0xe, 124)
            .put_u32(b + 0x18, engine::tags::GroupTag::from_chars(*b"bitm").0)
            .put_u32(b + 0x1c, 0xe588_0368)
            .put_u16(b + 0x20, 2);
        g
    }

    /// The block is the retail one, placed at ours with our animation (the
    /// retail block's own, 11 here, is not kept); one
    /// that is not the bitmap meant is refused, with why.
    #[test]
    fn the_picture_is_a_copy_of_the_retail_bitmap_block_placed_at_ours() {
        let g = screen_tag();
        let b = picture_block(&g, 0x1234_0005, &spec()).unwrap();
        let word = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let half = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        assert_eq!(word(4), 12, "our animation, so it fades with its screen");
        assert_eq!((half(0xc), half(0xe)), (56, 150));
        assert_eq!(half(0xa), 3);
        assert_eq!(word(0x1c), 0xe588_0368, "the map's own bitmap");
        assert_eq!(half(0x20), 2, "the rest as the tag has it");
        assert!(
            picture_block(&g, u32::MAX, &spec())
                .unwrap_err()
                .contains("not in this")
        );
        let wrong = PictureSpec { image: 2, ..spec() };
        assert!(
            picture_block(&g, 0x1234_0005, &wrong)
                .unwrap_err()
                .contains("not the one")
        );
        let pane = PictureSpec { pane: 4, ..spec() };
        assert!(
            picture_block(&g, 0x1234_0005, &pane)
                .unwrap_err()
                .contains("4 panes")
        );
        let bitmap = PictureSpec {
            bitmap: 2,
            ..spec()
        };
        assert!(
            picture_block(&g, 0x1234_0005, &bitmap)
                .unwrap_err()
                .contains("2 bitmaps")
        );
    }

    /// The pane set-up with a picture: after the description, the screen's
    /// tag is looked up, the block written, then the bitmap step on it.
    #[test]
    fn the_pane_set_up_adds_the_picture_after_the_description() {
        let mut g = screen_tag();
        g.put_u32(STACK + 4, 0x55);
        let shown = Arc::new(Shown::default());
        let d = Description {
            at: TABLES + DESCRIPTION_BLOCK,
            block: TextBlock {
                rect: [1, 2, 3, 4],
                font: 1,
                colour: [1.0; 3],
                animation: 0,
            },
            shown: Arc::clone(&shown),
            picture: Some(spec()),
            picture_at: TABLES + PICTURE_BLOCK,
            frame: None,
            frame_at: TABLES + FRAME_BLOCK,
            panel: Panel::default(),
            panel_at: 0,
            rows: None,
        };
        let f =
            move |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x77, Some(d.clone()), "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: match c.target {
                t if t == points::PANE_TEXT_ADD.base => 0x9000,
                t if t == points::SCREEN_TAG_BY_ID.base => 0x1234_0005,
                t if t == points::PANE_BITMAP_ADD.base => 0x9800,
                _ => 0,
            },
            ..Default::default()
        });
        assert_eq!(
            &run.calls[3..],
            [
                Gc::new(points::SCREEN_TAG_BY_ID.base).args([12]),
                Gc::new(points::PANE_BITMAP_ADD.base)
                    .esi(SCREEN)
                    .args([TABLES + PICTURE_BLOCK]),
            ]
        );
        let want = picture_block(&screen_tag(), 0x1234_0005, &spec()).unwrap();
        assert_eq!(run.guest.bytes(TABLES + PICTURE_BLOCK, 0x38).unwrap(), want);
        assert_eq!(shown.picture(SCREEN), Some(0x9800));
    }

    /// The frame comes after the picture: its own screen's tag, its block
    /// at the frame's place, the bitmap step on it, and no tint kept.
    #[test]
    fn the_pane_set_up_adds_the_frame_after_the_picture() {
        let mut g = screen_tag();
        g.put_u32(STACK + 4, 0x55);
        let shown = Arc::new(Shown::default());
        let frame = PictureSpec {
            at: [-259, 177],
            ..spec()
        };
        let d = Description {
            at: TABLES + DESCRIPTION_BLOCK,
            block: TextBlock {
                rect: [1, 2, 3, 4],
                font: 1,
                colour: [1.0; 3],
                animation: 0,
            },
            shown: Arc::clone(&shown),
            picture: Some(spec()),
            picture_at: TABLES + PICTURE_BLOCK,
            frame: Some(frame),
            frame_at: TABLES + FRAME_BLOCK,
            panel: Panel::default(),
            panel_at: 0,
            rows: None,
        };
        let f =
            move |cx| -> EntryFuture { Box::pin(setup_pane(cx, 0x77, Some(d.clone()), "test")) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let mut bitmaps = 0x9800;
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: match c.target {
                t if t == points::PANE_TEXT_ADD.base => 0x9000,
                t if t == points::SCREEN_TAG_BY_ID.base => 0x1234_0005,
                t if t == points::PANE_BITMAP_ADD.base => {
                    bitmaps += 0x100;
                    bitmaps
                }
                _ => 0,
            },
            ..Default::default()
        });
        assert_eq!(
            &run.calls[5..],
            [
                Gc::new(points::SCREEN_TAG_BY_ID.base).args([12]),
                Gc::new(points::PANE_BITMAP_ADD.base)
                    .esi(SCREEN)
                    .args([TABLES + FRAME_BLOCK]),
            ]
        );
        let want = picture_block(&screen_tag(), 0x1234_0005, &frame).unwrap();
        assert_eq!(run.guest.bytes(TABLES + FRAME_BLOCK, 0x38).unwrap(), want);
        // The picture keeps the tint, not the frame.
        assert_eq!(shown.picture(SCREEN), Some(0x9900));
    }

    /// Row text tints the picture for the focused row when the focus moves,
    /// and shows the row's bitmap in it.
    #[test]
    fn row_text_tints_the_picture_for_the_focused_row() {
        const ITEM: u32 = 0x0060_0000;
        const PICTURE: u32 = 0x0061_0000;
        let list = SCREEN + ui::LIST;
        let shown = Arc::new(Shown::default());
        shown.set_widget(SCREEN, 0x9000);
        shown.set_picture(SCREEN, PICTURE, 0xe58c_036c);
        let mut g = guest();
        g.put_u32(STACK + 4, ITEM).put_u32(STACK + 8, 0);
        const BLOCK: u32 = 0x0062_0000;
        g.map(ITEM, Item::SIZE).map(PICTURE, 0x90).map(BLOCK, 0x38);
        g.put_u32(PICTURE + WIDGET_BLOCK, BLOCK)
            .put_u32(BLOCK + BITMAP_HANDLE as u32, 0xe58c_036c);
        Item::at(ITEM)
            .set_row(&mut g, Some(DatumHandle::new(0x8001, 3)))
            .unwrap();
        let f = {
            let shown = Arc::clone(&shown);
            move |cx| -> EntryFuture {
                Box::pin(row_text(cx, Arc::new(Described), Some(Arc::clone(&shown)), None))
            }
        };
        let regs = Regs {
            esp: STACK,
            ecx: list,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |c, _| Regs {
            eax: match c.target {
                t if t == points::ITEM_TEXT.base => 0x7000,
                t if t == points::LIST_FOCUSED_ROW.base => DatumHandle::new(0x8001, 3).raw(),
                _ => 0,
            },
            ..Default::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0, 8)));
        let tint: Vec<f32> = (0..3)
            .map(|i| f32::from_bits(run.guest.u32(PICTURE + WIDGET_TINT + 4 * i).unwrap()))
            .collect();
        assert_eq!(tint, [0.5, 0.25, 1.0]);
        // And points it at the row's bitmap, in our copy of its block.
        assert_eq!(run.guest.u32(BLOCK + BITMAP_HANDLE as u32).unwrap(), 0xe6dd_04bb);
        let mut m = FakeGuest::new();
        m.map(PICTURE, 0x90);
        set_tint(&mut m, PICTURE, [1.0, 0.78, 0.3]).unwrap();
        assert_eq!(f32::from_bits(m.u32(PICTURE + 0x2c).unwrap()), 0.78);
    }

    /// A Controls-shaped tag: one pane with the full-width list and two
    /// frame bitmaps, the left one and the bottom-right one.
    fn controls_tag(g: &mut FakeGuest) -> u32 {
        const T: u32 = 0x0090_0000;
        g.map(T, 0x400);
        let (pane, list, bitmaps) = (T + 0x100, T + 0x200, T + 0x300);
        g.put_u32(T, 0x11)
            .put_u16(T + 4, 39)
            .put_u32(T + 0x2c, 0x0600_0233);
        g.put_u32(T + 0x20, 1).put_u32(T + 0x24, pane);
        g.put_u32(pane + 0x0c, 1).put_u32(pane + 0x10, list);
        g.put_u32(pane + 0x24, 2).put_u32(pane + 0x28, bitmaps);
        g.put_u32(pane + 0x1c, 0).put_u32(pane + 0x20, 0);
        // The list: skin 2, six visible, anchor (-254, 130), animation 21.
        g.put_u16(list + 4, 2)
            .put_u16(list + 6, 6)
            .put_u16(list + 8, (-254i16) as u16)
            .put_u16(list + 0xa, 130)
            .put_u16(list + 0xc, 21);
        g.put_u16(bitmaps + 0xc, (-280i16) as u16)
            .put_u32(bitmaps + 0x1c, 0xaaaa);
        let b1 = bitmaps + BITMAP_BLOCK_SIZE;
        g.put_u16(b1 + 0xc, 150).put_u32(b1 + 0x1c, 0xbbbb);
        T
    }

    const LAYOUT: ListLayout = ListLayout {
        skin: 0,
        anchor: [-215, 130],
        bitmaps_left_of: 0,
        visible: 0,
    };

    /// A layout asking for more rows than the six embedded item widgets:
    /// the copy's list block lays out that many, and the class's list gets
    /// the rest as item widgets of ours, a slot per live list.
    #[test]
    fn a_taller_layout_lays_out_more_rows_with_items_of_ours() {
        let mut g = guest();
        let retail = controls_tag(&mut g);
        let at = TABLES + TAG_COPY;
        let tall = ListLayout {
            visible: 15,
            ..LAYOUT
        };
        assert_eq!(copy_tag(&mut g, retail, at, &tall), Ok(Some(at)));
        let pane = g.u32(at + 0x24).unwrap();
        let list = g.u32(pane + 0x10).unwrap();
        assert_eq!(g.u16(list + 6), Ok(15), "the rows the page has room for");
        assert_eq!(tall.extra_items(), 9);
        assert_eq!(LAYOUT.extra_items(), 0);

        const VT: u32 = 0x00c0_0070;
        const POOL: u32 = 0x00c1_0000;
        EXTRA_ITEMS.lock().unwrap().push(ExtraItems {
            list_vtable: VT,
            extra: 9,
            pool: POOL,
            size: 0x1000,
            slots: Vec::new(),
            next: 0,
        });
        let first = extra_items(0x5000, VT);
        assert_eq!(first.len(), 9);
        assert_eq!(first[0], POOL);
        assert_eq!(first[8], POOL + 8 * 0x80);
        assert_eq!(extra_items(0x5000, VT), first, "the same list keeps its slot");
        let second = extra_items(0x6000, VT);
        assert_eq!(second[0], POOL + 9 * 0x80, "another list, the next slot");
        assert!(extra_items(0x5000, VT + 4).is_empty(), "not one of ours");
        EXTRA_ITEMS.lock().unwrap().retain(|e| e.list_vtable != VT);
    }

    #[test]
    fn the_tag_copy_moves_and_reskins_the_list_and_drops_the_right_frame() {
        let mut g = guest();
        let retail = controls_tag(&mut g);
        let at = TABLES + TAG_COPY;
        assert_eq!(copy_tag(&mut g, retail, at, &LAYOUT), Ok(Some(at)));
        // The root is the retail one but for its one pane.
        assert_eq!(g.u32(at), Ok(0x11));
        assert_eq!(g.u16(at + 4), Ok(39));
        assert_eq!(g.u32(at + 0x2c), Ok(0x0600_0233));
        assert_eq!(g.u32(at + 0x20), Ok(1));
        let pane = g.u32(at + 0x24).unwrap();
        assert!((at..at + TAG_COPY_SIZE).contains(&pane));
        // The list: ours in place and skin, the rest retail.
        assert_eq!(g.u32(pane + 0x0c), Ok(1));
        let list = g.u32(pane + 0x10).unwrap();
        assert_eq!(g.u16(list + 4), Ok(0));
        assert_eq!(g.u16(list + 6), Ok(6), "visible count kept");
        assert_eq!(g.u16(list + 8).map(|v| v as i16), Ok(-215));
        assert_eq!(g.u16(list + 0xa), Ok(130));
        assert_eq!(g.u16(list + 0xc), Ok(21), "animation kept");
        // Only the frame left of the centre.
        assert_eq!(g.u32(pane + 0x24), Ok(1));
        let kept = g.u32(pane + 0x28).unwrap();
        assert_eq!(g.u32(kept + 0x1c), Ok(0xaaaa));
        // The retail tag is untouched.
        assert_eq!(g.u32(retail + 0x20), Ok(1));
        assert_eq!(g.u16(0x0090_0200 + 4), Ok(2));
    }

    #[test]
    fn only_screens_of_a_class_with_a_layout_get_the_copy_made_once() {
        let mut g = guest();
        let retail = controls_tag(&mut g);
        const OURS: u32 = 0x00a0_0000;
        const THEIRS: u32 = 0x00a1_0000;
        g.map(OURS, 4).map(THEIRS, 4);
        g.put_u32(OURS, TABLES)
            .put_u32(THEIRS, CONTROLS_SCREEN_VTABLE.base);
        LAYOUTS.lock().unwrap().push(RegisteredLayout {
            screen: "test",
            screen_vtable: TABLES,
            list_vtable: entries().list_vtable,
            list: LAYOUT,
            scroll_arrows: None,
        });
        assert!(has_layout(TABLES));
        assert_eq!(served_tag(&mut g, THEIRS, retail), Ok(None));
        assert_eq!(served_tag(&mut g, OURS, 0), Ok(None), "no retail tag");
        let at = TABLES + TAG_COPY;
        assert_eq!(served_tag(&mut g, OURS, retail), Ok(Some(at)));
        assert_eq!(g.u32(TABLES + TAG_SOURCE), Ok(retail));
        // Made once: a change to our copy stays until the retail tag moves.
        g.put_u32(at, 0x77);
        assert_eq!(served_tag(&mut g, OURS, retail), Ok(Some(at)));
        assert_eq!(g.u32(at), Ok(0x77));
        LAYOUTS
            .lock()
            .unwrap()
            .retain(|layout| layout.screen_vtable != TABLES);
    }

    /// Slot 10 of a class with an action: Start runs it and is handled;
    /// anything else goes to the Controls handler.
    #[test]
    fn x_and_start_run_the_action_and_the_rest_go_to_the_original() {
        const EVENT: u32 = 0x0061_0000;
        let ran = Arc::new(AtomicU32::new(0));
        let run_with = |button: u32| {
            let mut g = guest();
            g.map(EVENT, 0x10);
            g.put_u32(EVENT, BUTTON_EVENT).put_u32(EVENT + 8, button);
            g.put_u32(STACK + 4, EVENT);
            let ran = Arc::clone(&ran);
            let action: Action = Arc::new(move |_, screen| {
                ran.store(screen, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            });
            let f = move |cx| -> EntryFuture {
                Box::pin(event(cx, 0x2300ea, Some(Arc::clone(&action)), None, None))
            };
            let regs = Regs {
                esp: STACK,
                ecx: SCREEN,
                ..Default::default()
            };
            testing::run(&f, g, regs, |_, _| Regs {
                eax: 7,
                ..Default::default()
            })
        };
        for button in [START_BUTTON, X_BUTTON] {
            let run = run_with(button);
            assert_eq!(run.result, Ok(Ret::new(1, 4)), "button {button}");
            assert_eq!(run.calls, []);
            assert_eq!(ran.swap(0, Ordering::SeqCst), SCREEN, "button {button}");
        }
        let run = run_with(1);
        assert_eq!(run.calls, [Gc::new(0x2300ea).ecx(SCREEN).args([EVENT])]);
        assert_eq!(run.result, Ok(Ret::new(7, 4)));
        assert_eq!(ran.load(Ordering::SeqCst), 0, "B is not Start");
    }

    /// A class's own event hook comes first: a handled event stops there
    /// (before the sideways steps and the list), a passed one goes on to
    /// the Controls handler (B still closes).
    #[test]
    fn a_class_s_own_event_hook_is_asked_first() {
        const EVENT: u32 = 0x0061_0000;
        let seen = Arc::new(Mutex::new(Vec::new()));
        let run_with = |kind: u32, button: u32| {
            let mut g = guest();
            g.map(EVENT, 0x10);
            g.put_u32(EVENT, kind).put_u32(EVENT + 8, button);
            g.put_u32(STACK + 4, EVENT);
            let seen = Arc::clone(&seen);
            let buttons: Buttons = Arc::new(move |k, b| {
                seen.lock().unwrap().push((k, b));
                if k == LEFT_EVENT {
                    Button::Handled
                } else {
                    Button::Pass
                }
            });
            let sideways: Arc<dyn Rows> = Arc::new(Stepping::default());
            let f = move |cx| -> EntryFuture {
                Box::pin(event(
                    cx,
                    0x2300ea,
                    None,
                    Some(Arc::clone(&buttons)),
                    Some(Arc::clone(&sideways)),
                ))
            };
            let regs = Regs {
                esp: STACK,
                ecx: SCREEN,
                ..Default::default()
            };
            testing::run(&f, g, regs, |_, _| Regs {
                eax: 7,
                ..Default::default()
            })
        };
        let run = run_with(LEFT_EVENT, 10);
        assert_eq!(run.result, Ok(Ret::new(1, 4)));
        assert_eq!(
            run.calls,
            [],
            "handled: no focus asked, the list never sees it"
        );
        let run = run_with(BUTTON_EVENT, 1);
        assert_eq!(run.calls, [Gc::new(0x2300ea).ecx(SCREEN).args([EVENT])]);
        let run = run_with(BUTTON_EVENT, START_BUTTON);
        assert_eq!(run.calls.len(), 1, "no action: Start goes on too");
        assert_eq!(
            *seen.lock().unwrap(),
            [
                (LEFT_EVENT, 10),
                (BUTTON_EVENT, 1),
                (BUTTON_EVENT, START_BUTTON)
            ]
        );
    }

    /// Rows that step sideways: what they were asked, and a line for rows
    /// 0 to 2.
    #[derive(Default)]
    struct Stepping(Mutex<Vec<(i16, bool)>>);
    impl Rows for Stepping {
        fn title(&self) -> u32 {
            0
        }
        fn count(&self) -> usize {
            3
        }
        fn texts(&self, _: i16) -> Vec<(u32, u32)> {
            Vec::new()
        }
        fn press(&self, _: i16) -> Chosen {
            Chosen::Nothing
        }
        fn sideways(&self, row: i16, right: bool) -> Option<String> {
            self.0.lock().unwrap().push((row, right));
            (0..3).contains(&row).then(|| format!("stepped {row}"))
        }
    }

    /// Slot 10 of a class with sideways steps: left (kind 2) and right
    /// (kind 4) ask the list's focused row (`0x24c447`, EAX the list) and
    /// step it, handled; up, down, buttons and a row with nothing to step
    /// go to the Controls handler. Without sideways steps they all do.
    #[test]
    fn left_and_right_step_the_focused_row() {
        const EVENT: u32 = 0x0061_0000;
        let run_with = |rows: &Arc<Stepping>, sideways: bool, kind: u32, focused: u32| {
            let mut g = guest();
            g.map(EVENT, 0x10);
            g.put_u32(EVENT, kind).put_u32(EVENT + 8, 16);
            g.put_u32(STACK + 4, EVENT);
            let rows: Arc<dyn Rows> = Arc::clone(rows) as Arc<dyn Rows>;
            let f = move |cx| -> EntryFuture {
                let sideways = sideways.then(|| Arc::clone(&rows));
                Box::pin(event(cx, 0x2300ea, None, None, sideways))
            };
            let regs = Regs {
                esp: STACK,
                ecx: SCREEN,
                ..Default::default()
            };
            testing::run(&f, g, regs, move |c, _| Regs {
                eax: if c.target == points::LIST_FOCUSED_ROW.base {
                    focused
                } else {
                    7
                },
                ..Default::default()
            })
        };
        let focus = Gc::new(points::LIST_FOCUSED_ROW.base).eax(SCREEN + ui::LIST);
        let original = Gc::new(0x2300ea).ecx(SCREEN).args([EVENT]);
        let rows = Arc::new(Stepping::default());
        // Row 2's datum handle (a salt in the top half).
        let run = run_with(&rows, true, LEFT_EVENT, 0xe1a2_0002);
        assert_eq!(run.result, Ok(Ret::new(1, 4)));
        assert_eq!(run.calls, std::slice::from_ref(&focus));
        let run = run_with(&rows, true, RIGHT_EVENT, 0xe1a2_0001);
        assert_eq!(run.result, Ok(Ret::new(1, 4)));
        assert_eq!(run.calls, std::slice::from_ref(&focus));
        assert_eq!(*rows.0.lock().unwrap(), [(2, false), (1, true)]);
        // A row with nothing to step, and no focus: the original's.
        let run = run_with(&rows, true, RIGHT_EVENT, 0xe1a2_0005);
        assert_eq!(run.calls, [focus.clone(), original.clone()]);
        assert_eq!(run.result, Ok(Ret::new(7, 4)));
        let run = run_with(&rows, true, LEFT_EVENT, u32::MAX);
        assert_eq!(run.calls, [focus.clone(), original.clone()]);
        rows.0.lock().unwrap().clear();
        // Up, down and a button are the list's; a class without sideways
        // steps leaves left and right to it too.
        for kind in [1, 3, BUTTON_EVENT] {
            let run = run_with(&rows, true, kind, 0xe1a2_0001);
            assert_eq!(run.calls, std::slice::from_ref(&original), "kind {kind}");
        }
        for kind in [LEFT_EVENT, RIGHT_EVENT] {
            let run = run_with(&rows, false, kind, 0xe1a2_0001);
            assert_eq!(run.calls, std::slice::from_ref(&original), "kind {kind}");
        }
        assert!(rows.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_class_with_an_action_gets_our_event_slot() {
        let mut g = guest();
        let e = Entries {
            event: 0xa0f0_0050,
            ..entries()
        };
        write_vtables(&mut g, &e).unwrap();
        assert_eq!(ui::slot(&g, TABLES, SCREEN_EVENT_SLOT), Ok(0xa0f0_0050));
        let mut g = guest();
        write_vtables(&mut g, &entries()).unwrap();
        assert_eq!(
            ui::slot(&g, TABLES, SCREEN_EVENT_SLOT),
            Ok(0x5000 + SCREEN_EVENT_SLOT),
            "no action: the Controls handler"
        );
    }

    /// Slot 0 of a class told of frees: ours, and the Controls destructor
    /// runs first, then the class hears of it with the screen.
    #[test]
    fn a_class_told_of_frees_gets_our_destructor() {
        let mut g = guest();
        let e = Entries {
            destructor: 0xa0f0_0060,
            ..entries()
        };
        write_vtables(&mut g, &e).unwrap();
        assert_eq!(
            ui::slot(&g, TABLES, SCREEN_DESTRUCTOR_SLOT),
            Ok(0xa0f0_0060)
        );
        let mut g = guest();
        write_vtables(&mut g, &entries()).unwrap();
        assert_eq!(
            ui::slot(&g, TABLES, SCREEN_DESTRUCTOR_SLOT),
            Ok(0x5000 + SCREEN_DESTRUCTOR_SLOT),
            "none: the Controls destructor"
        );

        let told = Arc::new(AtomicU32::new(0));
        let mut g = guest();
        g.put_u32(STACK + 4, 0);
        let on_free: OnFree = {
            let told = Arc::clone(&told);
            Arc::new(move |_, screen| {
                told.store(screen, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            })
        };
        let f = move |cx| -> EntryFuture { Box::pin(destructor(cx, 0x1234, Arc::clone(&on_free))) };
        let regs = Regs {
            esp: STACK,
            ecx: SCREEN,
            ..Default::default()
        };
        let run = testing::run(&f, g, regs, |_, _| Regs {
            eax: SCREEN,
            ..Default::default()
        });
        assert_eq!(run.calls, [Gc::new(0x1234).ecx(SCREEN).args([0])]);
        assert_eq!(run.result, Ok(Ret::new(SCREEN, 4)));
        assert_eq!(told.load(Ordering::SeqCst), SCREEN);
    }
}
