//! Settings under the campaign and multiplayer pause menus. The Settings
//! list and its Video and Voice pages use the in-game Controller Settings
//! skin and sit on the pressing player's window and slot. B or Back closes
//! only the top screen. See docs/game/settings-menu.md "Pause menu".

use std::sync::Arc;

use engine::DatumHandle;
use engine::datum::DatumArray;
use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::GuestMemory;
use engine::points;
use engine::view::ui::{self, Item, ListHandler, Screen};
use hooks::{BuildError, Builder, Call, Feature, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

use crate::list_class::{self, ListClass, Rows};
use crate::ui::{Chosen, Model};
use crate::ui_kit::{ids, lifecycle};
use crate::video_screen::{SCREEN as VIDEO, VOICE_SCREEN as VOICE};

const STRINGS: ids::Family = ids::family("pause settings");
const TITLE: u32 = STRINGS.id(0);
const CONTROLLER: u32 = STRINGS.id(1);
const PLAYER: u32 = STRINGS.id(2);

pub const TEXTS: &[(u32, &str)] = &[
    (TITLE, "Settings"),
    (CONTROLLER, "Controller Settings"),
    (PLAYER, "Player Settings"),
];

const CAMPAIGN_SETTINGS_ROW: u16 = 3;
const MP_PAUSE: i16 = 195;
const MP_ROWS: i32 = 6;
const MP_SETTINGS_ACTION: u16 = 1;
const ACTION_WORD: u32 = 2;

#[derive(Clone, Copy)]
pub enum Mode {
    Campaign,
    Multiplayer,
}

impl Mode {
    fn screen(self) -> &'static str {
        match self {
            Self::Campaign => "pause-settings",
            Self::Multiplayer => "mp-pause-settings",
        }
    }

    fn retail_settings_label(self) -> u32 {
        match self {
            Self::Campaign => CONTROLLER,
            Self::Multiplayer => PLAYER,
        }
    }
}

struct SettingsRows {
    mode: Mode,
    pages: Vec<(&'static str, u32)>,
}

impl Rows for SettingsRows {
    fn title(&self) -> u32 {
        TITLE
    }

    fn count(&self) -> usize {
        1 + self.pages.len()
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        if row == 0 {
            return vec![(0, self.mode.retail_settings_label())];
        }
        self.page(row)
            .map_or_else(Vec::new, |&(_, id)| vec![(0, id)])
    }

    fn press(&self, row: i16) -> Chosen {
        if row == 0 {
            return Chosen::Act;
        }
        self.page(row)
            .map_or(Chosen::Nothing, |&(screen, _)| Chosen::Push(screen))
    }
}

impl SettingsRows {
    fn page(&self, row: i16) -> Option<&(&'static str, u32)> {
        self.pages.get(usize::try_from(i32::from(row) - 1).ok()?)
    }
}

pub struct Settings {
    mode: Mode,
    pages: Vec<(&'static str, u32)>,
}

impl Settings {
    pub fn new(mode: Mode, model: &Model, video: bool, voice: bool) -> Option<Self> {
        let pages: Vec<_> = [(VIDEO, video), (VOICE, voice)]
            .into_iter()
            .filter(|(_, enabled)| *enabled)
            .filter_map(|(page, _)| model.title_id(page).map(|id| (page, id)))
            .collect();
        (!pages.is_empty()).then_some(Self { mode, pages })
    }

