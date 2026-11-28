//! The menus on the overlay stage (docs/graphics/native-post-hud.md, "The
//! menus on the overlay"; docs/graphics/retire-d3d.md, slice R7): which of
//! a screen's sorted widgets the host draws from the scene's overlay, where
//! in the frame, and which of the game's widget draws it then leaves out.
//!
//! One list is one call of `0x002353a5` (Code, `h2 disasm`): it collects
//! a screen's shown widgets (`0x0023566a`), sorts them with the C
//! runtime's `qsort` and the comparator `0x002356d8`, calls the screen's
//! pre-draw slot, takes the depth window, and calls the draw (`+0x10`, at
//! `points::ui_render::ITEM_CALL`) of each entry whose depth is inside it.
//! At [`LIST_READY`] the list is read as the guest sorted it ([`at_list`]);
//! each drawn entry's widget is read and listed (`scene_extract::ui`), and
//! consecutive drawn entries that list exactly form a run ([`plan`]). At a
//! run's first draw the host places one overlay pass with the run's items
//! (`super::Host::ui_pass`), and each claimed entry's draw is skipped as a
//! host-drawn widget's is. At [`LIST_DONE`] the claim closes; a list whose
//! skipped draws are not its claimed ones is counted and its screen is not
//! claimed again until the next map ([`close`]). A bitmap widget's draw
//! is read by [`bitmaps`]. A shader bitmap's draw is the game's here and
//! its quad is the overlay's at the shader quad
//! (`super::hud_overlay::menu_shader_quad`): it ends a run and is not
//! refused.

pub mod bitmaps;

mod list;
pub(super) mod model;
mod text;

pub use scene_extract::ui::{BUTTON_GLYPHS, Justify, TextLayout, UiText};
use std::cell::RefCell;

use engine::memory::GuestMemory;
pub use engine::points::ui_render::{LIST_DONE, LIST_READY};
use frame_scene::Overlay;
use scene_extract::GuestRead;
use scene_extract::ui::{self as walk, NoText, UiDraw, UiFrame, UiScreen, Unread};
use timeline::native::Ui;

/// `0x002353a5`'s frame: the entry count at `[ebp-4]`, the window's near
/// and far depths at `[ebp-8]` and `[ebp-0xc]`, the entries from
/// `[ebp-0xc0c]`, `0xc` bytes each (the widget, its depth, its bias), the
/// user argument at `[ebp+8]`.
const COUNT_FROM_EBP: u32 = 4;
const NEAR_FROM_EBP: u32 = 8;
const FAR_FROM_EBP: u32 = 0xc;
const LIST_FROM_EBP: u32 = 0xc0c;
const ENTRY_BYTES: u32 = 0xc;
const ENTRY_DEPTH: u32 = 4;
const ENTRY_BIAS: u32 = 8;
const USER_FROM_EBP: u32 = 8;

/// The screen quads' viewport (`0x00035b90` reads it): four `i16`, top,
/// left, bottom, right.
pub const SCREEN_RECT: u32 = 0x0048_5648;
/// The one viewport the overlay draws menus in: the full UI canvas.
pub const FULL_SCREEN: [i16; 4] = [0, 0, walk::CANVAS[1] as i16, walk::CANVAS[0] as i16];

/// One entry of the sorted list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListEntry {
    pub widget: u32,
    pub depth: f32,
    pub bias: i16,
}

/// One sorted list at [`LIST_READY`].
#[derive(Clone, Debug, PartialEq)]
pub struct UiList {
    /// `0x002353a5`'s EBP: names the list.
    pub frame: u32,
    /// The screen widget (EDI).
    pub root: u32,
    pub user: u32,
    /// `[near, far]`: an entry is drawn when its depth is inside.
    pub window: [f32; 2],
    pub entries: Vec<ListEntry>,
}

/// Why a list, or one drawn widget of it, is the game's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRefusal {
    /// The screen quads' viewport is not the full screen.
    Frame,
    /// The screen's list did not match its claim earlier on the map.
    Mismatched,
    /// The widget's draw is not read yet: the draw and the first check
    /// that refused it.
    Widget(Gap),
    /// A bitmap widget's draw the overlay does not make.
    Bitmap(bitmaps::Refusal),
    /// The overlay refused the widget's items.
    Unlisted,
    /// The host did not place the run's pass.
    Pass,
}

impl From<Unread> for MenuRefusal {
    fn from(why: Unread) -> Self {
        MenuRefusal::Widget(why.into())
    }
}

impl From<Gap> for MenuRefusal {
    fn from(why: Gap) -> Self {
        MenuRefusal::Widget(why)
    }
}

