//! The Skulls page between the campaign difficulty and the level, on our
//! own list class, and the flow that puts it there (docs/game/skulls.md
//! "Menu").
//!
//! **The flow.** Level select (or New Campaign) → difficulty → the Skulls
//! page → the level. The difficulty list's row chosen (`0x2c9d38`, thiscall
//! on the list, the player chain and a pointer to the row, `ret 8`) stores
//! the difficulty and starts: from New Campaign (`list + 0x2a0` clear) it
//! opens the loading screen (`0x2b1467`, screen 209); from Select Level it
//! animates the difficulty screen out (`0x22eeee`, `0x22e957`) and the
//! level select goes on. A `before` hook on it, for rows 0 to 3, asks the
//! [`PageFlow`]: when the pressing player's profile has collected a skull,
//! it keeps the press ([`Pending`]) and opens the page instead; with none
//! collected the original runs, as retail. On the page, the Start row (or
//! the Start button) closes the page (`0x14800c`, as B does) and calls the
//! original with the kept list and row ([`start`]); the hook lets that call
//! through. B closes the page as on any screen of ours: back on the
//! difficulty list, where a press opens the page again.
//!
//! **The page** is a [`SkullsScreen`] (rows, strings and descriptions in
//! [`skulls_menu`]) on a class with the level select's layout
//! ([`skulls_menu::LIST_LAYOUT`]), the picture and the description.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::layout_glue::chosen_row;
use crate::list_class::{self, Action, ListClass, ListLayout, PictureSpec, TextBlock};
use crate::skulls;
use crate::skulls_menu::{self, SkullsScreen};
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::MemFault;
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

/// The screen id `Open` rows and the loader registry know it by.
pub const SCREEN: &str = "skulls";
/// The difficulty list handler's vtable (`0x2c9c12` stores it): the kept
/// list is checked against it before its press is replayed.
pub const DIFFICULTY_LIST_VTABLE: u32 = 0x0045_d688;
/// Close the top screen of a window's stack (`eax` the slot, `ecx` the
/// window, as the generic back handler `0x2300ea` calls it with the
/// screen's `+0x78` and `+0x74`). Retail's close starts the screen's out
/// transition; the screen is freed when it ends.
pub const SCREEN_CLOSE: u32 = 0x0014_800c;
const SCREEN_WINDOW: u32 = 0x74;
const SCREEN_SLOT: u32 = 0x78;
/// The difficulty rows: Easy, Normal, Heroic, Legendary.
pub const DIFFICULTIES: std::ops::RangeInclusive<i16> = 0..=3;

/// The page's class: the level select's layout, the picture and the
/// description, and Start as its action.
pub fn class(
    screen: Arc<SkullsScreen>,
    block: TextBlock,
    layout: ListLayout,
    picture: PictureSpec,
    flow: Arc<PageFlow>,
) -> ListClass {
    let action: Action = Arc::new(move |cx, page| {
        let flow = Arc::clone(&flow);
        Box::pin(async move { start(&cx, &flow, page).await })
    });
    ListClass::with_rows(skulls_menu::FEATURE, SCREEN, screen)
        .description(block)
        .picture(picture)
        .layout(layout)
        .action(action)
}

/// A difficulty press kept while the page is up: what to replay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The difficulty list handler (the press's `this`).
    pub list: u32,
    pub row: i16,
    pub player: u32,
    pub profile: String,
}

/// What a press on the difficulty list knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    pub list: u32,
    pub row: Option<i16>,
    pub player: Option<u32>,
    pub profile: Option<String>,
    /// How many skulls `profile` has collected.
    pub collected: usize,
}

/// What the hook does with a difficulty press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Run the original (start as retail); the line to log, if any.
    Retail(Option<String>),
    /// Open the page for this press.
    Page(Pending),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Pending(Pending),
    /// The page's Start is replaying the press: the next difficulty press
    /// is that call, and runs as retail.
    Starting,
}

