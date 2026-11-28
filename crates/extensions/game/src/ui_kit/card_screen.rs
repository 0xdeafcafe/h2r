//! A card carousel as the game draws it (docs/game/ui-screens.md, "Cards
//! and the card row"): the class of every screen whose doc declares
//! `cards:` with `layout: carousel`.
//!
//! The game draws the widgets its tags have, so the carousel is made of
//! them. The focused card's name is the screen's one list row
//! ([`Layout::list`]), so the game's own list carries the focus and the
//! press; its picture is the pregame lobby's map picture, moved into the
//! card's well and pointed at the card's map as the focus moves; its
//! stats, the count and its help are text blocks. Left and right move the
//! focus as up and down do ([`Rows::sideways_focus`]). The frames, the
//! peeking cards and the eased slide are the offline page's alone
//! ([`crate::ui_kit::card::CardsPage`]): no tag has a solid fill.
//!
//! [`screen`] makes the screen from its doc and its feature's cards;
//! [`class`] is its list class. What it shows is published for `ui.stack`
//! ([`crate::ui_kit::card::shown`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::entry::GuestCall;
use guest_layout::memory::{GuestMemory, MemFault};

use crate::list_class::{
    self, Action, ListClass, LiveText, OnFree, Panel, PanelText, PictureSpec, Rows, TextBlock,
};
use crate::mods_screen::page_layout;
use crate::ui::Chosen;
use crate::ui_kit::card::{self, Card, Layout, State};
use crate::ui_kit::data::{self, DataError};
use crate::ui_kit::ids::Family;
use crate::ui_kit::screen::{Filled, KitScreen, Row, Then};
use crate::ui_kit::spec::{ScreenSpec, ValueStyle};

/// The most cards a carousel holds: a row slot each in its text family.
pub const MAX_CARDS: usize = 12;

type Get<T> = Arc<dyn Fn() -> T + Send + Sync>;

/// A carousel's cards and the one held, as its feature gives them now.
#[derive(Clone)]
struct Source {
    cards: Get<Vec<Card>>,
    held: Get<usize>,
}

/// Each carousel's source, by screen name, from [`screen`] to [`class`].
static SOURCES: Mutex<Vec<(&'static str, Source)>> = Mutex::new(Vec::new());

fn source(name: &str) -> Option<Source> {
    SOURCES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| s.clone())
}

/// The carousel `name` over its doc
/// (`content/ui/screens/<name>.yaml`, `cards.layout: carousel`): the
/// title and the legend are the doc's; `cards` gives the cards as the
/// screen shows them (the first [`MAX_CARDS`]), `held` the one focused as
/// it opens, and `pick` is what A on card `i` does.
pub fn screen(
    feature: &'static str,
    name: &'static str,
    ids: Family,
    cards: impl Fn() -> Vec<Card> + Send + Sync + 'static,
    held: impl Fn() -> usize + Send + Sync + 'static,
    pick: impl Fn(usize) -> Then + Send + Sync + 'static,
) -> Result<KitScreen, DataError> {
    let (s, source) = build(feature, name, ids, cards, held, pick)?;
    let mut sources = SOURCES.lock().unwrap_or_else(|e| e.into_inner());
    sources.retain(|(n, _)| *n != name);
    sources.push((name, source));
    Ok(s)
}