/// A drawn widget the overlay does not read, named: the draw it is
/// (`draw`, its `Ui` counter) and the first check on its path that
/// refused it (`cause`, its `scene_kept_ui_widget_<cause>`). A gap with
/// no finer name has the draw's own (`text`, `draw`, `model`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gap {
    pub draw: Unread,
    pub cause: &'static str,
}

impl Gap {
    pub const fn new(draw: Unread, cause: &'static str) -> Self {
        Gap { draw, cause }
    }

    pub const fn text(cause: &'static str) -> Self {
        Gap::new(Unread::Text, cause)
    }

    pub const fn class(cause: &'static str) -> Self {
        Gap::new(Unread::Draw, cause)
    }

    pub const fn model(cause: &'static str) -> Self {
        Gap::new(Unread::Model, cause)
    }
}

impl From<Unread> for Gap {
    fn from(draw: Unread) -> Self {
        let cause = match draw {
            Unread::Bitmap => "bitmap_state",
            Unread::Shader => "shader",
            Unread::Text => "text",
            Unread::Model => "model",
            Unread::Draw => "draw",
            Unread::Unreadable => "unreadable",
        };
        Gap { draw, cause }
    }
}

impl MenuRefusal {
    pub fn counter(self) -> Ui {
        match self {
            MenuRefusal::Frame => Ui::OverlayMenuRefusedFrame,
            MenuRefusal::Mismatched => Ui::OverlayMenuRefusedMismatched,
            // Read by `bitmaps`: never a reason of its own.
            MenuRefusal::Widget(Gap { draw: Unread::Bitmap, .. }) => Ui::OverlayMenuRefusedBitmapState,
            MenuRefusal::Bitmap(why) => why.counter(),
            // Drawn at its quad (`plan`): never a reason of its own.
            MenuRefusal::Widget(Gap { draw: Unread::Shader, .. }) => Ui::OverlayMenuShaderFallback,
            MenuRefusal::Widget(Gap { draw: Unread::Text, .. }) => Ui::OverlayMenuRefusedText,
            MenuRefusal::Widget(Gap { draw: Unread::Model, .. }) => Ui::OverlayMenuRefusedModel,
            MenuRefusal::Widget(Gap { draw: Unread::Draw, .. }) => Ui::OverlayMenuRefusedDraw,
            MenuRefusal::Widget(Gap { draw: Unread::Unreadable, .. }) => Ui::OverlayMenuRefusedUnreadable,
            MenuRefusal::Unlisted => Ui::OverlayMenuRefusedUnlisted,
            MenuRefusal::Pass => Ui::OverlayMenuRefusedPass,
        }
    }

    /// What the native menus cannot draw, named: no guest draw takes it.
    pub fn panic(self) -> ! {
        match self {
            MenuRefusal::Frame => unimplemented!("menus: viewport not the full screen"),
            MenuRefusal::Mismatched => unimplemented!("menus: claim mismatched"),
            MenuRefusal::Widget(gap) => unimplemented!("menus: {:?} {}", gap.draw, gap.cause),
            MenuRefusal::Bitmap(why) => unimplemented!("menus: bitmap {why:?}"),
            MenuRefusal::Unlisted => unimplemented!("menus: unlisted"),
            MenuRefusal::Pass => unimplemented!("menus: pass"),
        }
    }
}

/// Consecutive drawn entries the overlay draws: one pass at the first.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// The entries' ordinals, in draw order.
    pub entries: Vec<u32>,
    /// Their items, in that order.
    pub part: Overlay,
    /// `None` until its first entry is drawn; then whether it was placed
    /// (its pass, when it has items) and its entries are left out.
    pub placed: Option<bool>,
}

/// One list's claim: its runs and the draws left out so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub frame: u32,
    pub root: u32,
    /// Per entry: the run it is in.
    pub run_of: Vec<Option<usize>>,
    /// A guest draw ran after planning. Later runs need fresh widget state.
    guest_drew: bool,
    pub runs: Vec<Run>,
    pub skipped: u32,
}

impl Claim {
    /// The draws the claim leaves out: every entry of a run not refused.
    pub fn claimed(&self) -> u32 {
        self.runs
            .iter()
            .filter(|r| r.placed != Some(false))
            .map(|r| r.entries.len() as u32)
            .sum()
    }
}

/// What a draw of the list does under its claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// The game draws it.
    Game,
    /// Left out.
    Skip,
    /// The first entry of run `n`: the run is placed here ([`placed`]),
    /// then the entry is left out, or, not placed, the run is the game's.
    Place(usize),
}

thread_local! {
    /// The game thread's open claims, innermost last.
    static CLAIMS: RefCell<Vec<Claim>> = const { RefCell::new(Vec::new()) };
}

