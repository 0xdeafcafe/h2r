//! Player Settings keeps the retail controller and player lists, with Video
//! Settings and Voice appended to either native list. See
//! docs/game/settings-menu.md "Pause menu".

use guest_layout::DatumHandle;
use guest_layout::datum::DatumArray;
use guest_layout::entry::{Cx, EntryError, EntryFuture, Ret};
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use guest_layout::view::ui::{self, Item, ListHandler};
use hooks::{BuildError, Builder, Call, Feature, Flow, Manifest};
use runtime::GuestFault;

use crate::list_class;
use crate::speaker_row;
use crate::ui_kit::ids;
use crate::video_screen::{SCREEN as VIDEO, VOICE_SCREEN as VOICE};

const PLAYER: u32 = ids::family("pause settings").id(2);
pub const TEXTS: &[(u32, &str)] = &[(PLAYER, "Player Settings")];

const CAMPAIGN_SETTINGS_ROW: u16 = 3;
const MP_SETTINGS_ACTION: u16 = 1;
const ACTION_WORD: u32 = 2;

#[derive(Clone, Copy)]
pub enum Mode {
    Campaign,
    Multiplayer,
}

impl Mode {
    const fn feature(self) -> &'static str {
        match self {
            Self::Campaign => "pause-rows",
            Self::Multiplayer => "pause-mp-rows",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Video,
    Voice,
}

impl Page {
    pub const ALL: [Self; 2] = [Self::Video, Self::Voice];

    pub const fn value(self) -> u16 {
        match self {
            Self::Video => 9,
            Self::Voice => 10,
        }
    }

    pub const fn screen(self) -> &'static str {
        match self {
            Self::Video => VIDEO,
            Self::Voice => VOICE,
        }
    }
}

/// The two native lists selected by the pause menu's Settings action.
#[derive(Clone, Copy)]
pub enum NativeList {
    Controller,
    Player,
}

impl NativeList {
    pub const ALL: [Self; 2] = [Self::Controller, Self::Player];

    /// Constructor, row text and row chosen, in that order. Both lists use
    /// the same row callback signatures and datum layout.
    pub const fn hooks(self) -> [u32; 3] {
        match self {
            Self::Controller => [
                speaker_row::SMALL_CTOR,
                speaker_row::SMALL_ROW_TEXT,
                speaker_row::SMALL_ROW_CHOSEN,
            ],
            Self::Player => [0x002b_4f5c, 0x002b_508c, 0x002b_50d9],
        }
    }

    const fn array(self) -> (u32, u32) {
        match self {
            Self::Controller => (speaker_row::SMALL_LIST_NAME, speaker_row::VANILLA_ROWS),
            // The four possible retail rows are conditional. Grow the
            // capacity by option words, regardless of which rows exist.
            Self::Player => (0x0045_ba18, 4),
        }
    }

    pub fn from_array(name: u32, count: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|list| list.array() == (name, count))
    }

    pub fn controller(self) -> bool {
        matches!(self, Self::Controller)
    }
}

/// The installed child classes and the two per-mode feature switches.
#[derive(Clone, Copy)]
pub struct Pages {
    video: bool,
    voice: bool,
    campaign: bool,
    multiplayer: bool,
}

impl Pages {
    pub fn new(video: bool, voice: bool, switches: &hooks::Switches) -> Self {
        Self {
            video,
            voice,
            campaign: switches.feature(Mode::Campaign.feature(), true),
            multiplayer: switches.feature(Mode::Multiplayer.feature(), true),
        }
    }

    fn mode(self, mode: Mode) -> bool {
        (self.video || self.voice)
            && match mode {
                Mode::Campaign => self.campaign,
                Mode::Multiplayer => self.multiplayer,
            }
    }

    pub fn enabled(self, list: NativeList) -> bool {
        self.mode(Mode::Multiplayer) || list.controller() && self.mode(Mode::Campaign)
    }

    pub fn page(self, list: NativeList, kind: Option<u8>, action: Option<u16>) -> Option<Page> {
        self.for_game(list, kind)
            .find(|page| Some(page.value()) == action)
    }