/// The page's state: whether a difficulty press is kept, and the scratch
/// page the replay's arguments are written to.
pub struct PageFlow {
    state: Mutex<State>,
    on: AtomicBool,
    scratch: OnceLock<u32>,
}

impl Default for PageFlow {
    fn default() -> Self {
        PageFlow {
            state: Mutex::new(State::Idle),
            on: AtomicBool::new(true),
            scratch: OnceLock::new(),
        }
    }
}

impl PageFlow {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Whether the page is shown at all (its class is on the plan).
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::SeqCst);
    }
    /// A press on the difficulty list.
    pub fn difficulty(&self, p: Press) -> Decision {
        let mut st = self.lock();
        if *st == State::Starting {
            *st = State::Idle;
            return Decision::Retail(None);
        }
        let Some(row) = p.row.filter(|r| DIFFICULTIES.contains(r)) else {
            return Decision::Retail(None);
        };
        if !self.on.load(Ordering::SeqCst) {
            return Decision::Retail(None);
        }
        let (Some(player), Some(profile)) = (p.player, p.profile) else {
            return Decision::Retail(Some(
                "skulls page: the pressing player is unreadable; starting without the page".into(),
            ));
        };
        if p.collected == 0 {
            return Decision::Retail(Some(format!(
                "skulls page: {profile} has no skulls collected; starting as retail"
            )));
        }
        let pending = Pending {
            list: p.list,
            row,
            player,
            profile,
        };
        *st = State::Pending(pending.clone());
        Decision::Page(pending)
    }
    /// The page's Start: the press to replay, which the next difficulty
    /// press is. `None` with nothing kept.
    pub fn start(&self) -> Option<Pending> {
        let mut st = self.lock();
        match std::mem::replace(&mut *st, State::Idle) {
            State::Pending(p) => {
                *st = State::Starting;
                Some(p)
            }
            _ => None,
        }
    }
    /// The replay returned (or could not be made): nothing is kept.
    pub fn done(&self) {
        *self.lock() = State::Idle;
    }
    /// The press kept, if any.
    pub fn pending(&self) -> Option<Pending> {
        match &*self.lock() {
            State::Pending(p) => Some(p.clone()),
            _ => None,
        }
    }
}

/// The replay's arguments at `at`: a player chain (`[at]` the record at
/// `at + 8`, whose `+4` is the player index, as `[[chain]]+4` is read) and
/// the row at `at + 0x10`. Returns (chain, row pointer).
pub fn write_press(
    m: &mut dyn engine::memory::GuestMemory,
    at: u32,
    p: &Pending,
) -> Result<(u32, u32), MemFault> {
    m.set_u32(at, at + 8)?;
    m.set_u32(at + 8, 0)?;
    m.set_u32(at + 0xc, p.player)?;
    m.set_u32(at + 0x10, u32::from(p.row as u16))?;
    Ok((at, at + 0x10))
}

/// Whether to close the page before the replay (`H2_SKULLS_PAGE_CLOSE=0`
/// leaves it up, should closing first misbehave in game).
fn close_first() -> bool {
    std::env::var("H2_SKULLS_PAGE_CLOSE").map_or(true, |v| v != "0")
}