pub(super) struct Ro<'a>(pub(super) &'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The live portion of the sort frame shared by list planning and a run
/// refreshed after an intervening guest draw.
struct ListFrame {
    first: u32,
    count: u32,
    user: u32,
}

impl ListFrame {
    fn read(mem: &dyn GuestMemory, ebp: u32) -> Option<Self> {
        let count = mem.u32(ebp.checked_sub(COUNT_FROM_EBP)?).ok()?;
        if count == 0 || count as usize > walk::MAX_LISTED {
            return None;
        }
        Some(Self {
            first: ebp.checked_sub(LIST_FROM_EBP)?,
            count,
            user: mem.u32(ebp.checked_add(USER_FROM_EBP)?).ok()?,
        })
    }

    fn entry(&self, mem: &dyn GuestMemory, ordinal: u32) -> Option<ListEntry> {
        if ordinal >= self.count {
            return None;
        }
        let at = self.first.checked_add(ordinal * ENTRY_BYTES)?;
        let mut words = [0; ENTRY_BIAS as usize + 2];
        mem.read(at, &mut words).ok()?;
        Some(ListEntry {
            widget: u32::from_le_bytes(words[..4].try_into().ok()?),
            depth: f32::from_le_bytes(
                words[ENTRY_DEPTH as usize..ENTRY_BIAS as usize].try_into().ok()?,
            ),
            bias: i16::from_le_bytes(words[ENTRY_BIAS as usize..].try_into().ok()?),
        })
    }
}

/// The sorted list at LIST_READY (EBP, EDI), if the frame reads.
pub fn at_list(mem: &dyn GuestMemory, ebp: u32, edi: u32) -> Option<UiList> {
    let frame = ListFrame::read(mem, ebp)?;
    let depth = |n: u32| mem.f32(ebp.checked_sub(n)?).ok();
    Some(UiList {
        frame: ebp,
        root: edi,
        user: frame.user,
        window: [depth(NEAR_FROM_EBP)?, depth(FAR_FROM_EBP)?],
        entries: (0..frame.count)
            .map(|ordinal| frame.entry(mem, ordinal))
            .collect::<Option<Vec<_>>>()?,
    })
}

/// The screen quads' viewport, when it reads.
pub fn screen_rect(mem: &dyn GuestMemory) -> Option<[i16; 4]> {
    let word = |n: u32| mem.u16(SCREEN_RECT + 2 * n).ok().map(|w| w as i16);
    Some([word(0)?, word(1)?, word(2)?, word(3)?])
}

/// Where a list class the host registered at run time puts its scroll
/// arrows (`Host::ui_list_arrows`): by its vtable and its rectangle's
/// height, the up and down arrows' offsets from the list's animated left
/// and bottom; `None` for a class the host does not know.
pub type ListArrows<'a> = &'a dyn Fn(u32, i32) -> Option<[[i16; 2]; 2]>;

/// Read one sorted entry in the pixel order of its virtual draw.
/// `viewport` is the list's user argument, the view a bitmap widget's draw
/// places it in.
fn entry_frame(mem: &dyn GuestRead, e: &ListEntry, viewport: u32, arrows: ListArrows) -> Result<UiFrame, MenuRefusal> {
    use walk::draws;
    let draw = walk::widget_draw(mem, e.widget)?;
    let mut frame = UiFrame::default();
    let widget = match draw {
        draws::TEXT | draws::BUTTON => {
            // The draw receives the caller's viewport independently of SCREEN_RECT.
            if viewport == 0 || mem.read_u32(viewport) != Some(0)
                || viewport.checked_add(4).and_then(|p| mem.read_u32(p))
                    != Some((walk::CANVAS[0] << 16) | walk::CANVAS[1]) {
                return Err(Gap::text("text_viewport").into());
            }
            if draw == draws::BUTTON {
                frame.draws.extend(list::button_art(mem, e.widget, viewport)?.map(UiDraw::Bitmap));
            }
            text::read_widget(mem, e.widget, e.depth, draw == draws::BUTTON)?
        }
        draws::LIST => {
            frame.draws.extend(list::arrows(mem, e.widget, viewport, arrows)?.into_iter().map(UiDraw::Bitmap));
            return Ok(frame);
        }
        draws::MODEL => walk::read_widget_header(mem, e.widget, e.depth, model::read(mem, e.widget)?)?,
        _ => match walk::read_widget(mem, e.widget, e.depth) {
            Ok(widget) => widget,
            Err(Unread::Bitmap) => {
                match bitmaps::read(mem, e.widget, e.depth, viewport).map_err(MenuRefusal::Bitmap)? {
                    bitmaps::Drawn::Widget(widget) => widget,
                    // A scrolling bitmap: its quads, in its draw's order.
                    bitmaps::Drawn::Quads(quads) => {
                        frame.draws.extend(quads.into_iter().map(UiDraw::Bitmap));
                        return Ok(frame);
                    }
                }
            }
            // A class whose draw the overlay does not know: a runtime
            // class or a retail draw not read yet.
            Err(Unread::Draw) => return Err(Gap::class("draw_class").into()),
            Err(why) => return Err(why.into()),
        },
    };
    frame.draws.push(UiDraw::Screen(UiScreen {
        backdrop: None,
        widgets: vec![walk::UiWidget { bias: e.bias, ..widget }],
    }));
    Ok(frame)
}