    pub fn for_game(self, list: NativeList, kind: Option<u8>) -> impl Iterator<Item = Page> {
        let enabled = match kind {
            Some(0) => list.controller() && self.mode(Mode::Campaign),
            Some(1) => self.mode(Mode::Multiplayer),
            _ => false,
        };
        Page::ALL.into_iter().filter(move |page| {
            enabled
                && match page {
                    Page::Video => self.video,
                    Page::Voice => self.voice,
                }
        })
    }
}

fn action_word(m: &dyn GuestMemory, list: u32, handle: DatumHandle) -> Option<u16> {
    let rows = DatumArray::at(ListHandler::at(list).rows(m).ok()?);
    let datum = rows.try_get(m, handle).ok()??;
    m.u16(datum + ACTION_WORD).ok()
}

pub fn item_action(m: &dyn GuestMemory, list: u32, item: u32) -> Option<u16> {
    action_word(m, list, Item::at(item).row(m).ok()??)
}

pub fn chosen_action(m: &dyn GuestMemory, list: u32, row: u32) -> Option<u16> {
    action_word(m, list, DatumHandle::from_raw(m.u32(row).ok()?)?)
}

pub async fn open_page(cx: Cx, page: Page, list: u32) -> Result<Ret, EntryError> {
    list_class::push(&cx, cx.arg(0)?, list.wrapping_sub(ui::LIST), page.screen()).await?;
    Ok(Ret::new(0, 8))
}

fn settings_label(m: &dyn GuestMemory, mode: Mode, list: u32, item: u32) -> Option<u32> {
    let settings = match mode {
        Mode::Campaign => Item::at(item).row(m).ok()??.index() == CAMPAIGN_SETTINGS_ROW,
        Mode::Multiplayer => item_action(m, list, item) == Some(MP_SETTINGS_ACTION),
    };
    settings.then_some(PLAYER)
}

fn draw_label(c: &mut Call, mode: Mode) -> Result<Flow, GuestFault> {
    let Some(item) = c.arg_u32("item") else {
        return Ok(Flow::Continue);
    };
    let Some(label) = settings_label(&*c.ctx.runtime().memory(), mode, c.cpu.ecx, item) else {
        return Ok(Flow::Continue);
    };
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, label)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn campaign_settings_pressed(m: &dyn GuestMemory, arg: u32) -> bool {
    m.u32(arg)
        .ok()
        .and_then(DatumHandle::from_raw)
        .is_some_and(|handle| handle.index() == CAMPAIGN_SETTINGS_ROW)
}

pub struct Settings(pub Mode);

impl Feature for Settings {
    fn manifest(&self) -> Manifest {
        Manifest::new(self.0.feature(), "Player Settings on the pause menu")
            .doc("docs/game/settings-menu.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Snapshots: nothing to save. The rows are drawn from guest memory
        // on each call, and the feature holds no host state between calls;
        // an answer still in flight is a kernel continuation, which blocks
        // a snapshot on its own (kernel `quiesce.rs`), and a load restores
        // the guest this reads. Without this the features forbid every
        // keyframe for the whole session, in games too, not only while the
        // pause menu is up.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let mode = self.0;
        let text = match mode {
            Mode::Campaign => &points::PAUSE_ROW_TEXT,
            Mode::Multiplayer => &points::MP_MENU_ROW_TEXT,
        };
        b.raw()
            .before(text, "name the Player Settings row", move |c| {
                draw_label(c, mode)
            });
        // The native campaign action closes the pause menu before opening
        // Controller Settings. Push directly so Back restores the pause menu.
        // Multiplayer's native action already opens in place and chooses the
        // correct list for the pressing player.
        if matches!(mode, Mode::Campaign) {
            b.raw()
                .before(&points::PAUSE_ROW_CHOSEN, "open Player Settings", |c| {
                    let Some(row) = c.arg_u32("row") else {
                        return Ok(Flow::Continue);
                    };
                    if !campaign_settings_pressed(&*c.ctx.runtime().memory(), row) {
                        return Ok(Flow::Continue);
                    }
                    let over = c.cpu.ecx.wrapping_sub(ui::LIST);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(async move {
                            list_class::push_loader(
                                &cx,
                                cx.arg(0)?,
                                over,
                                points::PAUSE_CONTROLS_LOADER.base,
                            )
                            .await?;
                            Ok(Ret::new(0, 8))
                        })
                    })
                });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