/// The page's Start, on the page `page`: close it, then run the kept
/// difficulty press as the game would have.
pub async fn start(cx: &Cx, flow: &PageFlow, page: u32) -> Result<(), EntryError> {
    let Some(p) = flow.start() else {
        eprintln!("skulls page: Start with no difficulty kept; nothing to start");
        return Ok(());
    };
    let Some(&scratch) = flow.scratch.get() else {
        eprintln!("skulls page: no scratch page; not started");
        flow.done();
        return Ok(());
    };
    let vtable = cx.read(|m| m.u32(p.list)).unwrap_or(0);
    if vtable != DIFFICULTY_LIST_VTABLE {
        eprintln!(
            "skulls page: the difficulty list at {:#x} is gone (vtable {vtable:#x}); not started",
            p.list
        );
        flow.done();
        return Ok(());
    }
    eprintln!(
        "skulls page: starting on difficulty {} for {} with {}",
        p.row,
        p.profile,
        skulls_on_line(&p.profile)
    );
    if close_first() {
        let (window, slot) = cx.read(|m| -> Result<_, MemFault> {
            Ok((m.u32(page + SCREEN_WINDOW)?, m.u32(page + SCREEN_SLOT)?))
        })?;
        cx.call(GuestCall::new(SCREEN_CLOSE).eax(slot).ecx(window))
            .await?;
    }
    let (chain, row) = cx.write(|m| write_press(m, scratch, &p))?;
    let r = cx
        .call(
            GuestCall::new(engine::points::DIFFICULTY_ROW_CHOSEN.base)
                .ecx(p.list)
                .args([chain, row]),
        )
        .await;
    flow.done();
    r.map(|_| ())
}

/// The store, for the start's log line.
static SKULLS_STORE: OnceLock<Arc<settings::Store>> = OnceLock::new();

/// The skulls the start will turn on for `profile`, for the log (the
/// campaign start's own line says what it applied).
fn skulls_on_line(profile: &str) -> String {
    SKULLS_STORE.get().map_or_else(
        || "its skulls".into(),
        |store| {
            let on: Vec<&str> = (0..skulls::TOTAL)
                .filter(|i| skulls::state(store, profile, *i) == Some(skulls::State::On))
                .map(|i| skulls::SKULLS[i].label)
                .collect();
            if on.is_empty() {
                "no skulls on".to_string()
            } else {
                on.join(", ")
            }
        },
    )
}

/// The page between the difficulty and the level: the hook on the
/// difficulty list's press.
pub struct DifficultyPage {
    pub screen: Arc<SkullsScreen>,
    pub flow: Arc<PageFlow>,
    pub store: Arc<settings::Store>,
}