    fn class(&self, retail_row: u32) -> ListClass {
        let mode = self.mode;
        ListClass::with_rows(
            mode.screen(),
            mode.screen(),
            Arc::new(SettingsRows {
                mode,
                pages: self.pages.clone(),
            }),
        )
        .action(Arc::new(move |cx, over| {
            Box::pin(open_retail_settings(cx, over, mode, retail_row))
        }))
        .action_buttons(&[])
    }
}

fn mp_rows(m: &dyn GuestMemory, list: u32) -> Option<DatumArray> {
    let at = ListHandler::at(list).rows(m).ok()?;
    (at != 0).then_some(DatumArray::at(at))
}

fn mp_action(m: &dyn GuestMemory, rows: DatumArray, handle: DatumHandle) -> Option<u16> {
    let datum = rows.try_get(m, handle).ok()??;
    m.u16(datum + ACTION_WORD).ok()
}

/// The MP pause screen stays immediately below Settings on the same stack.
/// Find the native Settings action by its datum value, independent of which
/// optional rows are present.
fn mp_settings(m: &dyn GuestMemory, over: u32) -> Option<(u32, DatumHandle)> {
    let parent = m.u32(over + lifecycle::SCREEN_NEXT).ok()?;
    if parent == 0 || Screen::at(parent).id(m).ok()? != MP_PAUSE {
        return None;
    }
    let list = parent + ui::LIST;
    let rows = mp_rows(m, list)?;
    for index in 0..MP_ROWS {
        if let Some(handle) = rows.handle_of(m, index).ok()?
            && mp_action(m, rows, handle) == Some(MP_SETTINGS_ACTION)
        {
            return Some((list, handle));
        }
    }
    None
}

async fn open_retail_settings(
    cx: Cx,
    over: u32,
    mode: Mode,
    retail_row: u32,
) -> Result<(), EntryError> {
    let chain = cx.arg(0)?;
    match mode {
        // The native campaign press also closes the pause root. Opening its
        // loader directly preserves the Settings and pause screens below it.
        Mode::Campaign => {
            list_class::push_loader(&cx, chain, over, points::PAUSE_CONTROLS_LOADER.base).await
        }
        // The original chooses Controller Settings or Player Settings for
        // this player. Give it the real row handle and pause list.
        Mode::Multiplayer => {
            if let Some((list, row)) = cx.read(|m| mp_settings(m, over)) {
                cx.write(|m| m.set_u32(retail_row, row.raw()))?;
                cx.call(
                    GuestCall::new(points::MP_MENU_ROW_CHOSEN.base)
                        .ecx(list)
                        .args([chain, retail_row]),
                )
                .await?;
            }
            Ok(())
        }
    }
}

fn campaign_row_label(m: &dyn GuestMemory, item: u32) -> Option<u32> {
    let handle = Item::at(item).row(m).ok()??;
    (handle.index() == CAMPAIGN_SETTINGS_ROW).then_some(TITLE)
}

fn settings_pressed(m: &dyn GuestMemory, list: u32, row: u32, mode: Mode) -> bool {
    let Some(handle) = m.u32(row).ok().and_then(DatumHandle::from_raw) else {
        return false;
    };
    match mode {
        Mode::Campaign => handle.index() == CAMPAIGN_SETTINGS_ROW,
        Mode::Multiplayer => {
            mp_rows(m, list).and_then(|rows| mp_action(m, rows, handle)) == Some(MP_SETTINGS_ACTION)
        }
    }
}

fn open_settings(c: &mut Call, mode: Mode, retail_row: u32) -> Result<Flow, GuestFault> {
    let Some(row) = c.arg_u32("row") else {
        return Ok(Flow::Continue);
    };
    let list = c.cpu.ecx;
    // The MP hub also calls this handler to run the original Settings
    // action. Only its dedicated row argument bypasses our redirect.
    if row == retail_row || !settings_pressed(&*c.ctx.runtime().memory(), list, row, mode) {
        return Ok(Flow::Continue);
    }
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            let chain = cx.arg(0)?;
            list_class::push(&cx, chain, list.wrapping_sub(ui::LIST), mode.screen()).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl Feature for Settings {
    fn manifest(&self) -> Manifest {
        let id = match self.mode {
            Mode::Campaign => "pause-rows",
            Mode::Multiplayer => "pause-mp-rows",
        };
        Manifest::new(id, "Settings on the pause menu").doc("docs/game/settings-menu.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let mode = self.mode;
        let retail_row = match mode {
            Mode::Campaign => {
                b.raw()
                    .before(&points::PAUSE_ROW_TEXT, "name the Settings row", |c| {
                        let Some(item) = c.arg_u32("item") else {
                            return Ok(Flow::Continue);
                        };
                        let Some(id) = campaign_row_label(&*c.ctx.runtime().memory(), item) else {
                            return Ok(Flow::Continue);
                        };
                        c.answer_async(move |cx| -> EntryFuture {
                            Box::pin(async move {
                                list_class::draw_texts(&cx, item, vec![(0, id)]).await?;
                                Ok(Ret::new(0, 8))
                            })
                        })
                    });
                b.raw()
                    .before(&points::PAUSE_ROW_CHOSEN, "open Settings", |c| {
                        open_settings(c, Mode::Campaign, 0)
                    });
                0
            }
            Mode::Multiplayer => {
                let retail_row = b.guest_alloc(0x1000, Permissions::RW)?;
                b.raw()
                    .before(&points::MP_MENU_ROW_CHOSEN, "open Settings", move |c| {
                        open_settings(c, Mode::Multiplayer, retail_row)
                    });
                retail_row
            }
        };
        self.class(retail_row).build(b)
    }
}

#[cfg(test)]
mod tests;
