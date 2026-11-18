//! Rows of ours on a retail list, as data (docs/engine/feature-engine.md "UI step
//! c"): the `ui` model's extension rows of that list, answered by `before`
//! hooks on the plan at the list's row text and row chosen. A retail row
//! goes on to the original; a row of ours never reaches it.
//!
//! The row count (the shared datum array constructor) and the pane hook are
//! still `layout_glue`'s raw hooks: they serve every list at once.
//!
//! **Our rows among the retail ones** ([`crate::ui::Extension::at`]): a
//! list that tells its rows apart by index (the campaign pause menu) shows
//! a retail row past ours at a higher index than the original knows. Its
//! text and press go to the original with the handle of the retail row it
//! shows ([`moved`]), the item's row word (`+0x70`) or the press's row
//! word swapped for the call and the item's put back after. Our hooks let
//! that call through.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::points::Point;
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;

use crate::layout_glue::{Gm, chosen_row};
use crate::list_class;
use crate::ui::{Chosen, List, Model};

/// One retail list's rows of ours.
pub struct Extension {
    /// The feature id.
    pub id: &'static str,
    pub title: &'static str,
    pub list: List,
    /// Its row text and row chosen (thiscall, two arguments, `ret 8`).
    pub text: &'static Point,
    pub chosen: &'static Point,
    /// Draw the label only (a list whose items have one text).
    pub label_only: bool,
    pub model: Arc<Model>,
}

/// The texts row `row` of `list` draws: `None` for a retail row.
pub fn texts(model: &Model, list: List, row: i16, label_only: bool) -> Option<Vec<(u32, u32)>> {
    let r = model.row(list, row)?;
    let t = model.texts(r);
    Some(if label_only { vec![t[0]] } else { t.to_vec() })
}

/// The item a row text call draws (`[esp+4]`) and its row.
fn text_call<M: Gm>(m: &M, esp: u32) -> Option<(u32, i16)> {
    let item = m.u32(esp.wrapping_add(4))?;
    Some((item, crate::speaker_row::item_row(m, item)?))
}

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

/// The handle of retail row `retail` on the list at `list` (its rows'
/// datum array at `+0x70`): what a moved retail row's call takes.
pub fn retail_handle(m: &dyn engine::memory::GuestMemory, list: u32, retail: i16) -> Option<u32> {
    let array = engine::datum::DatumArray::at(m.u32(list.wrapping_add(LIST_ROWS)).ok()?);
    Some(array.handle_of(m, i32::from(retail)).ok()??.0)
}

/// The rows' datum array of a list handler.
pub const LIST_ROWS: u32 = 0x70;

/// Row `row` of `list` when it is a retail row shown at another index:
/// the retail row's index.
pub fn moved(model: &Model, list: List, row: i16) -> Option<i16> {
    model.retail_index(list, row).filter(|r| *r != row)
}

fn row_text(c: &mut Call, e: &Arc<Ext>) -> Result<Flow, GuestFault> {
    if e.passing.load(Ordering::SeqCst) {
        return Ok(Flow::Continue);
    }
    let call = text_call(&*c.ctx.runtime().memory(), c.cpu.esp);
    if let Some((item, retail)) =
        call.and_then(|(item, row)| Some((item, moved(&e.model, e.list, row)?)))
    {
        let list = c.cpu.ecx;
        let word = c.ctx.runtime().memory().u32(c.cpu.esp + 8).unwrap_or(0);
        let Some(handle) = retail_handle(&*c.ctx.runtime().memory(), list, retail) else {
            return Ok(Flow::Continue);
        };
        let (e, text) = (Arc::clone(e), e.text);
        return c.answer_async(move |cx| -> EntryFuture {
            Box::pin(async move {
                let at = item + crate::pause_menu::ITEM_ROW;
                let was = cx.read(|m| m.u32(at))?;
                cx.write(|m| m.set_u32(at, handle))?;
                e.passing.store(true, Ordering::SeqCst);
                let r = cx
                    .call(GuestCall::new(text).ecx(list).args([item, word]))
                    .await;
                e.passing.store(false, Ordering::SeqCst);
                cx.write(|m| m.set_u32(at, was))?;
                r?;
                Ok(Ret::new(0, 8))
            })
        });
    }
    // The model reads the store: never with guest memory held.
    let Some((item, texts)) =
        call.and_then(|(item, row)| Some((item, texts(&e.model, e.list, row, e.label_only)?)))
    else {
        return Ok(Flow::Continue);
    };
    once(&e.drawn, &format!("{}: row text", e.id));
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, texts).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, e: &Arc<Ext>) -> Result<Flow, GuestFault> {
    if e.passing.load(Ordering::SeqCst) {
        return Ok(Flow::Continue);
    }
    let row = chosen_row(&*c.ctx.runtime().memory(), c.cpu.esp);
    if let Some(retail) = row.and_then(|r| moved(&e.model, e.list, r)) {
        let list = c.cpu.ecx;
        let (chain, handle) = {
            let m = c.ctx.runtime().memory();
            (
                m.u32(c.cpu.esp + 4).unwrap_or(0),
                retail_handle(&*m, list, retail),
            )
        };
        let Some(handle) = handle else {
            return Ok(Flow::Continue);
        };
        let (e, chosen, scratch) = (Arc::clone(e), e.chosen, e.scratch);
        return c.answer_async(move |cx| -> EntryFuture {
            Box::pin(async move {
                cx.write(|m| m.set_u32(scratch, handle))?;
                e.passing.store(true, Ordering::SeqCst);
                let r = cx
                    .call(GuestCall::new(chosen).ecx(list).args([chain, scratch]))
                    .await;
                e.passing.store(false, Ordering::SeqCst);
                r?;
                Ok(Ret::new(0, 8))
            })
        });
    }
    // The store is written with no guest memory held.
    let Some(chosen) = row
        .and_then(|r| e.model.row(e.list, r))
        .map(|r| e.model.press(r))
    else {
        return Ok(Flow::Continue);
    };
    match chosen {
        Chosen::Open(id) => c.answer_async(move |cx| -> EntryFuture {
            Box::pin(async move {
                let chain = cx.arg(0)?;
                list_class::open(&cx, chain, id).await?;
                Ok(Ret::new(0, 8))
            })
        }),
        Chosen::Stepped(msg) => {
            eprintln!("{msg}");
            c.ret(8).map(Flow::Return)
        }
        _ => c.ret(8).map(Flow::Return),
    }
}

