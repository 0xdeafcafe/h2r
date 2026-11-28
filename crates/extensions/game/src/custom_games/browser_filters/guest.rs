//! Attach filter controls and copy to the Live instance of the retail browser.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::{points, view::ui::Screen};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

use super::{Browser, CHROME, RETAIL_FOOTER, RETAIL_TITLE, current, lock};
use crate::custom_games::{BROWSER_EVENT, browser_is_live};
use crate::layout_glue::{Gm, ReadGm};
use crate::list_class;
use crate::ui_back;
use crate::ui_kit::dialog_tag;
use crate::ui_kit::input::{Glyph, Input, Prompt, footer_legend};
use crate::ui_kit::lifecycle::SCREEN_STATE;
use crate::ui_kit::screen::{SLOT_LEN, SlotTexts};
use crate::ui_watch::{ACTIVE, SHARED, channel};

const BROWSER_WINDOW: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Open { mask: u16 },
    Clear,
}

fn event(m: &impl Gm, at: u32, filtered: bool) -> Option<Event> {
    match ui_back::request_input(m, at)? {
        // The screen event's word at +4 is the local controller index
        // (docs/game/skulls.md, "The press that opened it").
        Input::Y => {
            let player = m.u32(at.wrapping_add(4))?;
            (player < 4).then(|| Event::Open {
                mask: 1u16 << player,
            })
        }
        Input::X if filtered => Some(Event::Clear),
        _ => None,
    }
}

pub(crate) fn handle_event(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
) -> Option<Result<TransferAbi, GuestFault>> {
    let browser = current()?;
    let filtered = browser.active_count() != 0;
    let decorated = {
        let m = ctx.runtime().memory();
        let filter_event = m.u32(cpu.esp.wrapping_add(4)).ok()?;
        event(&ReadGm(&*m), filter_event, filtered)?
    };
    match decorated {
        Event::Open { mask } => {
            let loader = list_class::loader_of(super::SCREEN)?;
            let dialog = dialog_tag::by_loader(loader)?;
            Some(hooks::entry::start_with(
                cpu,
                ctx,
                BROWSER_EVENT,
                move |cx| {
                    Box::pin(async move {
                        // The browser remains in window 5, the filters page
                        // goes in window 3, and the picker opens above both
                        // on window 1. B closes the top one and keeps focus.
                        let at = dialog.page + dialog_tag::REQUEST_AT;
                        let request = dialog_tag::request(ui_back::LOBBY_DIALOG, mask, loader);
                        cx.write(|m| m.write(at, &request))?;
                        cx.call(GuestCall::new(loader).args([at])).await?;
                        Ok(Ret::new(1, 4))
                    })
                },
            ))
        }
        Event::Clear => {
            browser.clear();
            cpu.eax = 1;
            Some(ctx.runtime().memory().ret(cpu, 4))
        }
    }
}