fn row_chosen(c: &mut Call, page: &DifficultyPage, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let (row, player, profile) = {
        let mem = c.ctx.runtime().memory();
        let player = skulls_menu::player_index(&*mem, c.cpu.esp);
        (
            chosen_row(&*mem, c.cpu.esp),
            player,
            player.and_then(|p| skulls_menu::profile_of(&*mem, p)),
        )
    };
    // The store is read with guest memory released.
    let collected = profile
        .as_deref()
        .map_or(0, |p| skulls::collected(&page.store, p).len());
    let press = Press {
        list: c.cpu.ecx,
        row,
        player,
        profile,
        collected,
    };
    let pending = match page.flow.difficulty(press) {
        Decision::Retail(line) => {
            if let Some(line) = line {
                eprintln!("{line}");
            }
            return Ok(Flow::Continue);
        }
        Decision::Page(p) => p,
    };
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("skulls page: difficulty chosen; the Skulls page comes first");
    }
    let Some(loader) = list_class::loader_of(SCREEN) else {
        eprintln!("skulls page: its list class is off; starting as retail");
        page.flow.done();
        return Ok(Flow::Continue);
    };
    eprintln!(
        "skulls page: opening the Skulls page for {} (player {}, difficulty {})",
        pending.profile, pending.player, pending.row
    );
    page.screen.open(pending.profile.clone());
    let mask = 1u32 << pending.player;
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.call(GuestCall::new(engine::points::OPEN_SCREEN.base).args([mask, loader]))
                .await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl feature::Feature for DifficultyPage {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "skulls-page",
            "The Skulls page between the difficulty and the level",
        )
        .doc("docs/game/skulls.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let scratch = b.guest_alloc(0x1000, Permissions::RW)?;
        self.flow.scratch.set(scratch).ok();
        SKULLS_STORE.set(Arc::clone(&self.store)).ok();
        let seen = Arc::new(AtomicBool::new(false));
        let page = DifficultyPage {
            screen: Arc::clone(&self.screen),
            flow: Arc::clone(&self.flow),
            store: Arc::clone(&self.store),
        };
        b.raw().before(
            &engine::points::DIFFICULTY_ROW_CHOSEN,
            "the Skulls page before the level",
            move |c| row_chosen(c, &page, &seen),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::Rows;
    use engine::entry::{GuestCall as Gc, Regs, testing};
    use engine::memory::GuestMemory;
    use engine::testing::FakeGuest;
    use settings::Store;

    fn screen() -> Arc<SkullsScreen> {
        Arc::new(SkullsScreen::new(Arc::new(Store::defaults(
            crate::setting_defs::ALL,
        ))))
    }

    fn press(row: i16, collected: usize) -> Press {
        Press {
            list: 0x5000,
            row: Some(row),
            player: Some(1),
            profile: Some("Chief".into()),
            collected,
        }
    }

    #[test]
    fn a_profile_with_no_skulls_goes_straight_to_the_level() {
        let f = PageFlow::default();
        match f.difficulty(press(3, 0)) {
            Decision::Retail(Some(line)) => {
                assert!(line.contains("no skulls collected"), "{line}")
            }
            d => panic!("{d:?}"),
        }
        assert_eq!(f.pending(), None);
        assert_eq!(f.start(), None, "no page, nothing to start");
    }

    #[test]
    fn a_profile_with_a_skull_gets_the_page_then_starts_on_its_difficulty() {
        let f = PageFlow::default();
        let want = Pending {
            list: 0x5000,
            row: 2,
            player: 1,
            profile: "Chief".into(),
        };
        assert_eq!(f.difficulty(press(2, 1)), Decision::Page(want.clone()));
        assert_eq!(f.pending(), Some(want.clone()));
        // Start: the kept press is replayed, and that replay runs as retail.
        assert_eq!(f.start(), Some(want));
        assert_eq!(f.difficulty(press(2, 1)), Decision::Retail(None));
        f.done();
        assert_eq!(f.pending(), None);
        assert_eq!(f.start(), None, "started once");
    }

    /// B closes the page (the generic back handler); on the difficulty list
    /// again, another difficulty opens the page with that one.
    #[test]
    fn back_then_another_difficulty_keeps_the_new_one() {
        let f = PageFlow::default();
        assert!(matches!(f.difficulty(press(3, 2)), Decision::Page(_)));
        assert!(matches!(f.difficulty(press(0, 2)), Decision::Page(p) if p.row == 0));
        assert_eq!(f.start().map(|p| p.row), Some(0));
    }

    #[test]
    fn only_the_four_difficulties_and_a_known_player_open_the_page() {
        let f = PageFlow::default();
        assert_eq!(f.difficulty(press(4, 3)), Decision::Retail(None));
        assert_eq!(f.difficulty(press(-1, 3)), Decision::Retail(None));
        let mut p = press(1, 3);
        p.row = None;
        assert_eq!(f.difficulty(p), Decision::Retail(None));
        let mut p = press(1, 3);
        p.player = None;
        assert!(matches!(f.difficulty(p), Decision::Retail(Some(_))));
        f.set_on(false);
        assert_eq!(
            f.difficulty(press(1, 3)),
            Decision::Retail(None),
            "class off"
        );
        assert_eq!(f.pending(), None);
    }

    #[test]
    fn the_replay_arguments_read_as_the_game_reads_them() {
        let mut g = FakeGuest::new();
        g.map(0x9000, 0x100);
        let p = Pending {
            list: 0x5000,
            row: 3,
            player: 2,
            profile: "Chief".into(),
        };
        let (chain, row) = write_press(&mut g, 0x9000, &p).unwrap();
        // `[[chain]]+4` is the player; `[row] & 0xffff` the row.
        let record = g.u32(chain).unwrap();
        assert_eq!(g.u32(record + 4), Ok(2));
        assert_eq!(g.u32(row).map(|r| r & 0xffff), Ok(3));
    }

    const PAGE: u32 = 0x0010_0000;
    const LIST: u32 = 0x0020_0000;
    const SCRATCH: u32 = 0x0030_0000;

    fn run_start(f: &Arc<PageFlow>, g: FakeGuest) -> testing::Run {
        let flow = Arc::clone(f);
        let run_f = move |cx: Cx| -> EntryFuture {
            let flow = Arc::clone(&flow);
            Box::pin(async move {
                start(&cx, &flow, PAGE).await?;
                Ok(Ret::new(0, 0))
            })
        };
        testing::run(&run_f, g, Regs::default(), |_, _| Regs::default())
    }

    /// Start on the page: close the page on its own window and slot, then
    /// replay the difficulty press on the kept list with our arguments.
    #[test]
    fn start_closes_the_page_and_replays_the_press() {
        let f = Arc::new(PageFlow::default());
        f.scratch.set(SCRATCH).unwrap();
        assert!(matches!(
            f.difficulty(Press {
                list: LIST,
                ..press(1, 1)
            }),
            Decision::Page(_)
        ));
        let mut g = FakeGuest::new();
        g.map(PAGE, 0x100).map(LIST, 0x10).map(SCRATCH, 0x100);
        g.put_u32(PAGE + SCREEN_WINDOW, 5)
            .put_u32(PAGE + SCREEN_SLOT, 4)
            .put_u32(LIST, DIFFICULTY_LIST_VTABLE);
        let run = run_start(&f, g);
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        assert_eq!(
            run.calls,
            [
                Gc::new(SCREEN_CLOSE).eax(4).ecx(5),
                Gc::new(engine::points::DIFFICULTY_ROW_CHOSEN.base)
                    .ecx(LIST)
                    .args([SCRATCH, SCRATCH + 0x10]),
            ]
        );
        assert_eq!(f.pending(), None);
        assert_eq!(f.start(), None);
    }

    /// A kept list that is no longer a difficulty list is not called.
    #[test]
    fn start_refuses_a_list_that_is_gone() {
        let f = Arc::new(PageFlow::default());
        f.scratch.set(SCRATCH).unwrap();
        f.difficulty(Press {
            list: LIST,
            ..press(1, 1)
        });
        let mut g = FakeGuest::new();
        g.map(PAGE, 0x100).map(LIST, 0x10).map(SCRATCH, 0x100);
        let run = run_start(&f, g);
        assert_eq!(run.calls, []);
        assert_eq!(f.start(), None, "nothing kept after the refusal");
    }

    #[test]
    fn the_start_row_comes_first_and_acts() {
        let r = screen();
        assert_eq!(r.count(), skulls::TOTAL + 1);
        assert_eq!(r.texts(0), [(crate::ui::LABEL_SLOT, skulls_menu::START_ID)]);
        assert_eq!(r.press(0), crate::ui::Chosen::Act);
        assert!(matches!(r.press(1), crate::ui::Chosen::Stepped(m) if m.contains("no profile")));
        assert_eq!(r.description(0), Some(skulls_menu::START_DESCRIPTION_ID));
    }

    #[test]
    fn the_hook_and_the_class_are_on_the_plan() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let s = screen();
        let flow = Arc::new(PageFlow::default());
        let c = class(
            Arc::clone(&s),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
            skulls_menu::PICTURE,
            Arc::clone(&flow),
        );
        r.add(&c).unwrap();
        r.add(&DifficultyPage {
            screen: s,
            flow,
            store: Arc::new(Store::defaults(crate::setting_defs::ALL)),
        })
        .unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        assert!(
            text.contains(
                "0x002c9d38 DIFFICULTY_ROW_CHOSEN: before [skulls-page:the Skulls page before the level] -> original\n"
            ),
            "{text}"
        );
        assert!(
            !text.contains("DIFFICULTY_ROW_TEXT"),
            "no row on the difficulty list"
        );
        assert!(text.contains("entry skulls-screen:loader"));
        assert!(text.contains("entry skulls-screen:event"));
    }
}