/// What the hooks share.
struct Ext {
    id: &'static str,
    list: List,
    label_only: bool,
    model: Arc<Model>,
    drawn: AtomicBool,
    /// The row text and row chosen's addresses, for a moved retail row.
    text: u32,
    chosen: u32,
    /// A word for a moved retail row's press handle.
    scratch: u32,
    /// A moved retail row's call is going through to the original.
    passing: AtomicBool,
}

impl feature::Feature for Extension {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id, self.title).doc("docs/game/settings-menu.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let moves = (0..64).any(|i| moved(&self.model, self.list, i).is_some());
        let scratch = if moves {
            b.guest_alloc(0x1000, runtime::guest::Permissions::RW)?
        } else {
            0
        };
        let e = Arc::new(Ext {
            id: self.id,
            list: self.list,
            label_only: self.label_only,
            model: Arc::clone(&self.model),
            drawn: AtomicBool::new(false),
            text: self.text.base,
            chosen: self.chosen.base,
            scratch,
            passing: AtomicBool::new(false),
        });
        let t = Arc::clone(&e);
        b.raw()
            .before(self.text, "our rows' text", move |c| row_text(c, &t));
        b.raw()
            .before(self.chosen, "our rows' press", move |c| row_chosen(c, &e));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::ALL;
    use crate::video_screen::{self, SETTINGS_ROWS, Switches};
    use settings::Store;

    fn model() -> Arc<Model> {
        Arc::new(video_screen::model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap())
    }

    #[test]
    fn only_our_rows_are_answered_and_the_settings_row_draws_its_label() {
        let m = model();
        let title = m.title_id(video_screen::SCREEN).unwrap();
        assert_eq!(
            texts(&m, List::Settings, SETTINGS_ROWS as i16, true),
            Some(vec![(0, title)])
        );
        for r in [0i16, 1, 3, -1] {
            assert_eq!(texts(&m, List::Settings, r, true), None, "{r}");
        }
        assert_eq!(
            m.press(m.row(List::Settings, 2).unwrap()),
            Chosen::Open(video_screen::SCREEN)
        );
    }

    #[test]
    fn the_extension_hooks_the_lists_two_points_before_the_original() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        r.add(&Extension {
            id: "settings-rows",
            title: "Our rows on the Settings list",
            list: List::Settings,
            text: &engine::points::SETTINGS_ROW_TEXT,
            chosen: &engine::points::SETTINGS_ROW_CHOSEN,
            label_only: true,
            model: model(),
        })
        .unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        assert!(text.contains(
            "0x002b1d6e SETTINGS_ROW_TEXT: before [settings-rows:our rows' text] -> original\n"
        ));
        assert!(text.contains(
            "0x002b1da9 SETTINGS_ROW_CHOSEN: before [settings-rows:our rows' press] -> original\n"
        ));
    }

    /// The speaker row and the layouts' value text share the Controls row
    /// text: ours first (it answers for our rows), the layouts around the
    /// original.
    #[test]
    fn the_controls_row_text_is_one_ordered_chain() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        r.add(&crate::layout_glue::LayoutValueText).unwrap();
        r.add(&Extension {
            id: "controls-rows",
            title: "Our rows on the Controls list",
            list: List::Controls,
            text: &engine::points::CONTROLS_ROW_TEXT,
            chosen: &engine::points::CONTROLS_ROW_CHOSEN,
            label_only: false,
            model: model(),
        })
        .unwrap();
        let plan = r.build(&feature::Base).unwrap();
        let text = plan.text();
        assert!(
            text.contains(
                "0x002c8548 CONTROLS_ROW_TEXT: before [controls-rows:our rows' text, \
                 layouts-value:layout value pending] -> original -> after \
                 [layouts-value:layout value done]\n"
            ),
            "{text}"
        );
        assert!(text.contains(
            "0x002c8719 CONTROLS_ROW_CHOSEN: before [controls-rows:our rows' press] -> original\n"
        ));
        // The speaker row is ours, the retail six are not.
        let m = model();
        assert!(texts(&m, List::Controls, 6, false).is_some());
        assert!(texts(&m, List::Controls, 5, false).is_none());
    }
}