fn active_browser(m: &impl Gm) -> Option<u32> {
    [SHARED, 0, 1, 2, 3]
        .into_iter()
        .filter_map(|slot| channel(BROWSER_WINDOW, slot))
        .filter_map(|ch| m.u32(ch + ACTIVE))
        .find(|&screen| {
            screen != 0
                && browser_is_live(m, screen)
                && m.u8(screen.wrapping_add(SCREEN_STATE))
                    .is_some_and(|state| state & 2 == 0)
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Footer {
    Standard,
    Filtered,
    Empty,
    NoMatches,
    Unsupported,
}

impl Footer {
    fn for_result(count: usize, matching: Option<usize>, supported: Option<bool>) -> Self {
        match (count, matching, supported) {
            (0, Some(0), _) => Self::Empty,
            (0, _, _) => Self::Standard,
            (_, _, Some(false)) => Self::Unsupported,
            (_, Some(0), _) => Self::NoMatches,
            _ => Self::Filtered,
        }
    }

    fn text(self) -> String {
        match self {
            Self::Empty => "No games found. Press Y for filters".into(),
            Self::NoMatches => "No games match. Press X to clear filters".into(),
            Self::Unsupported => "Filters unavailable. Press X to clear".into(),
            Self::Standard | Self::Filtered => {
                let mut prompts = vec![
                    Prompt::new(&[Glyph::A], "Select"),
                    Prompt::new(&[Glyph::B], "Back"),
                    Prompt::new(&[Glyph::Y], "Filters"),
                ];
                if self == Self::Filtered {
                    prompts.push(Prompt::new(&[Glyph::X], "Clear"));
                }
                footer_legend(&prompts)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Chrome {
    screen: u32,
    filters: usize,
    footer: Footer,
}

impl Chrome {
    fn title(self) -> String {
        match self.filters {
            0 => "Custom Games (All)".into(),
            1 => "Custom Games (1 filter)".into(),
            n => format!("Custom Games ({n} filters)"),
        }
    }

    async fn draw(self, cx: &Cx, texts: &SlotTexts) -> Result<(), EntryError> {
        let ids = [
            (
                Screen::at(self.screen).header(),
                texts.set(RETAIL_TITLE, &self.title()),
            ),
            (
                self.screen + list_class::FOOTER,
                texts.set(RETAIL_FOOTER, &self.footer.text()),
            ),
        ];
        cx.write(|m| texts.write(m))?;
        for (widget, id) in ids {
            cx.call(
                GuestCall::new(points::TEXT_SET.base)
                    .edi(widget)
                    .esi(self.screen)
                    .args([id]),
            )
            .await?;
        }
        Ok(())
    }
}

/// The retail browser's header and footer while Live browses: its own
/// texts (the "browser chrome" family), over the filters in force.
pub(super) struct Presentation {
    browser: Arc<Browser>,
    texts: Arc<SlotTexts>,
}

impl Presentation {
    pub(super) fn new(browser: Arc<Browser>) -> Self {
        Presentation {
            browser,
            texts: Arc::new(SlotTexts::new(CHROME)),
        }
    }
}

impl Feature for Presentation {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "browser-filter-texts",
            "The Live custom game's browser and filter texts",
        )
        .doc("docs/live/live-custom-games.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let texts = Arc::clone(&self.texts);
        let bytes = CHROME.len * SLOT_LEN;
        let base = b.guest_alloc(bytes.next_multiple_of(0x1000), Permissions::RW)?;
        texts.base.store(base, Ordering::SeqCst);
        b.locale()
            .source("browser.chrome", move |id| texts.address(id));

        let drawn = Arc::new(Mutex::new(None));
        let restore = Arc::clone(&self.texts);
        let restore_drawn = Arc::clone(&drawn);
        b.snapshot(SnapshotPolicy::section_forbid_while(
            || None,
            Vec::new,
            move |_, _| {
                restore.rewrite_all();
                *lock(&restore_drawn) = None;
                Ok(())
            },
        ));

        let pane_drawn = Arc::clone(&drawn);
        b.raw()
            .observe_before(
                &points::SCREEN_SETUP_PANE,
                "refresh a rebuilt browser header",
                move |o| {
                    let screen = o.cpu.ecx;
                    if browser_is_live(&ReadGm(&*o.guest()), screen) {
                        *lock(&pane_drawn) = None;
                    }
                },
            )
            .optional();

        let (browser, texts) = (Arc::clone(&self.browser), Arc::clone(&self.texts));
        b.raw()
            .before(
                &points::UI_SYNC_NETWORK_SCREEN,
                "refresh the Live browser header and footer",
                move |c| {
                    let screen = active_browser(&ReadGm(&*c.ctx.runtime().memory()));
                    let Some(screen) = screen else {
                        *lock(&drawn) = None;
                        return Ok(Flow::Continue);
                    };
                    let filters = browser.active_count();
                    let (matching, supported) = browser
                        .hle()
                        .map_or((None, None), |h| h.browser_result());
                    let chrome = Chrome {
                        screen,
                        filters,
                        footer: Footer::for_result(filters, matching, supported),
                    };
                    {
                        let mut last = lock(&drawn);
                        if *last == Some(chrome) {
                            return Ok(Flow::Continue);
                        }
                        // Leave the nested call to the retail reconciler.
                        *last = Some(chrome);
                    }
                    let texts = Arc::clone(&texts);
                    c.answer_async(move |cx| {
                        Box::pin(async move {
                            let ret = cx
                                .call(GuestCall::new(points::UI_SYNC_NETWORK_SCREEN.base))
                                .await?;
                            if cx.read(|m| active_browser(&ReadGm(m))) == Some(screen) {
                                chrome.draw(&cx, &texts).await?;
                            }
                            Ok(Ret::new(ret.eax, 0))
                        })
                    })
                },
            )
            .last()
            .optional();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn the_browser_keeps_its_standard_buttons_and_uses_y_and_filtered_x() {
        let mut memory = FakeGuest::new();
        let at = 0x1000;
        memory.map(at, 0x1000);
        memory.put_u32(at + 4, 2);
        for (button, filtered, expected) in [
            (Input::Y, false, Some(Event::Open { mask: 4 })),
            (Input::X, false, None),
            (Input::X, true, Some(Event::Clear)),
            (Input::A, true, None),
            (Input::B, true, None),
        ] {
            let (kind, button) = button.event();
            memory.put_u32(at, kind).put_u32(at + 8, button);
            assert_eq!(event(&ReadGm(&memory), at, filtered), expected);
        }
    }

    #[test]
    fn empty_and_unsupported_filters_have_distinct_status_copy() {
        assert_eq!(Footer::for_result(0, Some(0), Some(true)), Footer::Empty);
        assert_eq!(Footer::for_result(3, None, Some(true)), Footer::Filtered);
        assert_eq!(
            Footer::for_result(3, Some(0), Some(true)).text(),
            "No games match. Press X to clear filters"
        );
        assert_eq!(
            Footer::for_result(3, None, Some(false)),
            Footer::Unsupported
        );
    }

    #[test]
    fn a_filter_summary_and_prompts_fit_the_browser_header_and_footer() {
        for count in 0..=8 {
            let title = Chrome {
                screen: 0,
                filters: count,
                footer: Footer::Standard,
            }
            .title();
            assert!(crate::ui_kit::text_fit::width(&title) <= crate::ui_kit::text_fit::TITLE);
        }
        for footer in [
            Footer::Standard,
            Footer::Filtered,
            Footer::Empty,
            Footer::NoMatches,
            Footer::Unsupported,
        ] {
            let text = footer.text();
            let room = crate::ui_kit::dialog::LAYOUT1_FOOTER;
            assert!(crate::ui_kit::text_fit::width(&text) <= i32::from(room.right - room.left));
            assert!(
                crate::ui_kit::spec::fallback::width(&text)
                    <= crate::ui_kit::spec::fallback::FOOTER
            );
        }
    }
}
