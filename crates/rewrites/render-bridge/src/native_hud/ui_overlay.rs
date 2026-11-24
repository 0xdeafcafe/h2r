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
//! claimed again until the next map ([`close`]).

use std::cell::RefCell;
use std::collections::HashSet;

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
    /// The widget's draw is not read yet.
    Widget(Unread),
    /// The overlay refused the widget's items.
    Unlisted,
    /// The host did not place the run's pass.
    Pass,
}

impl MenuRefusal {
    pub fn counter(self) -> Ui {
        match self {
            MenuRefusal::Frame => Ui::OverlayMenuRefusedFrame,
            MenuRefusal::Mismatched => Ui::OverlayMenuRefusedMismatched,
            MenuRefusal::Widget(Unread::Bitmap) => Ui::OverlayMenuRefusedBitmap,
            MenuRefusal::Widget(Unread::Shader) => Ui::OverlayMenuRefusedShader,
            MenuRefusal::Widget(Unread::Text) => Ui::OverlayMenuRefusedText,
            MenuRefusal::Widget(Unread::Model) => Ui::OverlayMenuRefusedModel,
            MenuRefusal::Widget(Unread::Draw) => Ui::OverlayMenuRefusedDraw,
            MenuRefusal::Widget(Unread::Unreadable) => Ui::OverlayMenuRefusedUnreadable,
            MenuRefusal::Unlisted => Ui::OverlayMenuRefusedUnlisted,
            MenuRefusal::Pass => Ui::OverlayMenuRefusedPass,
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
    /// Screens whose list did not match its claim: the game's until
    /// [`forget`].
    static REFUSED: RefCell<HashSet<u32>> = RefCell::new(HashSet::new());
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The sorted list at [`LIST_READY`] (EBP, EDI): `None` when its frame
/// does not read.
pub fn at_list(mem: &dyn GuestMemory, ebp: u32, edi: u32) -> Option<UiList> {
    let below = |n: u32| mem.u32(ebp.checked_sub(n)?).ok();
    let count = below(COUNT_FROM_EBP)? as i32;
    if count <= 0 || count as usize > walk::MAX_LISTED {
        return None;
    }
    let first = ebp.checked_sub(LIST_FROM_EBP)?;
    let entries = (0..count as u32)
        .map(|i| {
            let at = first.checked_add(i * ENTRY_BYTES)?;
            Some(ListEntry {
                widget: mem.u32(at).ok()?,
                depth: f32::from_bits(mem.u32(at.checked_add(ENTRY_DEPTH)?).ok()?),
                bias: mem.u16(at.checked_add(ENTRY_BIAS)?).ok()? as i16,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(UiList {
        frame: ebp,
        root: edi,
        user: mem.u32(ebp.checked_add(USER_FROM_EBP)?).ok()?,
        window: [
            f32::from_bits(below(NEAR_FROM_EBP)?),
            f32::from_bits(below(FAR_FROM_EBP)?),
        ],
        entries,
    })
}

/// The screen quads' viewport, when it reads.
pub fn screen_rect(mem: &dyn GuestMemory) -> Option<[i16; 4]> {
    let word = |n: u32| mem.u16(SCREEN_RECT + 2 * n).ok().map(|w| w as i16);
    Some([word(0)?, word(1)?, word(2)?, word(3)?])
}

/// One drawn entry's items: its widget read and listed alone, or why the
/// game draws it.
fn part(mem: &dyn GuestRead, e: &ListEntry) -> Result<Overlay, MenuRefusal> {
    let mut widget = walk::read_widget(mem, e.widget, e.depth).map_err(MenuRefusal::Widget)?;
    // The bias the guest sorted by.
    widget.bias = e.bias;
    let frame = UiFrame {
        draws: vec![UiDraw::Screen(UiScreen {
            backdrop: None,
            widgets: vec![widget],
        })],
    };
    let mut part = Overlay::new(walk::CANVAS);
    if walk::draw_list(&frame, &NoText, &mut part)
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
    let mut runs: Vec<Run> = Vec::new();
    let mut refused = Vec::new();
    let mut open = false;
    for (i, e) in list.entries.iter().enumerate() {
        if !walk::in_window(e.depth, list.window) {
            continue;
        }
        let items = match part(mem, e) {
            Ok(items) => items,
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

/// Whether screen `root`'s lists are the game's until [`forget`].
pub fn refused(root: u32) -> bool {
    REFUSED.with(|r| r.borrow().contains(&root))
}

/// The draw of entry `ordinal` of the list whose frame is `frame`.
pub fn at_item(frame: u32, ordinal: u32) -> Item {
    CLAIMS.with(|c| {
        let mut claims = c.borrow_mut();
        let Some(claim) = claims.iter_mut().rev().find(|c| c.frame == frame) else {
            return Item::Game;
        };
        let Some(Some(run)) = claim.run_of.get(ordinal as usize).copied() else {
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
            None => Item::Game,
        }
    })
}

/// The items of run `run` of the claim at `frame`, to place.
pub fn run_part(frame: u32, run: usize) -> Option<Overlay> {
    CLAIMS.with(|c| {
        let claims = c.borrow();
        let claim = claims.iter().rev().find(|c| c.frame == frame)?;
        Some(claim.runs.get(run)?.part.clone())
    })
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
            if placed {
                claim.skipped += 1;
            }
        }
    });
}

/// Closes the claim at `frame` and every one opened after it: those whose
/// skipped draws are not their claimed ones, counted, their screens
/// refused until [`forget`].
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
    for claim in &mismatched {
        Ui::OverlayMenuMismatch.count();
        REFUSED.with(|r| r.borrow_mut().insert(claim.root));
    }
    mismatched
}

/// Forgets the open claims and the refused screens (a snapshot load, a
/// new map).
pub fn forget() {
    CLAIMS.with(|c| c.borrow_mut().clear());
    REFUSED.with(|r| r.borrow_mut().clear());
}

/// The list at [`LIST_READY`]: its claim, or why the game draws all of
/// it. Each drawn widget the game keeps is counted by its reason.
pub fn claim_list(mem: &dyn GuestMemory, list: &UiList) -> Result<Claim, MenuRefusal> {
    if screen_rect(mem) != Some(FULL_SCREEN) {
        return Err(MenuRefusal::Frame);
    }
    if refused(list.root) {
        return Err(MenuRefusal::Mismatched);
    }
    let (claim, refused) = plan(&Ro(mem), list);
    for (_, why) in refused {
        why.counter().count();
    }
    Ok(claim)
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

    /// Runs are the drawn entries that list, consecutive among the drawn:
    /// an entry outside the window does not end one, a refused one does.
    #[test]
    fn a_refused_widget_splits_the_runs_and_an_undrawn_one_does_not() {
        let mem = widgets();
        // Entry 1 is past the far depth; entry 2 is text.
        let (claim, refused) = plan(&mem, &list(0x8000, [0.0, 5.0, 0.0, 0.0, 0.0]));
        assert_eq!(refused, [(2, MenuRefusal::Widget(Unread::Text))]);
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

    /// A run whose pass is not placed is the game's; a claimed draw that
    /// never comes is a mismatch, and the screen is refused until the
    /// claims are forgotten.
    #[test]
    fn an_unplaced_run_is_the_games_and_a_missing_draw_refuses_the_screen() {
        forget();
        let (claim, _) = plan(&widgets(), &list(0x8000, [0.0, 5.0, 0.0, 0.0, 0.0]));
        open(claim);
        assert_eq!(at_item(0x8000, 0), Item::Place(0));
        placed(0x8000, 0, false);
        assert_eq!(at_item(0x8000, 3), Item::Place(1));
        placed(0x8000, 1, true);
        // Entry 4 is never drawn.
        let mismatched = close(0x8000);
        assert_eq!(mismatched.len(), 1);
        assert_eq!((mismatched[0].skipped, mismatched[0].claimed()), (1, 2));
        assert!(refused(0x1000));
        forget();
        assert!(!refused(0x1000));
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