/// List the whole entry atomically, including both art and label for buttons.
fn part(mem: &dyn GuestRead, e: &ListEntry, viewport: u32, fonts: &dyn UiText, arrows: ListArrows) -> Result<Overlay, MenuRefusal> {
    let frame = entry_frame(mem, e, viewport, arrows)?;
    let mut part = Overlay::new(walk::CANVAS);
    if walk::draw_list(&frame, fonts, &mut part)
        .iter()
        .any(|l| l.note.is_some())
    {
        return Err(MenuRefusal::Unlisted);
    }
    Ok(part)
}

/// `list`'s claim over `mem`: its drawn entries that list exactly, in runs
/// of consecutive drawn entries, and each drawn entry the game keeps, with
/// why. An entry outside the window is not drawn and ends no run.
pub fn plan(mem: &dyn GuestRead, list: &UiList) -> (Claim, Vec<(u32, MenuRefusal)>) {
    plan_with_text(mem, list, &NoText, &|_| false, &|_, _| None)
}

fn plan_with_text(
    mem: &dyn GuestRead,
    list: &UiList,
    fonts: &dyn UiText,
    custom: &dyn Fn(u32) -> bool,
    arrows: ListArrows,
) -> (Claim, Vec<(u32, MenuRefusal)>) {
    let mut runs: Vec<Run> = Vec::new();
    let mut refused = Vec::new();
    let mut open = false;
    for (i, e) in list.entries.iter().enumerate() {
        if !walk::in_window(e.depth, list.window) {
            continue;
        }
        if custom(e.widget) {
            refused.push((i as u32, MenuRefusal::from(Gap::class("draw_custom"))));
            open = false;
            continue;
        }
        let items = match part(mem, e, list.user, fonts, arrows) {
            Ok(items) => items,
            Err(MenuRefusal::Widget(Gap { draw: Unread::Shader, .. })) => {
                // A shader bitmap: the game draws it, and its quad is drawn
                // from the overlay at the shader quad (`super::hud_overlay::
                // menu_shader_quad`). It ends the run, so order holds.
                open = false;
                continue;
            }
            Err(why) => {
                refused.push((i as u32, why));
                open = false;
                continue;
            }
        };
        if !open {
            runs.push(Run {
                entries: Vec::new(),
                part: Overlay::new(walk::CANVAS),
                placed: None,
            });
            open = true;
        }
        let run = runs.last_mut().expect("a run is open");
        if run.part.append(&items).is_err() {
            // Not drawable in one run: this entry is the game's.
            refused.push((i as u32, MenuRefusal::Unlisted));
            open = false;
            continue;
        }
        run.entries.push(i as u32);
    }
    runs.retain(|r| !r.entries.is_empty());
    let mut run_of = vec![None; list.entries.len()];
    for (n, run) in runs.iter().enumerate() {
        for &e in &run.entries {
            run_of[e as usize] = Some(n);
        }
    }
    let claim = Claim {
        frame: list.frame,
        root: list.root,
        run_of,
        guest_drew: false,
        runs,
        skipped: 0,
    };
    (claim, refused)
}

/// Opens `claim` over the claims open, first closing any at its frame or
/// inside it (a list left without its end): those that mismatched.
pub fn open(claim: Claim) -> Vec<Claim> {
    let stale = close(claim.frame);
    if !claim.runs.is_empty() {
        CLAIMS.with(|c| c.borrow_mut().push(claim));
    }
    stale
}