/// [`screen`], with its source beside it.
fn build(
    feature: &'static str,
    name: &'static str,
    ids: Family,
    cards: impl Fn() -> Vec<Card> + Send + Sync + 'static,
    held: impl Fn() -> usize + Send + Sync + 'static,
    pick: impl Fn(usize) -> Then + Send + Sync + 'static,
) -> Result<(KitScreen, Source), DataError> {
    let doc = data::require(name)?;
    let file = format!("screens/{name}.yaml");
    if doc.cards.as_ref().map(|c| c.layout.as_str()) != Some("carousel") {
        return Err(DataError::new(
            &file,
            "cards.layout",
            "a card screen in the game is a carousel",
        ));
    }
    let layout = Layout::CAROUSEL;
    let cards: Get<Vec<Card>> = Arc::new(move || {
        let mut all = cards();
        all.truncate(MAX_CARDS);
        all
    });
    let mut s = KitScreen::bare(feature, name, ids, &doc.text("title", &[])?)
        .layout(layout.list(), ValueStyle::Inline)
        .scrolls()
        .description(TextBlock {
            rect: layout.help_rect(),
            font: card::FONT,
            colour: page_layout::PALE_BLUE,
            animation: 0,
        });
    if doc.widget("legend").is_some() {
        s = s.prompts(&doc.prompts("legend")?);
    }
    let pick: Arc<dyn Fn(usize) -> Then + Send + Sync> = Arc::new(pick);
    for i in 0..MAX_CARDS {
        let (fill, press) = (Arc::clone(&cards), Arc::clone(&pick));
        s = s.row(Row::card(
            move || {
                fill().get(i).map(|c| Filled {
                    label: c.name.clone(),
                    shown: String::new(),
                    help: c.help.clone(),
                })
            },
            move || press(i),
        ));
    }
    Ok((
        s,
        Source {
            cards,
            held: Arc::new(held),
        },
    ))
}

/// The well's picture: the pregame lobby's map picture
/// ([`page_layout::PICTURE`]), moved into the centred card's well.
pub fn picture() -> PictureSpec {
    let layout = Layout::CAROUSEL;
    PictureSpec {
        at: layout.well_at(layout.left()),
        ..page_layout::PICTURE
    }
}

/// The panel's texts: the focused card's stats on its last line, and the
/// count beside the help.
pub fn panel() -> Panel {
    let layout = Layout::CAROUSEL;
    let text = |rect: [i16; 4], right: bool| PanelText {
        block: TextBlock {
            rect,
            font: card::FONT,
            colour: page_layout::PALE_BLUE,
            animation: 0,
        },
        right,
        bias: 0,
    };
    Panel {
        art: Vec::new(),
        texts: vec![
            text(layout.stats_rect(layout.left()), false),
            text(layout.count_rect(), true),
        ],
    }
}

/// The carousel's rows: the kit screen's, with the focused card's
/// picture, stats and count, and left and right as the list's up and
/// down.
struct CardRows {
    screen: Arc<KitScreen>,
    source: Source,
    /// The maps' pictures in the loaded map, by stem.
    pictures: Mutex<BTreeMap<String, u32>>,
    /// The focused card, as the description last said.
    focus: AtomicUsize,
}

impl CardRows {
    fn cards(&self) -> Vec<Card> {
        (self.source.cards)()
    }

    /// Text slot `i` past the kit screen's own: 0 the stats, 1 the count.
    fn slot(&self, i: u32) -> u32 {
        self.screen.slots() + i
    }

    /// The kit screen as rows: its builders share names with the rows'
    /// methods (`description`), so every call goes through the trait.
    fn inner(&self) -> &dyn Rows {
        &*self.screen
    }

    /// What `ui.stack` says with card `row` focused.
    fn shown_at(&self, row: usize) -> card::Shown {
        let cards = self.cards();
        let layout = Layout::CAROUSEL;
        let state = State::at(row.min(cards.len().saturating_sub(1)), cards.len(), layout);
        let row = card::Row {
            layout,
            cards,
            open: None,
        };
        card::Shown::of(self.screen.name(), &row, state)
    }

    /// Notes the focus and says what is shown, for `ui.stack`.
    fn focused(&self, row: usize) {
        self.focus.store(row, Ordering::SeqCst);
        card::publish(Some(self.shown_at(row)));
    }
}

