//! Back through the lobby's single-active-screen dialog channel.
//! Window 3 disposes of the previous screen at activation. Remember the
//! audited opener so the generic B/Back handler can recreate it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::points;

use crate::layout_glue::{Gm, ReadGm};
use crate::ui_kit::input::Input;
use crate::ui_kit::lifecycle::{SCREEN_SLOT, SCREEN_STATE, SCREEN_WINDOW};
use crate::ui_watch::{ACTIVE, PENDING, channel};

pub const FEATURE: &str = "ui-back";
pub(crate) const LOBBY_DIALOG: u32 = 3;
pub(crate) const GAME_SETUP_ID: u32 = 23;
pub(crate) const SELECT_MAP_ID: u32 = 0xce;
pub(crate) const SCREEN_ID: u32 = 0x70;
const CONTROLLER_MASK: u32 = 8;
const REQUEST_LOADER: u32 = 0x1c;
pub(crate) const REQUEST_FOCUS: u32 = 0x18;
const BACK_SOUND: u32 = 4;

/// Each entry belongs to one constructed screen until the channel frees it.
#[derive(Default)]
pub struct Navigation {
    pub(crate) maps: crate::map_select::Flow,
    game_setup: Mutex<BTreeMap<u32, u16>>,
}

impl Navigation {
    fn game_setup(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, u16>> {
        self.game_setup.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn setup_parent(&self, child: u32) -> Option<u16> {
        self.game_setup().get(&child).copied()
    }

    /// A map list replaces the folders on open, and folders replace it on
    /// Back. The new instance inherits their Game Setup return.
    pub(crate) fn inherit_setup(&self, from: u32, to: u32) {
        let mut parents = self.game_setup();
        if to != 0 && let Some(mask) = parents.get(&from).copied() {
            parents.insert(to, mask);
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DialogChannel {
    pub(crate) window: u32,
    pub(crate) slot: u32,
    pub(crate) mask: u16,
}

impl DialogChannel {
    fn of(m: &impl Gm, screen: u32) -> Option<(Self, u32)> {
        if screen == 0 {
            return None;
        }
        let window = m.u32(screen.wrapping_add(SCREEN_WINDOW))?;
        let slot = m.u32(screen.wrapping_add(SCREEN_SLOT))?;
        let mask = m.u16(screen.wrapping_add(CONTROLLER_MASK))?;
        let ch = channel(window, slot)?;
        Some((Self { window, slot, mask }, ch))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Back {
    Retail,
    Wait,
    Reopen,
}

fn back_target(input: Option<Input>, active: bool, pending: bool, closing: bool) -> Back {
    if !matches!(input, Some(Input::B | Input::Back)) || !active {
        Back::Retail
    } else if pending || closing {
        Back::Wait
    } else {
        Back::Reopen
    }
}

/// Gate a return owned by us on the same channel and active screen that
/// retail Back would close. A second Back during its transition is consumed.
pub(crate) fn back(m: &impl Gm, screen: u32, input: Option<Input>) -> (Back, Option<DialogChannel>) {
    let Some((source, ch)) = DialogChannel::of(m, screen) else {
        return (Back::Retail, None);
    };
    let active = source.window == LOBBY_DIALOG && m.u32(ch + ACTIVE) == Some(screen);
    let pending = m.u32(ch + PENDING) != Some(0);
    let closing = m.u8(screen.wrapping_add(SCREEN_STATE)).is_none_or(|state| state & 2 != 0);
    (back_target(input, active, pending, closing), Some(source))
}

pub(crate) fn request_input(m: &impl Gm, event: u32) -> Option<Input> {
    Input::from_event(m.u32(event)?, m.u32(event.wrapping_add(8))?)
}

/// The channel applies this request after initializing the incoming
/// screen's widgets. Bit 0 restores focus and closes the outgoing
/// screen with message 2, the same message as retail B.
pub(crate) async fn reopen(
    cx: &Cx,
    at: u32,
    source: DialogChannel,
    loader: u32,
) -> Result<u32, EntryError> {
    let mut request = crate::skulls_mp::open_request(source.mask, source.window, source.slot, loader);
    request[0] = 1;
    cx.write(|m| m.write(at, &request))?;
    let opened = cx.call(GuestCall::new(loader).args([at])).await?.eax;
    cx.call(GuestCall::new(points::UI_SOUND.base).eax(BACK_SOUND)).await?;
    Ok(opened)
}

fn is_setup_selector(id: u32, loader: u32) -> bool {
    match id {
        SELECT_MAP_ID => loader == points::SELECT_MAP_LOAD.base,
        0xcf => loader == points::LOBBY_VARIANT_LIST_LOAD.base,
        11 => loader == points::LOBBY_LEVEL_LIST_LOAD.base,
        12 => loader == points::LOBBY_DIFFICULTY_LOAD.base,
        _ => false,
    }
}

fn capture_setup(m: &impl Gm, child: u32, request: u32) -> Option<u16> {
    let (source, ch) = DialogChannel::of(m, child)?;
    if source.window != LOBBY_DIALOG
        || m.u16(request)? != 0
        || !is_setup_selector(m.u32(child.wrapping_add(SCREEN_ID))?, m.u32(request.wrapping_add(REQUEST_LOADER))?)
        || m.u32(ch + PENDING)? != 0
    {
        return None;
    }
    let parent = m.u32(ch + ACTIVE)?;
    if parent == 0
        || m.u32(parent.wrapping_add(SCREEN_ID))? != GAME_SETUP_ID
        || m.u8(parent.wrapping_add(SCREEN_STATE))? & 2 != 0
    {
        return None;
    }
    let mask = m.u16(parent.wrapping_add(CONTROLLER_MASK))?;
    (mask & source.mask != 0).then_some(mask)
}

pub struct RetailBack(pub Arc<Navigation>);

impl hooks::Feature for RetailBack {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Back returns to Game Setup from its nested selectors")
            .doc("docs/game/ui-screens.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let request_at = b.guest_alloc(0x20, runtime::guest::Permissions::RW)?;
        let nav = Arc::clone(&self.0);
        b.raw().observe_before(&points::SCREEN_PUSH, "remember the replaced Game Setup", move |o| {
            let (child, request) = (o.cpu.edi, o.cpu.ebx);
            let m = o.guest();
            if let Some(mask) = capture_setup(&ReadGm(&*m), child, request) {
                nav.game_setup().insert(child, mask);
            }
        });
        let nav = Arc::clone(&self.0);
        b.raw().observe_before(&points::UI_CHANNEL_FREE, "forget the freed screen's opener", move |o| {
            if let Some(screen) = o.arg_u32("screen") {
                nav.game_setup().remove(&screen);
            }
        });
        let nav = Arc::clone(&self.0);
        b.raw()
            .before(&points::SCREEN_BACK_EVENT, "return to the replaced Game Setup", move |c| {
                let screen = c.cpu.ecx;
                let Some(mask) = nav.setup_parent(screen) else {
                    return Ok(hooks::Flow::Continue);
                };
                let (action, source) = {
                    let m = c.ctx.runtime().memory();
                    let mem = ReadGm(&*m);
                    let input = m
                        .u32(c.cpu.esp + 4)
                        .ok()
                        .and_then(|event| request_input(&mem, event));
                    back(&mem, screen, input)
                };
                match (action, source) {
                    (Back::Wait, _) => {
                        c.cpu.eax = 1;
                        c.ret(4).map(hooks::Flow::Return)
                    }
                    (Back::Reopen, Some(mut source)) => {
                        source.mask = mask;
                        c.answer_async(move |cx| Box::pin(async move {
                            reopen(&cx, request_at, source, points::GAME_SETUP_LOAD.base).await?;
                            Ok(Ret::new(1, 4))
                        }))
                    }
                    _ => Ok(hooks::Flow::Continue),
                }
            })
            .after_feature(crate::map_select::FEATURE);
        let nav = Arc::clone(&self.0);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            (!nav.game_setup().is_empty()).then(|| "a lobby selector has a Game Setup return".into())
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cancel_of_the_active_screen_reopens_the_parent() {
        assert_eq!(back_target(Some(Input::B), true, false, false), Back::Reopen);
        assert_eq!(back_target(Some(Input::Back), true, false, false), Back::Reopen);
        assert_eq!(back_target(Some(Input::A), true, false, false), Back::Retail);
        assert_eq!(back_target(Some(Input::B), false, false, false), Back::Retail);
        assert_eq!(back_target(Some(Input::B), true, true, false), Back::Wait);
        assert_eq!(back_target(Some(Input::B), true, false, true), Back::Wait);
    }

    #[test]
    fn only_audited_lobby_selector_loaders_are_eligible() {
        assert!(is_setup_selector(SELECT_MAP_ID, points::SELECT_MAP_LOAD.base));
        assert!(is_setup_selector(0xcf, points::LOBBY_VARIANT_LIST_LOAD.base));
        assert!(is_setup_selector(11, points::LOBBY_LEVEL_LIST_LOAD.base));
        assert!(is_setup_selector(12, points::LOBBY_DIFFICULTY_LOAD.base));
        assert!(!is_setup_selector(12, points::LOBBY_LEVEL_LIST_LOAD.base));
        assert!(!is_setup_selector(GAME_SETUP_ID, points::GAME_SETUP_LOAD.base));
    }
    #[test]
    fn setup_must_be_the_active_opener_of_the_same_controller() {
        let mut mem = engine::testing::FakeGuest::new();
        let slot = 4;
        let channel = channel(LOBBY_DIALOG, slot).unwrap();
        let setup = 0x10000;
        let child = 0x11000;
        let request = 0x12000;
        mem.put_u32(channel + ACTIVE, setup)
            .put_u32(setup + SCREEN_ID, GAME_SETUP_ID)
            .put_u16(setup + CONTROLLER_MASK, 1)
            .put_u32(child + SCREEN_ID, SELECT_MAP_ID)
            .put_u32(child + SCREEN_WINDOW, LOBBY_DIALOG)
            .put_u32(child + SCREEN_SLOT, slot)
            .put_u16(child + CONTROLLER_MASK, 1)
            .put_u32(request + REQUEST_LOADER, points::SELECT_MAP_LOAD.base);
        assert_eq!(capture_setup(&ReadGm(&mem), child, request), Some(1));

        mem.put_u16(child + CONTROLLER_MASK, 2);
        assert_eq!(capture_setup(&ReadGm(&mem), child, request), None);
        mem.put_u16(child + CONTROLLER_MASK, 1);
        mem.put_u32(channel + PENDING, 0x13000);
        assert_eq!(capture_setup(&ReadGm(&mem), child, request), None);
        mem.put_u32(channel + PENDING, 0);
        mem.put_u8(setup + SCREEN_STATE, 2);
        assert_eq!(capture_setup(&ReadGm(&mem), child, request), None);
    }

    #[test]
    fn back_returns_to_setup_by_default_and_accepts_the_off_switch() {
        for (switches, enabled) in [("", true), ("-ui-back", false)] {
            let mut registry = hooks::Registry::new(hooks::Switches::parse(switches).unwrap());
            assert_eq!(registry.add(&RetailBack(Arc::default())).unwrap(), enabled);
        }
    }

}
