//! Rows of ours on a retail list, as data (docs/engine/feature-engine.md "UI step
//! c"): the `ui` model's extension rows of that list, answered by `before`
//! hooks on the plan at the list's row text and row chosen. A retail row
//! goes on to the original; a row of ours never reaches it.
//!
//! The row count (the shared datum array constructor) and the pane hook are
//! still `layout_glue`'s raw hooks: they serve every list at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use guest_layout::entry::{EntryFuture, Ret};
use guest_layout::points::Point;
use hooks::{BuildError, Builder, Call, Flow, Manifest};
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
/// datum array at `+0x70`).
pub fn retail_handle(m: &dyn guest_layout::memory::GuestMemory, list: u32, retail: i16) -> Option<u32> {
    let array = guest_layout::datum::DatumArray::at(m.u32(list.wrapping_add(LIST_ROWS)).ok()?);
    Some(array.handle_of(m, i32::from(retail)).ok()??.0)
}

/// The rows' datum array of a list handler.
pub const LIST_ROWS: u32 = 0x70;
/// An item widget's row handle.
pub const ITEM_ROW: u32 = 0x70;

fn row_text(c: &mut Call, e: &Arc<Ext>) -> Result<Flow, GuestFault> {
    let call = text_call(&*c.ctx.runtime().memory(), c.cpu.esp);
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
    let row = chosen_row(&*c.ctx.runtime().memory(), c.cpu.esp);
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
}

impl hooks::Feature for Extension {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.id, self.title).doc("docs/game/settings-menu.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let e = Arc::new(Ext {
            id: self.id,
            list: self.list,
            label_only: self.label_only,
            model: Arc::clone(&self.model),
            drawn: AtomicBool::new(false),
        });
        let t = Arc::clone(&e);
        b.raw()
            .before(self.text, "our rows' text", move |c| row_text(c, &t));
        b.raw()
            .before(self.chosen, "our rows' press", move |c| row_chosen(c, &e));
        // Texts and presses go through the model, whose strings are fixed
        // at build and whose values are the settings store's, kept outside
        // the snapshot; their async answers are continuations the machine
        // check sees. `drawn` only quiets a log line.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
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
        let voice = m.title_id(video_screen::VOICE_SCREEN).unwrap();
        assert_eq!(
            texts(&m, List::Settings, SETTINGS_ROWS as i16 + 1, true),
            Some(vec![(0, voice)])
        );
        for r in [0i16, 1, 4, -1] {
            assert_eq!(texts(&m, List::Settings, r, true), None, "{r}");
        }
        assert_eq!(
            m.press(m.row(List::Settings, 2).unwrap()),
            Chosen::Open(video_screen::SCREEN)
        );
        assert_eq!(
            m.press(m.row(List::Settings, 3).unwrap()),
            Chosen::Open(video_screen::VOICE_SCREEN)
        );
    }

    #[test]
    fn the_extension_hooks_the_lists_two_points_before_the_original() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&Extension {
            id: "settings-rows",
            title: "Our rows on the Settings list",
            list: List::Settings,
            text: &guest_layout::points::SETTINGS_ROW_TEXT,
            chosen: &guest_layout::points::SETTINGS_ROW_CHOSEN,
            label_only: true,
            model: model(),
        })
        .unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
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
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&crate::layout_glue::LayoutValueText).unwrap();
        r.add(&Extension {
            id: "controls-rows",
            title: "Our rows on the Controls list",
            list: List::Controls,
            text: &guest_layout::points::CONTROLS_ROW_TEXT,
            chosen: &guest_layout::points::CONTROLS_ROW_CHOSEN,
            label_only: false,
            model: model(),
        })
        .unwrap();
        let plan = r.build(&hooks::Base).unwrap();
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