impl Rows for CardRows {
    fn title(&self) -> u32 {
        self.inner().title()
    }
    fn count(&self) -> usize {
        self.inner().count()
    }
    fn content_spec(&self) -> Option<ScreenSpec> {
        self.inner().content_spec()
    }
    fn growing_capacity(&self) -> Option<usize> {
        self.inner().growing_capacity()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        self.inner().texts(row)
    }
    fn shrink_growing_rows(&self) -> bool {
        self.inner().shrink_growing_rows()
    }
    /// A carousel stops at its ends, as the offline row does.
    fn wraps(&self) -> bool {
        false
    }
    fn press(&self, row: i16) -> Chosen {
        self.inner().press(row)
    }
    fn description(&self, row: i16) -> Option<u32> {
        if let Ok(i) = usize::try_from(row) {
            self.focused(i);
        }
        self.inner().description(row)
    }
    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        self.inner().focused_footer(row)
    }
    fn tint(&self, row: i16) -> Option<[f32; 3]> {
        self.inner().tint(row)
    }
    /// The card's map's picture; a card with none, or a map the loaded
    /// map has no picture of, leaves the well clear.
    fn picture(&self, row: i16) -> Option<u32> {
        let cards = self.cards();
        let stem = cards.get(usize::try_from(row).ok()?)?.level.as_deref()?;
        self.pictures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&stem.to_ascii_lowercase())
            .copied()
    }
    fn prepare_picture(&self, m: &dyn GuestMemory, template: u32) -> Option<u32> {
        *self.pictures.lock().unwrap_or_else(|e| e.into_inner()) =
            crate::ui_kit::level_art::loaded(m, template);
        // The lobby's block names unknown_map: a null reference keeps the
        // well clear until a focused card has a picture.
        Some(u32::MAX)
    }
    fn restore_missing_picture(&self) -> bool {
        true
    }
    fn live_text(&self) -> LiveText {
        self.inner().live_text()
    }
    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        self.inner().sideways(row, right)
    }
    fn sideways_focus(&self, row: i16, right: bool) -> Option<usize> {
        let at = usize::try_from(row).ok()?;
        let to = if right { at + 1 } else { at.checked_sub(1)? };
        (to < self.count()).then_some(to)
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.inner().write_texts(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        self.inner().describe(count)
    }
    fn line(&self, row: i16) -> Option<String> {
        self.inner().line(row)
    }
    fn hide_empty_pane_texts(&self) -> bool {
        self.inner().hide_empty_pane_texts()
    }
    fn panel_text(&self, i: usize) -> Option<u32> {
        let cards = self.cards();
        let focus = self
            .focus
            .load(Ordering::SeqCst)
            .min(cards.len().checked_sub(1)?);
        let text = match i {
            0 => cards[focus].stats.clone(),
            1 => card::count_text(focus, cards.len()),
            _ => return None,
        };
        Some(self.screen.set_text(self.slot(i as u32), &text))
    }
}

