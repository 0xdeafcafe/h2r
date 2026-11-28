//! Back through the lobby's single-active-screen dialog channel.
//! Window 3 disposes of the previous screen at activation. Remember the
//! audited opener so the generic B/Back handler can recreate it.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::points;

use crate::layout_glue::{Gm, ReadGm};
use crate::ui_kit::input::Input;
use crate::ui_kit::lifecycle::{SCREEN_SLOT, SCREEN_STATE, SCREEN_WINDOW};
use crate::ui_watch::{ACTIVE, PENDING, channel};

pub const FEATURE: &str = "ui-back";
pub(crate) const LOBBY_DIALOG: u32 = 3;
pub(crate) const GAME_SETUP_ID: u32 = 23;
pub(crate) const SELECT_MAP_ID: u32 = 0xce;
pub(crate) const SCREEN_ID: u32 = 0x70;
pub(crate) const CATEGORY_ID: u32 = 207;
const VARIANTS_ID: u32 = 208;
const CONTROLLER_MASK: u32 = 8;
const REQUEST_LOADER: u32 = 0x1c;
pub(crate) const REQUEST_FOCUS: u32 = 0x18;
const BACK_SOUND: u32 = 4;

/// Each entry belongs to one constructed screen until the channel frees it.
#[derive(Default)]
pub struct Navigation {
    pub(crate) maps: crate::map_select::Flow,
    categories: Mutex<BTreeMap<u32, CategoryReturn>>,
    game_setup: Mutex<BTreeMap<u32, SetupReturn>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SetupReturn {
    mask: u16,
    action: Option<i16>,
}

#[derive(Clone, Copy)]
pub(crate) struct CategoryReturn {
    mask: u16,
    focus: Option<i16>,
    pub(crate) setup: Option<SetupReturn>,
}

impl Navigation {
    fn game_setup(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, SetupReturn>> {
        self.game_setup.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn setup_parent(&self, child: u32) -> Option<SetupReturn> {
        self.game_setup().get(&child).copied()
    }

    pub(crate) fn category_parent(&self, screen: u32) -> Option<CategoryReturn> {
        self.categories.lock().unwrap_or_else(|e| e.into_inner()).get(&screen).copied()
    }

    /// Capture a lobby replacement before retail disposes of its predecessor.
    /// Resolve the saved focus against the newly constructed parent list.
    fn pushed(&self, m: &dyn guest_layout::GuestMemory, child: u32, request: u32) -> Option<u32> {
        let mem = ReadGm(m);
        if let Some(setup) = capture_setup(m, child, request) {
            self.game_setup().insert(child, setup);
            return None;
        }
        let (source, ch) = DialogChannel::of(&mem, child)?;
        let parent = mem.u32(ch + ACTIVE)?;
        if source.window != LOBBY_DIALOG || mem.u32(ch + PENDING)? != 0 || parent == 0 {
            return None;
        }
        let mask = mem.u16(parent + CONTROLLER_MASK)?;
        if mask & source.mask == 0 {
            return None;
        }
        let parent_id = mem.u32(parent + SCREEN_ID)?;
        let child_id = mem.u32(child + SCREEN_ID)?;
        match (parent_id, child_id, mem.u16(request)?) {
            (SELECT_MAP_ID, SELECT_MAP_ID, 0 | 1)
                if mem.u32(request + REQUEST_LOADER)? == points::SELECT_MAP_LOAD.base =>
            {
                if let Some(setup) = self.setup_parent(parent) {
                    self.game_setup().insert(child, setup);
                }
                None
            }
            (_, GAME_SETUP_ID, 1) if mem.u32(request + REQUEST_LOADER)? == points::GAME_SETUP_LOAD.base => {
                let action = self.setup_parent(parent)?.action?;
                setup_handle(m, child + crate::game_setup_help::SETUP_LIST, action)
            }
            (CATEGORY_ID, VARIANTS_ID, 0) if mem.u8(parent + SCREEN_STATE)? & 2 == 0 => {
                let list = parent + crate::tweakers::SCREEN_LIST;
                let focus = crate::ui_kit::lifecycle::focused_row_handle(m, ch, list)
                    .map(|handle| handle as i16);
                let target = CategoryReturn { mask, focus, setup: self.setup_parent(parent) };
                self.categories.lock().unwrap_or_else(|e| e.into_inner()).insert(child, target);
                None
            }
            (VARIANTS_ID, CATEGORY_ID, 1)
                if mem.u32(request + REQUEST_LOADER)? == points::LOBBY_VARIANT_LIST_LOAD.base =>
            {
                let target = self.category_parent(parent)?;
                if let Some(setup) = target.setup {
                    self.game_setup().insert(child, setup);
                }
                crate::retail_rows::retail_handle(m, child + crate::tweakers::SCREEN_LIST, target.focus?)
            }
            _ => None,
        }
    }

    fn freed(&self, screen: u32) {
        self.game_setup().remove(&screen);
        self.categories.lock().unwrap_or_else(|e| e.into_inner()).remove(&screen);
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
        CATEGORY_ID => loader == points::LOBBY_VARIANT_LIST_LOAD.base,
        11 => loader == points::LOBBY_LEVEL_LIST_LOAD.base,
        12 => loader == points::LOBBY_DIFFICULTY_LOAD.base,
        _ => false,
    }
}

fn setup_handle(m: &dyn guest_layout::GuestMemory, list: u32, action: i16) -> Option<u32> {
    use crate::h1_options_row::{SETUP_ACTION, SETUP_ROWS};
    use guest_layout::datum::{DatumArray, layout::HIGH_WATER};

    let array = DatumArray::at(m.u32(list + SETUP_ROWS).ok()?);
    let count = array.header_field(m, HIGH_WATER).ok()?;
    (0..count).find_map(|index| {
        let elem = array.get_absolute(m, index).ok()??;
        if m.u16(elem + SETUP_ACTION).ok()? as i16 != action {
            return None;
        }
        Some(array.handle_of(m, index).ok()??.0)
    })
}

fn capture_setup(memory: &dyn guest_layout::GuestMemory, child: u32, request: u32) -> Option<SetupReturn> {
    let m = ReadGm(memory);
    let (source, ch) = DialogChannel::of(&m, child)?;
    if source.window != LOBBY_DIALOG
        || m.u16(request)? != 0
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
    let loader = m.u32(request + REQUEST_LOADER)?;
    let selector = is_setup_selector(m.u32(child + SCREEN_ID)?, loader)
        || crate::list_class::owns_loader(m.u32(child)?, loader);
    if mask & source.mask == 0 || !selector {
        return None;
    }
    let list = parent + crate::game_setup_help::SETUP_LIST;
    let action = crate::ui_kit::lifecycle::focused_row_handle(memory, ch, list)
        .and_then(|row| crate::h1_options_row::row_action(memory, list, row));
    Some(SetupReturn { mask, action })
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
        b.raw().before(&points::SCREEN_PUSH, "remember the replaced lobby selector", move |c| {
            let mut mem = c.ctx.runtime().memory();
            let request = c.cpu.ebx;
            if let Some(handle) = nav.pushed(&*mem, c.cpu.edi, request) {
                mem.set_u32(request + REQUEST_FOCUS, handle)?;
            }
            Ok(hooks::Flow::Continue)
        });
        let nav = Arc::clone(&self.0);
        b.raw().observe_before(&points::UI_CHANNEL_FREE, "forget the freed screen's opener", move |o| {
            if let Some(screen) = o.arg_u32("screen") {
                nav.freed(screen);
            }
        });
        let nav = Arc::clone(&self.0);
        b.raw()
            .before(&points::SCREEN_BACK_EVENT, "return to the replaced Game Setup", move |c| {
                let screen = c.cpu.ecx;
                let (mask, loader) = if let Some(category) = nav.category_parent(screen) {
                    (category.mask, points::LOBBY_VARIANT_LIST_LOAD.base)
                } else if let Some(setup) = nav.setup_parent(screen) {
                    (setup.mask, points::GAME_SETUP_LOAD.base)
                } else {
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
                            reopen(&cx, request_at, source, loader).await?;
                            Ok(Ret::new(1, 4))
                        }))
                    }
                    _ => Ok(hooks::Flow::Continue),
                }
            })
            .after_feature(crate::map_select::FEATURE);
        let nav = Arc::clone(&self.0);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            (!nav.game_setup().is_empty() || !nav.categories.lock().unwrap_or_else(|e| e.into_inner()).is_empty()).then(|| "a lobby selector has a Game Setup return".into())
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
        let mut mem = guest_layout::testing::FakeGuest::new();
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
        assert_eq!(capture_setup(&mem, child, request), Some(SetupReturn { mask: 1, action: None }));

        mem.put_u16(child + CONTROLLER_MASK, 2);
        assert_eq!(capture_setup(&mem, child, request), None);
        mem.put_u16(child + CONTROLLER_MASK, 1);
        mem.put_u32(channel + PENDING, 0x13000);
        assert_eq!(capture_setup(&mem, child, request), None);
        mem.put_u32(channel + PENDING, 0);
        mem.put_u8(setup + SCREEN_STATE, 2);
        assert_eq!(capture_setup(&mem, child, request), None);
    }

    #[test]
    fn lobby_variants_restore_the_category_focus_and_setup_return_across_reuse() {
        use guest_layout::{GuestMemory, datum::DatumArray, testing::FakeGuest};

        let nav = Navigation::default();
        let mut m = FakeGuest::new();
        let ch = channel(LOBBY_DIALOG, 4).unwrap();
        let (setup, category, variants, request) = (0x10000, 0x20000, 0x30000, 0x40000);
        let (item, array, name) = (0x50000, 0x60000, 0x70000);
        let list = category + crate::tweakers::SCREEN_LIST;
        m.map(array, 0x1000).map(name, 32)
            .put_u32(item + 0x10, list)
            .put_u32(list + crate::retail_rows::LIST_ROWS, array)
            .put_u32(ch + 0x34, item);
        for (screen, id) in [(setup, GAME_SETUP_ID), (category, CATEGORY_ID), (variants, VARIANTS_ID)] {
            m.put_u32(screen + SCREEN_WINDOW, LOBBY_DIALOG)
                .put_u32(screen + SCREEN_SLOT, 4)
                .put_u16(screen + CONTROLLER_MASK, 1)
                .put_u32(screen + SCREEN_ID, id);
        }
        let rows = DatumArray::at(array);
        rows.initialize_contiguous(&mut m, name, 8, 4, 0, 0).unwrap();
        rows.make_valid(&mut m).unwrap();
        let handles: Vec<_> = (0..4).map(|_| rows.new_datum(&mut m).unwrap().unwrap().0).collect();

        let setup_list = setup + crate::game_setup_help::SETUP_LIST;
        let setup_item = 0x81000;
        let setup_array = 0x82000;
        m.put_u32(setup_item + 0x10, setup_list)
            .put_u32(setup_list + crate::h1_options_row::SETUP_ROWS, setup_array)
            .map(setup_array, 0x1000);
        let setup_rows = DatumArray::at(setup_array);
        setup_rows.initialize_contiguous(&mut m, name, 4, 4, 0, 0).unwrap();
        setup_rows.make_valid(&mut m).unwrap();
        let first = setup_rows.new_datum(&mut m).unwrap().unwrap();
        let second = setup_rows.new_datum(&mut m).unwrap().unwrap();
        let a = setup_rows.try_get(&m, first).unwrap().unwrap() + crate::h1_options_row::SETUP_ACTION;
        let b = setup_rows.try_get(&m, second).unwrap().unwrap() + crate::h1_options_row::SETUP_ACTION;
        let saved = Some(SetupReturn { mask: 1, action: Some(9) });

        for index in [3, 0, 2, 1, 3] {
            m.put_u16(a, 9).put_u16(b, 4)
                .put_u32(setup_item + 0x70, first.0)
                .put_u32(ch + 0x34, setup_item)
                .put_u32(ch + ACTIVE, setup)
                .put_u16(request, 0)
                .put_u32(request + REQUEST_LOADER, points::LOBBY_VARIANT_LIST_LOAD.base);
            assert_eq!(nav.pushed(&m, category, request), None);
            nav.freed(setup);
            m.put_u32(ch + 0x34, item)
                .put_u32(ch + ACTIVE, category)
                .put_u32(item + 0x70, 0x1234_0000 | index as u32);
            assert_eq!(nav.pushed(&m, variants, request), None);
            nav.freed(category);
            m.put_u32(ch + ACTIVE, variants);
            assert_eq!(nav.category_parent(variants).unwrap().setup, saved);
            assert_eq!(back(&ReadGm(&m), variants, Some(Input::Back)).0, Back::Reopen);
            m.put_u16(request, 1);
            assert_eq!(nav.pushed(&m, category, request), Some(handles[index]));
            nav.freed(variants);
            m.put_u32(ch + ACTIVE, category);
            assert_eq!(nav.setup_parent(category), saved);
            assert_eq!(back(&ReadGm(&m), category, Some(Input::B)).0, Back::Reopen);
            m.put_u32(request + REQUEST_LOADER, points::GAME_SETUP_LOAD.base)
                .put_u16(a, 4).put_u16(b, 9);
            assert_eq!(nav.pushed(&m, setup, request), Some(second.0), "focus follows the action");
            nav.freed(category);
            assert!(nav.setup_parent(category).is_none());
            assert!(nav.category_parent(variants).is_none());
            assert_eq!(m.u32(list + crate::retail_rows::LIST_ROWS), Ok(array));
        }
    }

    #[test]
    fn kit_options_replacing_setup_return_on_the_same_channel() {
        use crate::{firefight, list_class::ListClass, variant_quick::page};
        let store = Arc::new(settings::Store::defaults(crate::setting_defs::ALL));
        let quick = page::page(&Arc::new(page::Dialog::new(Arc::default())));
        let pages = [
            ("test-lobby-firefight", firefight::options::page(store)),
            ("test-lobby-quick", quick),
        ];
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        let pages = pages.map(|(name, page)| {
            let class = ListClass::with_rows(name, name, Arc::new(page));
            registry.add(&class).unwrap();
            (name, class.loader())
        });
        let plan = registry.build(&hooks::Base).unwrap();
        let nav = Navigation::default();
        let mut m = guest_layout::testing::FakeGuest::new();
        let ch = channel(LOBBY_DIALOG, 4).unwrap();
        let (setup, page, request) = (0x10000, 0x20000, 0x30000);
        m.put_u32(page + SCREEN_WINDOW, LOBBY_DIALOG)
            .put_u32(page + SCREEN_SLOT, 4).put_u16(page + CONTROLLER_MASK, 1)
            .put_u32(setup + SCREEN_ID, GAME_SETUP_ID).put_u16(setup + CONTROLLER_MASK, 1);
        for (name, loader) in pages.into_iter().chain(pages.into_iter().rev()) {
            let tables = plan.allocs.iter().find(|alloc| alloc.feature == name).unwrap().addr;
            m.put_u32(ch + ACTIVE, setup).put_u32(page, tables)
                .put_u32(request + REQUEST_LOADER, loader);
            nav.pushed(&m, page, request);
            nav.freed(setup);
            m.put_u32(ch + ACTIVE, page);
            assert_eq!(nav.setup_parent(page), Some(SetupReturn { mask: 1, action: None }));
            assert_eq!(back(&ReadGm(&m), page, Some(Input::B)).0, Back::Reopen);
            nav.freed(page);
            assert!(nav.setup_parent(page).is_none());
        }
    }

    #[test]
    fn back_returns_to_setup_by_default_and_accepts_the_off_switch() {
        for (switches, enabled) in [("", true), ("-ui-back", false)] {
            let mut registry = hooks::Registry::new(hooks::Switches::parse(switches).unwrap());
            assert_eq!(registry.add(&RetailBack(Arc::default())).unwrap(), enabled);
        }
    }

}