/// The draw of entry `ordinal` of the list whose frame is `frame`.
pub fn at_item(frame: u32, ordinal: u32) -> Item {
    CLAIMS.with(|c| {
        let mut claims = c.borrow_mut();
        let Some(claim) = claims.iter_mut().rev().find(|c| c.frame == frame) else {
            return Item::Game;
        };
        let Some(Some(run)) = claim.run_of.get(ordinal as usize).copied() else {
            claim.guest_drew = true;
            return Item::Game;
        };
        match claim.runs[run].placed {
            Some(false) => Item::Game,
            Some(true) => {
                claim.skipped += 1;
                Item::Skip
            }
            None if claim.runs[run].entries.first() == Some(&ordinal) => Item::Place(run),
            // A later entry of a run whose first was not drawn: the run is
            // not placed, and the game draws it.
            None => {
                claim.guest_drew = true;
                Item::Game
            },
        }
    })
}

/// The items of run `run` of the claim at `frame`, to place. A run is
/// placed once, at its first entry, so its planned items are moved out,
/// not copied.
pub fn run_part(frame: u32, run: usize, mem: &dyn GuestMemory) -> Option<Overlay> {
    let pending = CLAIMS.with(|c| {
        let mut claims = c.borrow_mut();
        let claim = claims.iter_mut().rev().find(|c| c.frame == frame)?;
        let guest_drew = claim.guest_drew;
        let run = claim.runs.get_mut(run)?;
        Some(if guest_drew {
            Err(run.entries.clone())
        } else {
            Ok(std::mem::replace(&mut run.part, Overlay::new(walk::CANVAS)))
        })
    })?;
    let ordinals = match pending {
        Ok(part) => return Some(part),
        Err(refresh) => refresh,
    };
    if screen_rect(mem) != Some(FULL_SCREEN) {
        return None;
    }
    let list = ListFrame::read(mem, frame)?;
    let host = super::host()?;
    let fonts = host.ui_text(mem);
    let mut refreshed = Overlay::new(walk::CANVAS);
    for ordinal in ordinals {
        let entry = list.entry(mem, ordinal)?;
        if host.ui_custom_widget(mem, entry.widget) {
            return None;
        }
        let arrows = |vtable, height| host.ui_list_arrows(vtable, height);
        let items = part(&Ro(mem), &entry, list.user, fonts.as_deref().unwrap_or(&NoText), &arrows).ok()?;
        refreshed.append(&items).ok()?;
    }
    Some(refreshed)
}

/// Entry `ordinal` of the list at `frame` was left out: the guest state
/// its draw writes, written in its place (a text or button's reveal
/// cursor, `text::left_out`). Other draws write none the overlay reads.
pub fn left_out(mem: &mut dyn GuestMemory, frame: u32, ordinal: u32) {
    use walk::draws;
    let Some(widget) = ListFrame::read(&*mem, frame).and_then(|l| l.entry(&*mem, ordinal)).map(|e| e.widget) else {
        return;
    };
    match walk::widget_draw(&Ro(&*mem), widget) {
        Ok(draws::TEXT) => text::left_out(mem, widget, false),
        Ok(draws::BUTTON) => text::left_out(mem, widget, true),
        _ => {}
    }
}

/// Records whether run `run` of the claim at `frame` was placed; when it
/// was, its first entry is left out (counted as skipped).
pub fn placed(frame: u32, run: usize, placed: bool) {
    CLAIMS.with(|c| {
        let mut claims = c.borrow_mut();
        let Some(claim) = claims.iter_mut().rev().find(|c| c.frame == frame) else {
            return;
        };
        if let Some(r) = claim.runs.get_mut(run) {
            r.placed = Some(placed);
            claim.guest_drew |= !placed;
            if placed {
                claim.skipped += 1;
            }
        }
    });
}

/// Closes the claim at `frame` and every one opened after it: those whose
/// skipped draws are not their claimed ones (the caller panics on any).
pub fn close(frame: u32) -> Vec<Claim> {
    let closed: Vec<Claim> = CLAIMS.with(|c| {
        let mut claims = c.borrow_mut();
        match claims.iter().position(|c| c.frame == frame) {
            Some(at) => claims.drain(at..).collect(),
            None => Vec::new(),
        }
    });
    let mismatched: Vec<Claim> = closed
        .into_iter()
        .filter(|c| c.skipped != c.claimed())
        .collect();
    mismatched
}

/// Forgets the open claims (a snapshot load, a new map).
pub fn forget() {
    CLAIMS.with(|c| c.borrow_mut().clear());
}

/// Check the screen before reading its sorted entries.
pub(super) fn check_frame(mem: &dyn GuestMemory) -> Result<(), MenuRefusal> {
    if screen_rect(mem) != Some(FULL_SCREEN) {
        return Err(MenuRefusal::Frame);
    }
    Ok(())
}