/// The list class of carousel `screen` ([`screen`] made it): the kit
/// screen's class over [`CardRows`], with the well's picture and the
/// panel's texts, the held card focused once the pane is built, and
/// `ui.stack` told when it is freed.
pub fn class(screen: &Arc<KitScreen>) -> ListClass {
    let name = screen.name();
    let source = source(name)
        .unwrap_or_else(|| panic!("{name}: not a card screen (card_screen::screen makes one)"));
    assert!(
        screen.slots() + 2 <= screen.family().len,
        "{name}: {} text slots and two of the card's in a family of {}",
        screen.slots(),
        screen.family().len
    );
    let rows = Arc::new(CardRows {
        screen: Arc::clone(screen),
        source,
        pictures: Mutex::new(BTreeMap::new()),
        focus: AtomicUsize::new(0),
    });
    let me = Arc::clone(&rows);
    let ready: Action = Arc::new(move |cx, at| {
        let held = (me.source.held)().min(me.count().saturating_sub(1));
        me.focused(held);
        Box::pin(async move {
            let list = at.wrapping_add(guest_layout::view::ui::LIST);
            cx.call(
                GuestCall::new(list_class::LIST_SET_ROW)
                    .eax(list)
                    .args([held as u32]),
            )
            .await?;
            Ok(())
        })
    });
    let inner = screen.on_free();
    let freed: OnFree = Arc::new(move |cx, at| {
        card::publish(None);
        inner(cx, at)
    });
    screen
        .class()
        .rows(rows as Arc<dyn Rows>)
        .picture(picture())
        .panel(panel())
        .pane_ready(ready)
        .on_free(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::layout::SAFE;

    fn cards(n: usize) -> Vec<Card> {
        (0..n)
            .map(|i| Card {
                name: format!("Card {i}"),
                stats: format!("{i} rounds"),
                level: (i % 2 == 0).then(|| "lockout".to_owned()),
                help: Some(format!("What card {i} is.")),
                open: None,
            })
            .collect()
    }

    fn rows(n: usize) -> (Arc<CardRows>, Arc<Mutex<Vec<usize>>>) {
        let picked = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&picked);
        let (s, source) = build(
            "kit-cards-test",
            "siege.variants",
            crate::ui_kit::ids::family("firefight wave sets"),
            move || cards(n),
            || 1,
            move |i| {
                log.lock().unwrap().push(i);
                Then::Close
            },
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let rows = Arc::new(CardRows {
            source,
            screen: Arc::new(s),
            pictures: Mutex::new(BTreeMap::from([("lockout".to_owned(), 0x1234)])),
            focus: AtomicUsize::new(0),
        });
        (rows, picked)
    }

    /// A row a card and no more: an empty slot is not drawn. A picks the
    /// card and leaves the close to the screen's action.
    #[test]
    fn a_carousel_has_a_row_a_card_and_a_picks_it() {
        let (r, picked) = rows(3);
        assert_eq!(r.count(), 3);
        assert_eq!(r.press(2), Chosen::Act);
        assert_eq!(*picked.lock().unwrap(), [2]);
        assert_eq!(r.press(3), Chosen::Nothing, "no card there");
        let (many, _) = rows(MAX_CARDS + 5);
        assert_eq!(many.count(), MAX_CARDS);
    }

    /// Left and right are the list's up and down, and stop at the ends.
    #[test]
    fn left_and_right_move_the_focus_and_stop_at_the_ends() {
        let (r, _) = rows(3);
        assert_eq!(r.sideways_focus(0, true), Some(1));
        assert_eq!(r.sideways_focus(1, false), Some(0));
        assert_eq!(r.sideways_focus(0, false), None);
        assert_eq!(r.sideways_focus(2, true), None);
        assert!(!r.wraps());
    }

    /// The focused card gives the picture, the stats and the count, and
    /// `ui.stack` hears of it.
    #[test]
    fn the_focused_card_gives_the_picture_the_stats_and_the_count() {
        let (r, _) = rows(3);
        assert_eq!(r.picture(0), Some(0x1234));
        assert_eq!(r.picture(1), None, "a card with no map");
        assert!(r.restore_missing_picture());
        assert!(r.description(2).is_some());
        assert_eq!(r.focus.load(Ordering::SeqCst), 2);
        let shown = r.shown_at(2);
        assert_eq!(
            (shown.screen.as_str(), shown.focus, shown.cards),
            ("siege.variants", 2, 3)
        );
        assert_eq!(shown.name, "Card 2");
        let (stats, count) = (r.panel_text(0), r.panel_text(1));
        assert!(stats.is_some() && count.is_some() && stats != count);
        assert_eq!(r.panel_text(2), None);
    }

    /// The picture is in the centred card's well, and every text block
    /// inside the title-safe area.
    #[test]
    fn the_wells_picture_and_the_texts_are_inside_the_safe_area() {
        let layout = Layout::CAROUSEL;
        let p = picture();
        assert_eq!(p.at, layout.well_at(layout.left()));
        assert_eq!(page_layout::PICTURE_SIZE, card::PICTURE);
        let [x, y] = p.at;
        assert!(x >= SAFE.left && x + card::PICTURE[0] as i16 <= SAFE.right);
        assert!(y <= SAFE.top && y - card::PICTURE[1] as i16 >= SAFE.bottom);
        for t in panel().texts {
            let [top, left, bottom, right] = t.block.rect;
            assert!(top > bottom && right > left, "{:?}", t.block.rect);
            assert!(
                left >= SAFE.left && right <= SAFE.right,
                "{:?}",
                t.block.rect
            );
            assert!(
                top <= SAFE.top && bottom >= SAFE.bottom,
                "{:?}",
                t.block.rect
            );
        }
    }
}