/// Claim the list at LIST_READY. A drawn widget the overlay does not
/// draw panics, named ([`MenuRefusal::panic`]).
pub(super) fn claim_list(mem: &dyn GuestMemory, list: &UiList, host: &dyn super::Host) -> Claim {
    let fonts = host.ui_text(mem);
    let custom = |widget| host.ui_custom_widget(mem, widget);
    let arrows = |vtable, height| host.ui_list_arrows(vtable, height);
    let (claim, refused) = plan_with_text(&Ro(mem), list, fonts.as_deref().unwrap_or(&NoText), &custom, &arrows);
    if let Some((_, why)) = refused.into_iter().next() {
        why.panic();
    }
    claim
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;
    use scene_extract::testing::Mem;
    use scene_extract::ui::draws;

    const NONE_VTABLE: u32 = 0x5000;
    const TEXT_VTABLE: u32 = 0x5100;

    /// Five widgets: containers, but the third is text.
    fn widgets() -> Mem {
        let mut m = Mem::default();
        m.w(NONE_VTABLE + 0x10, draws::NONE);
        m.w(TEXT_VTABLE + 0x10, draws::TEXT);
        for n in 0..5u32 {
            let w = 0x1000 + 0x100 * n;
            m.w(w, if n == 2 { TEXT_VTABLE } else { NONE_VTABLE });
            m.w(w + 4, 10);
            m.w(w + 0x20, 0);
            m.w(w + 0x24, 0);
            m.f(w + 0x58, 0.0);
            m.f(w + 0x5c, 0.0);
            m.f(w + 0x64, 1.0);
            m.w(w + 0x68, 0);
        }
        m
    }

    fn list(frame: u32, depths: [f32; 5]) -> UiList {
        UiList {
            frame,
            root: 0x1000,
            user: 0,
            window: [-1.0, 1.0],
            entries: depths
                .iter()
                .enumerate()
                .map(|(n, &depth)| ListEntry {
                    widget: 0x1000 + 0x100 * n as u32,
                    depth,
                    bias: 0,
                })
                .collect(),
        }
    }

    /// The sorted list reads as the guest left it: the count, the window,
    /// each entry's widget, depth and bias. An empty or overfull list
    /// does not.
    #[test]
    fn the_sorted_list_reads_from_the_frame() {
        let mut mem = FakeGuest::new();
        let (ebp, root) = (0x9000, 0x1000);
        let first = ebp - LIST_FROM_EBP;
        mem.put_u32(ebp - COUNT_FROM_EBP, 2)
            .put_f32(ebp - NEAR_FROM_EBP, -0.5)
            .put_f32(ebp - FAR_FROM_EBP, 2.0)
            .put_u32(ebp + USER_FROM_EBP, 1)
            .put_u32(first, 0x1100)
            .put_f32(first + 4, 0.25)
            .put_u16(first + 8, 0xfffd)
            .put_u32(first + 12, 0x1200)
            .put_f32(first + 16, 3.0)
            .put_u16(first + 20, 4);
        let list = at_list(&mem, ebp, root).unwrap();
        assert_eq!(
            list,
            UiList {
                frame: ebp,
                root,
                user: 1,
                window: [-0.5, 2.0],
                entries: vec![
                    ListEntry {
                        widget: 0x1100,
                        depth: 0.25,
                        bias: -3
                    },
                    ListEntry {
                        widget: 0x1200,
                        depth: 3.0,
                        bias: 4
                    },
                ],
            }
        );
        mem.put_u32(ebp - COUNT_FROM_EBP, 0);
        assert_eq!(at_list(&mem, ebp, root), None);
        mem.put_u32(ebp - COUNT_FROM_EBP, 257);
        assert_eq!(at_list(&mem, ebp, root), None);
    }

    #[test]
    fn a_run_can_refresh_its_entries_without_reading_other_ordinals() {
        let mut mem = FakeGuest::new();
        let second = 0x8000;
        let ebp = second - ENTRY_BYTES + LIST_FROM_EBP;
        mem.put_u32(ebp - COUNT_FROM_EBP, 2)
            .put_u32(ebp + USER_FROM_EBP, 1)
            .put_u32(second, 0x1200)
            .put_f32(second + ENTRY_DEPTH, 0.25)
            .put_u16(second + ENTRY_BIAS, 4);
        let frame = ListFrame::read(&mem, ebp).unwrap();
        assert_eq!(frame.entry(&mem, 0), None);
        assert_eq!(
            frame.entry(&mem, 1),
            Some(ListEntry { widget: 0x1200, depth: 0.25, bias: 4 }),
        );
        assert_eq!(frame.entry(&mem, 2), None);
    }

    /// Runs are the drawn entries that list, consecutive among the drawn:
    /// an entry outside the window does not end one, a refused one does.
    #[test]
    fn a_refused_widget_splits_the_runs_and_an_undrawn_one_does_not() {
        let mem = widgets();
        // Entry 1 is past the far depth; entry 2 is text.
        let (claim, refused) = plan(&mem, &list(0x8000, [0.0, 5.0, 0.0, 0.0, 0.0]));
        assert_eq!(refused, [(2, MenuRefusal::from(Gap::text("text_viewport")))]);
        let runs: Vec<_> = claim.runs.iter().map(|r| r.entries.clone()).collect();
        assert_eq!(runs, [vec![0], vec![3, 4]]);
        assert_eq!(claim.run_of, [Some(0), None, None, Some(1), Some(1)]);
        assert_eq!(claim.claimed(), 3);
        assert!(
            claim.runs.iter().all(|r| r.part.is_empty()),
            "containers draw nothing"
        );
        let (none, _) = plan(&mem, &list(0x8000, [5.0, 5.0, 0.0, 5.0, 5.0]));
        assert!(none.runs.is_empty(), "only the text is drawn");
    }

    /// A widget gap panics under the draw and its own name.
    #[test]
    #[should_panic(expected = "menus: Text text_tab_overrun")]
    fn a_widget_gap_panics_under_its_own_name() {
        MenuRefusal::from(Gap::text("text_tab_overrun")).panic();
    }

    #[test]
    fn a_custom_picture_splits_the_run_even_when_its_guest_draw_has_no_pixels() {
        let mut mem = widgets();
        mem.w(0x1200, NONE_VTABLE);
        let (claim, refused) = plan_with_text(&mem, &list(0x8000, [0.0; 5]), &NoText, &|w| w == 0x1200, &|_, _| None);
        assert_eq!(refused, [(2, MenuRefusal::from(Gap::class("draw_custom")))]);
        assert_eq!(claim.run_of, [Some(0), Some(0), None, Some(1), Some(1)]);
    }

    /// A run's items move to its pass at its first draw: the claim keeps
    /// no copy, and the run is still claimed.
    #[test]
    fn a_runs_items_move_out_of_its_claim_once() {
        use bitmaps::tests::{VIEW, WIDGET, bitmap_widget};
        forget();
        let mut mem = Mem::default();
        bitmap_widget(&mut mem);
        let list = UiList {
            frame: 0x8100,
            root: WIDGET,
            user: VIEW,
            window: [-1.0, 1.0],
            entries: vec![ListEntry { widget: WIDGET, depth: 0.0, bias: 0 }],
        };
        let (claim, _) = plan(&mem, &list);
        let planned = claim.runs[0].part.clone();
        open(claim);
        assert_eq!(at_item(0x8100, 0), Item::Place(0));
        let guest = FakeGuest::new();
        assert_eq!(run_part(0x8100, 0, &guest), Some(planned));
        let left = CLAIMS.with(|c| c.borrow().last().map(|c| (c.runs[0].part.items.len(), c.claimed())));
        assert_eq!(left, Some((0, 1)), "moved, and still claimed");
        assert_eq!(run_part(0x8100, 1, &guest), None, "no such run");
        placed(0x8100, 0, true);
        assert!(close(0x8100).is_empty());
        forget();
    }

    /// Each run is placed at its first draw and its entries are left out;
    /// the claim closes clean when every claimed draw was left out.
    #[test]
    fn a_claim_places_each_run_once_and_checks_the_count() {
        forget();
        let (claim, _) = plan(&widgets(), &list(0x8000, [0.0, 5.0, 0.0, 0.0, 0.0]));
        assert!(open(claim).is_empty());
        assert_eq!(at_item(0x8000, 0), Item::Place(0));
        placed(0x8000, 0, true);
        assert_eq!(at_item(0x8000, 2), Item::Game, "the text widget");
        assert_eq!(at_item(0x8000, 3), Item::Place(1));
        placed(0x8000, 1, true);
        assert_eq!(at_item(0x8000, 4), Item::Skip);
        assert_eq!(at_item(0x7000, 4), Item::Game, "another list");
        assert!(close(0x8000).is_empty(), "three claimed, three left out");
        assert_eq!(at_item(0x8000, 0), Item::Game, "closed");
        forget();
    }

    /// A claimed draw that never comes is a mismatch, returned by `close`.
    #[test]
    fn a_missing_draw_is_a_mismatch() {
        forget();
        let (claim, _) = plan(&widgets(), &list(0x8000, [0.0, 5.0, 0.0, 0.0, 0.0]));
        open(claim);
        assert_eq!(at_item(0x8000, 0), Item::Place(0));
        placed(0x8000, 0, true);
        assert_eq!(at_item(0x8000, 3), Item::Place(1));
        placed(0x8000, 1, true);
        // Entry 4 is never drawn.
        let mismatched = close(0x8000);
        assert_eq!(mismatched.len(), 1);
        assert_eq!((mismatched[0].skipped, mismatched[0].claimed()), (2, 3));
        forget();
    }

    /// A bitmap widget lists as the quad its draw makes, in a run of its
    /// own; a scrolling one as its quads, one draw left out.
    #[test]
    fn a_bitmap_widget_is_claimed_with_its_quad() {
        use bitmaps::tests::{DATUM, DEF, VIEW, WIDGET, bitmap_widget};
        let mut mem = Mem::default();
        bitmap_widget(&mut mem);
        let list = UiList {
            frame: 0x8000,
            root: WIDGET,
            user: VIEW,
            window: [-1.0, 1.0],
            entries: vec![ListEntry {
                widget: WIDGET,
                depth: 0.0,
                bias: 0,
            }],
        };
        let (claim, refused) = plan(&mem, &list);
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(claim.claimed(), 1);
        let part = &claim.runs[0].part;
        assert_eq!(part.items.len(), 1);
        assert_eq!(
            part.items[0].material.texture,
            frame_scene::OverlayTexture::Bitmap {
                tag: DATUM,
                image: 1
            }
        );
        mem.w(DEF + 0x14, 0.5f32.to_bits());
        mem.w(WIDGET + 0x7c, 0.25f32.to_bits());
        mem.w(WIDGET + 0x80, 0.5f32.to_bits());
        let (claim, refused) = plan(&mem, &list);
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(claim.claimed(), 1, "one draw, whatever its quads");
        let part = &claim.runs[0].part;
        assert_eq!(part.items.len(), 4);
        assert!(part.items.iter().all(|i| i.material.texture
            == frame_scene::OverlayTexture::Bitmap { tag: DATUM, image: 1 }));
    }

    /// A meter is claimed as its one rectangle, in a run of its own.
    #[test]
    fn a_meter_is_claimed_with_its_rectangle() {
        use bitmaps::tests::{DATUM, VIEW, WIDGET, meter_widget};
        let mut mem = Mem::default();
        meter_widget(&mut mem);
        let list = UiList {
            frame: 0x8000,
            root: WIDGET,
            user: VIEW,
            window: [-1.0, 1.0],
            entries: vec![ListEntry {
                widget: WIDGET,
                depth: 0.0,
                bias: 0,
            }],
        };
        let (claim, refused) = plan(&mem, &list);
        assert!(refused.is_empty(), "{refused:?}");
        assert_eq!(claim.claimed(), 1);
        let part = &claim.runs[0].part;
        assert_eq!(part.items.len(), 1);
        assert_eq!(
            part.items[0].material.texture,
            frame_scene::OverlayTexture::Bitmap {
                tag: DATUM,
                image: bitmaps::METER_IMAGE
            }
        );
    }

    /// A shader bitmap is the game's at the list, drawn from the overlay at
    /// its quad: it ends a run and is not refused.
    #[test]
    fn a_shader_bitmap_ends_a_run_and_is_not_refused() {
        const SHADER_VTABLE: u32 = 0x5300;
        let mut mem = widgets();
        mem.w(SHADER_VTABLE + 0x10, draws::SHADER_BITMAP);
        mem.w(0x1200, SHADER_VTABLE);
        let (claim, refused) = plan(&mem, &list(0x8000, [0.0; 5]));
        assert!(refused.is_empty(), "{refused:?}");
        let runs: Vec<_> = claim.runs.iter().map(|r| r.entries.clone()).collect();
        assert_eq!(runs, [vec![0, 1], vec![3, 4]]);
        assert_eq!(claim.run_of[2], None, "the game draws it");
    }

    /// A list drawn inside another's draw has its own frame; its end
    /// closes it and leaves the outer one open.
    #[test]
    fn nested_lists_close_inside_out() {
        forget();
        let mem = widgets();
        let (outer, _) = plan(&mem, &list(0x8000, [0.0; 5]));
        let (inner, _) = plan(&mem, &list(0x7000, [0.0; 5]));
        open(outer);
        open(inner);
        let mut seen = Vec::new();
        for n in 0..5 {
            let item = at_item(0x7000, n);
            if let Item::Place(run) = item {
                placed(0x7000, run, true);
            }
            seen.push(item);
        }
        use Item::*;
        assert_eq!(seen, [Place(0), Skip, Game, Place(1), Skip]);
        assert!(close(0x7000).is_empty());
        assert_eq!(at_item(0x8000, 0), Place(0), "the outer list stays open");
        assert_eq!(close(0x8000).len(), 1, "its draws never came");
        forget();
    }
}
