//! The hooks and the picker screen of [`crate::campaigns`]
//! (docs/halo1/campaign-picker.md "What we do").
//!
//! - [`Campaigns`], a feature on the plan: the lists ([`campaigns::extend`])
//!   before each of their readers we hook runs; the campaign the level
//!   select shows; the campaign of the level started; our campaigns'
//!   progress kept apart from the profile's.
//! - The game picker uses our list class ([`crate::list_class`]).
//!   The main menu opens the picker, then the chosen game’s
//!   Campaign Options, with the native checkpoint and co-op handlers.
//!
//! [`prepare`] runs before the layouts install (which serve the picker's
//! strings and register its class) and [`install`] after.

mod options;
mod inspect;
pub use inspect::inspect;
pub use options::options_capacity;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use guest_layout::points::{
    CAMPAIGN_LEVEL_LIST, CAMPAIGN_LEVEL_LOOKUP, CAMPAIGN_START, LAST_LEVEL_SET, LEVEL_SELECT_NEW, NEXT_LEVEL,
    PROGRESS_MAX, PROGRESS_RAISE, SCREEN_REQUEST,
};
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::guest::Permissions;

use crate::campaign_cards::picker as picker_art;
use crate::campaigns::{
    self, LEVEL_ARRAY, Picker, RETAIL_CAMPAIGN, Region, SELECTED_CAMPAIGN, State,
};
use crate::layout_glue::Gm;
use crate::list_class::{ListClass, LiveText, Rows};
use crate::ui::{Chosen, GuestArg};

/// The picker's screen id.
pub const SCREEN: &str = "campaigns";
/// The main menu's level-select loader (flag 0; the lobby's is
/// `0x2b136d`).
pub const LEVEL_SELECT_LOADER: u32 = 0x002b_130a;
/// Main menu row zero calls 0x230888. Its normal branch requests
/// Campaign Options on window 3 before consulting the profile.
pub const OPTIONS_LOADER: u32 = 0x0023_0616;
pub const MAIN_MENU_REQUEST_RET: u32 = 0x0023_08d1;
/// OPEN_SCREEN returns here after building its loader request.
const OPEN_SCREEN_REQUEST_RET: u32 = 0x0014_9f3f;
/// Our strings: the title, then one label a row.
pub const ID_BASE: u32 = crate::ui_kit::ids::family("campaign picker").base;
pub const TITLE_ID: u32 = ID_BASE;
const TITLE: &str = "Campaign";
/// The most campaigns the picker lists (Halo 2 and 63 more).
pub const MAX_ROWS: usize = 64;
/// Each row's detail text ([`crate::campaign_info`]), the description
/// pane's, rewritten in place while the picker is up.
pub const INFO_IDS: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("campaign picker info");
/// The room each detail text has in guest memory, its NUL included: its
/// placeholder's length at install.
pub const INFO_ROOM: usize = 384;

/// The picker's layout: the level select's (`skulls_menu::LIST_LAYOUT`,
/// its list on the left), its bracket, its picture box with the focused
/// game's artwork, and its description text under it
/// with the campaign's detail. The level select's own picture bitmap
/// (screen 11, pane 0, bitmap 0, `[177 39 -30 259]`, 220 by 207, its
/// animation 13), copied and pointed at the campaign's picture
/// (`Rows::picture`): the picture keeps the frame the level pictures carry
/// in their pixels (`campaign_cards::framed`).
pub const PICTURE: crate::list_class::PictureSpec = crate::list_class::PictureSpec {
    screen_id: crate::skulls_menu::LEVEL_SELECT_SCREEN_ID as u32,
    pane: 0,
    bitmap: 0,
    image: 0,
    at: [
        crate::skulls_menu::PICTURE_BOX[1],
        crate::skulls_menu::PICTURE_BOX[0],
    ],
    animation: crate::skulls_menu::PICTURE_ANIMATION,
};
/// [`PICTURE`]'s size (the level pictures').
pub const PICTURE_SIZE: [u16; 2] = picker_art::SIZE;

static PREPARED: OnceLock<Arc<State>> = OnceLock::new();
static STORE: OnceLock<Arc<settings::Store>> = OnceLock::new();
static STRINGS: OnceLock<Vec<(u32, &'static str)>> = OnceLock::new();

/// Reads the installed campaigns and builds their records against the
/// disc's lists. Returns the state when there is at least one, after
/// which [`strings`] and [`class`] answer. Retail ids come from the disc
/// (`retail`: campaign ids, map ids), so ours never meet them. `store`
/// holds the Halo 1 options the detail pane shows.
pub fn prepare(state: State, store: Option<Arc<settings::Store>>) -> Arc<State> {
    let state = Arc::new(state);
    let picker = Picker::new(&state.built);
    let mut strings: Vec<(u32, &'static str)> = vec![(TITLE_ID, TITLE)];
    strings.extend(options::STRINGS);
    // Once a boot: the labels live as long as the game.
    let placeholder: &'static str = Box::leak(" ".repeat(INFO_ROOM - 1).into_boxed_str());
    for (i, (_, name)) in picker.rows.iter().enumerate().take(MAX_ROWS) {
        strings.push((
            ID_BASE + 1 + i as u32,
            Box::leak(name.clone().into_boxed_str()),
        ));
        if (i as u32) < INFO_IDS.len {
            strings.push((INFO_IDS.id(i as u32), placeholder));
        }
    }
    let _ = STRINGS.set(strings);
    let _ = PREPARED.set(Arc::clone(&state));
    if let Some(store) = store {
        let _ = STORE.set(store);
    }
    state
}

/// The key of the installed campaign map id `map_id` belongs to (`halo1`),
/// once [`prepare`]d; `None` for Halo 2's own levels.
pub fn campaign_key_of(map_id: u32) -> Option<String> {
    PREPARED.get()?.key_of(map_id).map(str::to_owned)
}

/// The picker's strings, once [`prepare`]d.
pub fn strings() -> Vec<(u32, &'static str)> {
    STRINGS.get().cloned().unwrap_or_default()
}

/// The picker's class, once [`prepare`]d.
pub fn class() -> Option<ListClass> {
    let state = PREPARED.get()?;
    Some(picker_class(Arc::new(PickerRows::new(
        Arc::clone(state),
        STORE.get().cloned(),
    ))))
}

/// The picker's class over `rows`: the level select's list, bracket,
/// picture and description ([`PICTURE`]).
pub fn picker_class(rows: Arc<PickerRows>) -> ListClass {
    crate::skulls_menu::level_select_list(
        ListClass::with_rows("campaigns-screen", SCREEN, rows),
        crate::skulls_menu::LIST_LAYOUT,
    )
    .description(crate::skulls_menu::DESCRIPTION_TEXT)
    .picture(PICTURE)
}

/// The picker's rows.
pub struct PickerRows {
    pub picker: Picker,
    pub state: Arc<State>,
    /// The settings (the Halo 1 options); `None` shows none.
    pub store: Option<Arc<settings::Store>>,
    /// The detail text each row's string holds now, so only a change is
    /// written.
    written: std::sync::Mutex<Vec<String>>,
}

impl PickerRows {
    pub fn new(state: Arc<State>, store: Option<Arc<settings::Store>>) -> Self {
        PickerRows {
            picker: Picker::new(&state.built),
            state,
            store,
            written: Default::default(),
        }
    }
    /// Each row's detail, for the profiles seen as the picker opened.
    pub fn infos(&self) -> Vec<crate::campaign_info::Info> {
        let seen = self.state.selection.seen();
        let options = || {
            self.store
                .as_deref()
                .map(crate::campaign_info::halo1_options)
                .unwrap_or_default()
        };
        crate::campaign_info::infos(
            &self.state.built,
            &self.picker.rows,
            &self.state.progress,
            &seen.profiles,
            &options,
        )
    }
}

/// The bitmap datum selected for this campaign's row.
fn card_picture(state: &State, id: u32) -> Option<u32> {
    let halo1 = state.key_of_campaign(id) == Some(campaigns::HALO1_KEY);
    let tile = if halo1 {
        Some(picker_art::CE)
    } else if id == RETAIL_CAMPAIGN {
        Some(picker_art::TWO)
    } else {
        None
    };
    if let Some(tile) = tile {
        let cards = state.cards.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&datum) = cards.get(tile) {
            return Some(datum);
        }
    }
    // Halo 1's level list may carry a generic retail level for an
    // unillustrated mission. The picker keeps its artwork area empty.
    if halo1 {
        return None;
    }
    state.selection.seen().pictures.get(&id).copied()
}

/// `text` as the string table serves it (NUL-terminated UTF-8), cut at a
/// character boundary to [`INFO_ROOM`].
pub fn served(text: &str) -> Vec<u8> {
    let mut cut = text.len().min(INFO_ROOM - 1);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut b = text.as_bytes()[..cut].to_vec();
    b.push(0);
    b
}

impl Rows for PickerRows {
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(crate::ui_kit::spec::Describe::spec(self))
    }

    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        self.picker.rows.len().min(MAX_ROWS)
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match usize::try_from(row) {
            Ok(r) if r < self.count() => vec![(0, ID_BASE + 1 + r as u32)],
            _ => Vec::new(),
        }
    }
    fn description(&self, row: i16) -> Option<u32> {
        let r = u32::try_from(row).ok()?;
        (r < INFO_IDS.len && (r as usize) < self.count()).then(|| INFO_IDS.id(r))
    }
    fn picture(&self, row: i16) -> Option<u32> {
        let id = self.picker.campaign(row)?;
        // A valid row with no art clears the previous row's picture.
        Some(card_picture(&self.state, id).unwrap_or(u32::MAX))
    }
    fn live_text(&self) -> LiveText {
        let texts: Vec<String> = self.infos().iter().map(|i| i.text()).collect();
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        let mut live = LiveText::default();
        for (row, text) in texts.iter().enumerate().take(INFO_IDS.len as usize) {
            if written.get(row) == Some(text) {
                continue;
            }
            let Some(at) = crate::layout_glue::string_address(INFO_IDS.id(row as u32)) else {
                continue;
            };
            live.writes.push((at, served(text)));
            live.redraw = true;
        }
        *written = texts;
        live
    }
    fn press(&self, row: i16) -> Chosen {
        let Some(id) = self.picker.campaign(row) else {
            return Chosen::Nothing;
        };
        self.state.selection.set(id);
        *self.state.selection.pick.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(campaigns::Pick { campaign: id, request: None });
        Chosen::Retail {
            line: format!(
                "campaigns: picked campaign {id} ({})",
                self.picker.rows[row as usize].1
            ),
            // The level select's list for that campaign, as `0x19c190`
            // builds it for campaign 1.
            calls: vec![(
                CAMPAIGN_LEVEL_LIST.base,
                vec![GuestArg::Load(LEVEL_ARRAY), GuestArg::Value(id)],
            )],
            loader: OPTIONS_LOADER,
        }
    }
}

impl crate::ui_kit::spec::Describe for PickerRows {
    /// The picker as the kit sees it: the level select's list on the left,
    /// its bracket, the picture box with the focused campaign's picture
    /// ([`PICTURE`]), and each row's detail ([`crate::campaign_info`]) as
    /// its description.
    fn spec(&self) -> crate::ui_kit::spec::ScreenSpec {
        use crate::ui_kit::input::Input;
        use crate::ui_kit::spec::{ListPlace, Press, RowSpec, ScreenSpec, ValueStyle};
        use crate::ui_kit::view::{Art, PaneArt};
        let mut s = ScreenSpec::new(SCREEN, TITLE);
        s.ids = vec![TITLE_ID];
        s.list = ListPlace::from(Some(crate::skulls_menu::LIST_LAYOUT));
        s.style = ValueStyle::Inline;
        s.description = Some(crate::skulls_menu::DESCRIPTION_TEXT.into());
        s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B];
        let art = |p: crate::list_class::PictureSpec, size: [u16; 2]| PaneArt {
            art: Art::Screen {
                screen_id: p.screen_id as i16,
                pane: p.pane as u16,
                index: p.bitmap as u16,
                image: p.image,
            },
            at: p.at,
            size,
            depth: 0,
        };
        s.art = vec![
            art(crate::skulls_menu::FRAME, crate::skulls_menu::FRAME_SIZE),
            art(PICTURE, PICTURE_SIZE),
        ];
        for (row, info) in self.infos().iter().enumerate().take(self.count()) {
            let r = row as i16;
            let mut ids: Vec<u32> = self.texts(r).into_iter().map(|(_, id)| id).collect();
            ids.extend(self.description(r));
            s.rows.push(
                RowSpec::action(&self.picker.rows[row].1)
                    .press(Press::Open("Campaign Options".into()))
                    .describe(&info.text())
                    .ids(&ids),
            );
        }
        s
    }
}

/// The hooks.
pub struct Campaigns {
    pub state: Arc<State>,
    /// The picker's loader, once its class is built (0: the level select
    /// opens as retail).
    pub picker: Arc<dyn Fn() -> u32 + Send + Sync>,
}

/// Our list memory's address, once mapped ([`install`]).
static REGION: AtomicU32 = AtomicU32::new(0);

fn ensure(c: &mut Call, state: &State) {
    let base = REGION.load(Ordering::SeqCst);
    if base == 0 {
        return;
    }
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let cards = state
        .cards
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    match campaigns::extend_with_cards(
        &mut *mem,
        &state.built,
        &Region {
            base,
            len: Region::LEN,
        },
        &cards,
    ) {
        Ok(r) if r.already || r.no_lists => {}
        Ok(r) => eprintln!("{}", r.line()),
        Err(e) => eprintln!("campaigns: the level lists were not extended: {e}"),
    }
}

fn stack(c: &Call, n: u32) -> Option<u32> {
    let rt = Arc::clone(c.ctx.runtime());
    let mem = rt.memory();
    Gm::u32(&*mem, c.cpu.esp + 4 + 4 * n)
}

fn set_stack(c: &Call, n: u32, v: u32) -> bool {
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    Gm::set_u32(&mut *mem, c.cpu.esp + 4 + 4 * n, v)
}

impl hooks::Feature for Campaigns {
    fn manifest(&self) -> Manifest {
        Manifest::new(campaigns::FEATURE, "More than one campaign in the menus")
            .doc("docs/halo1/campaign-picker.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let st = &self.state;

        let s = Arc::clone(st);
        b.raw().before(
            &CAMPAIGN_LEVEL_LIST,
            "the shown campaign's list",
            move |c| {
                ensure(c, &s);
                let want = s.selection.campaign();
                if stack(c, 1) != Some(want) && set_stack(c, 1, want) {
                    eprintln!("campaigns: the level select lists campaign {want}");
                }
                Ok(Flow::Continue)
            },
        );
        let s = Arc::clone(st);
        b.raw()
            .before(&LEVEL_SELECT_NEW, "note the build", move |c| {
                ensure(c, &s);
                s.selection.set_building(true);
                Ok(Flow::Continue)
            });
        let s = Arc::clone(st);
        b.raw()
            .after(&LEVEL_SELECT_NEW, "the shown campaign", move |c| {
                s.selection.set_building(false);
                let want = s.selection.campaign();
                if want != RETAIL_CAMPAIGN {
                    let rt = Arc::clone(c.ctx.runtime());
                    Gm::set_u32(&mut *rt.memory(), SELECTED_CAMPAIGN, want);
                }
                Ok(())
            });
        let s = Arc::clone(st);
        b.raw()
            .after(&PROGRESS_MAX, "our campaign's progress", move |c| {
                if !s.selection.building() {
                    return Ok(());
                }
                let Some(key) = s.key_of_campaign(s.selection.campaign()) else {
                    return Ok(());
                };
                let profiles = crate::skulls::profiles_or_default(&*c.ctx.runtime().memory()).0;
                c.cpu.eax = s.progress.max(&profiles, key) as u32;
                Ok(())
            });
        let s = Arc::clone(st);
        b.raw()
            .before(&CAMPAIGN_START, "the level's campaign", move |c| {
                ensure(c, &s);
                let rt = Arc::clone(c.ctx.runtime());
                if let Some(id) = s.fix_selection(&mut *rt.memory()) {
                    eprintln!("campaigns: starting a level of campaign {id}");
                }
                Ok(Flow::Continue)
            });
        // Both take `(campaign, map)`; some callers pass campaign 1 for
        // any map, which finds nothing for ours (the level select's press
        // faulted on it).
        for (point, what) in [
            (&NEXT_LEVEL, "the lists, the map's campaign"),
            (&CAMPAIGN_LEVEL_LOOKUP, "the lists, the map's campaign"),
        ] {
            let s = Arc::clone(st);
            b.raw().before(point, what, move |c| {
                ensure(c, &s);
                if let (Some(campaign), Some(map)) = (stack(c, 0), stack(c, 1))
                    && let Some(want) = s.lookup_campaign(campaign, map)
                {
                    set_stack(c, 0, want);
                }
                Ok(Flow::Continue)
            });
        }
        let s = Arc::clone(st);
        b.raw()
            .before(&PROGRESS_RAISE, "our campaign's progress", move |c| {
                let map = c.cpu.esi;
                let Some(key) = s.key_of(map) else {
                    return Ok(Flow::Continue);
                };
                let (slot, difficulty) =
                    (stack(c, 0).unwrap_or(u32::MAX), stack(c, 1).unwrap_or(0));
                let profile = {
                    let mem = c.ctx.runtime().memory();
                    let named = crate::skulls::slot_name(&*mem, slot);
                    let any = !crate::skulls::signed_in(&*mem).is_empty();
                    named.or_else(|| {
                        (!any && slot == 0).then(|| crate::skulls::DEFAULT_PROFILE.to_owned())
                    })
                };
                if let Some(p) = profile
                    && s.progress.finished(&p, key, difficulty, map)
                {
                    eprintln!(
                        "campaigns: {p} finished {key} level {map:#x} on difficulty {difficulty}"
                    );
                }
                // The profile keeps Halo 2's progress only.
                c.ret(8).map(Flow::Return)
            });
        let s = Arc::clone(st);
        b.raw()
            .before(&LAST_LEVEL_SET, "our campaign's last level", move |c| {
                let map = c.cpu.eax;
                let launch = c.ctx.runtime().memory().u32(c.cpu.esp)? == 0x0018_f0b7;
                let game = campaigns::campaign_of(&s.built, map);
                if launch {
                    s.selection.set(game.map_or(RETAIL_CAMPAIGN, |g| g.campaign_id));
                }
                let Some(game) = game else { return Ok(Flow::Continue) };
                if launch {
                    let level = game.levels.iter().find(|l| l.map_id == map).unwrap();
                    // 0x18f0a0 keeps the confirmed launch options in ESI.
                    let (profiles, difficulty, restore) = {
                        let mem = c.ctx.runtime().memory();
                        (crate::skulls::profiles_or_default(&*mem).0,
                         mem.u16(c.cpu.esi + crate::level_boot::OPT_DIFFICULTY)?,
                         mem.u8(options::RESTORE_CHECKPOINT)? != 0)
                    };
                    for p in profiles {
                        s.progress.launched(&p, &game.key, level, difficulty, restore);
                    }
                }
                c.ret(0).map(Flow::Return)
            });
        options::hooks(b, st);
        let picker = Arc::clone(&self.picker);
        let s = Arc::clone(st);
        b.raw()
            .before(&SCREEN_REQUEST, "the campaign picker first", move |c| {
                let ret = {
                    let rt = Arc::clone(c.ctx.runtime());
                    Gm::u32(&*rt.memory(), c.cpu.esp).unwrap_or(0)
                };
                // The native checkpoint completion copies the stored map id and
                // writes campaign 1. Resolve the saved basename again, since a
                // partial import may now have more installed missions.
                if ret == 0x002b_180c && s.selection.campaign() != RETAIL_CAMPAIGN {
                    options::checkpoint_selection(c, &s)?;
                }
                if ret == OPEN_SCREEN_REQUEST_RET {
                    let mem = c.ctx.runtime().memory();
                    inspect::observe_options_request(&*mem, &s, c.cpu.esp);
                }
                if ret != MAIN_MENU_REQUEST_RET || stack(c, 3) != Some(OPTIONS_LOADER) {
                    return Ok(Flow::Continue);
                }
                let loader = picker();
                if loader != 0 {
                    ensure(c, &s);
                    // What the picker's detail and pictures read while it
                    // is up.
                    let rt = Arc::clone(c.ctx.runtime());
                    let mem = rt.memory();
                    s.selection.set_seen(campaigns::Seen {
                        profiles: crate::skulls::profiles_or_default(&*mem).0,
                        pictures: campaigns::first_pictures(&*mem),
                    });
                }
                if loader != 0 && set_stack(c, 1, 5) && set_stack(c, 3, loader) {
                    eprintln!("campaigns: opening the game picker");
                }
                Ok(Flow::Continue)
            });
        // Progress goes to the profile files on the host disk, like a save
        // game, outside the snapshot.
        let s = Arc::clone(st);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || s.selection.snapshot_live()));
        Ok(())
    }
}

/// Adds the feature when at least one campaign is installed. Returns
/// whether it was added.
pub fn install(
    rt: &runtime::Runtime,
    registry: &mut hooks::Registry,
    state: Option<Arc<State>>,
) -> anyhow::Result<bool> {
    let Some(state) = state else {
        return Ok(false);
    };
    rt.memory()
        .map_zeroed(Region::BASE, Region::LEN, Permissions::RW)?;
    let picker: Arc<dyn Fn() -> u32 + Send + Sync> =
        Arc::new(|| crate::list_class::loader_of(SCREEN).unwrap_or(0));
    let added = registry
        .add(&Campaigns { state, picker })
        .map_err(|e| anyhow::anyhow!("campaigns: {e}"))?;
    REGION.store(if added { Region::BASE } else { 0 }, Ordering::SeqCst);
    if !added {
        eprintln!("campaigns: features.campaigns is off; one campaign, as retail");
    }
    Ok(added)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::campaigns::{Progress, Selection};
    use content::campaign_list::{self as cl, CampaignManifest, FIXTURE, HALO1};

    pub fn state() -> State {
        let ms = [
            CampaignManifest::parse(HALO1).unwrap(),
            CampaignManifest::parse(FIXTURE).unwrap(),
        ];
        State {
            built: cl::build(&ms, &[1], &[0x65]).unwrap(),
            selection: Selection::default(),
            progress: Progress::memory(),
            cards: Default::default(),
        }
    }

    #[test]
    fn choosing_a_game_opens_its_options_with_its_level_list() {
        let st = Arc::new(state());
        let rows = PickerRows::new(Arc::clone(&st), None);
        assert_eq!(rows.count(), 3);
        assert_eq!(rows.title(), TITLE_ID);
        assert_eq!(rows.texts(1), [(0, ID_BASE + 2)]);
        assert!(rows.texts(3).is_empty());
        // Halo 1 first, Halo 2 second.
        match rows.press(0) {
            Chosen::Retail {
                calls,
                loader,
                line,
            } => {
                assert_eq!(loader, OPTIONS_LOADER);
                assert_eq!(
                    calls,
                    [(
                        0x0019_c6d0,
                        vec![GuestArg::Load(LEVEL_ARRAY), GuestArg::Value(2)]
                    )]
                );
                assert!(line.contains("Halo: Combat Evolved"), "{line}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(st.selection.campaign(), 2);
        // Halo 2 again: campaign 1, the retail list.
        assert!(
            matches!(rows.press(1), Chosen::Retail { ref calls, .. } if calls[0].1[1] == GuestArg::Value(1))
        );
        assert_eq!(st.selection.campaign(), 1);
        assert_eq!(rows.press(9), Chosen::Nothing);
    }

    #[test]
    fn each_row_describes_its_campaign_and_shows_its_picture() {
        let st = Arc::new(state());
        st.progress.finished("Chief", "halo1", 2, 0x1003);
        let rows = PickerRows::new(Arc::clone(&st), None);
        assert_eq!(rows.description(0), Some(INFO_IDS.id(0)));
        assert_eq!(rows.description(2), Some(INFO_IDS.id(2)));
        assert_eq!(rows.description(3), None);
        assert_eq!(rows.picture(0), Some(u32::MAX), "no Halo 1 artwork");
        assert_eq!(rows.picture(1), Some(u32::MAX), "no Halo 2 artwork");
        st.selection.set_seen(campaigns::Seen {
            profiles: vec!["Chief".into()],
            pictures: [(1, 0xe6dd_04bb), (2, 0xe7a0_0010)].into(),
        });
        assert_eq!(rows.picture(0), Some(u32::MAX), "skip generic level art");
        assert_eq!(rows.picture(1), Some(0xe6dd_04bb), "retail level fallback");
        st.cards.lock().unwrap().extend([
            (picker_art::CE.into(), 0xe7ab_0020),
            (picker_art::TWO.into(), 0xe7ab_0021),
        ]);
        assert_eq!(rows.picture(0), Some(0xe7ab_0020), "Halo 1 artwork");
        assert_eq!(rows.picture(1), Some(0xe7ab_0021), "Halo 2 artwork");
        assert_eq!(rows.picture(2), Some(u32::MAX), "clear Halo 2's artwork");
        let custom = st.built[1].campaign_id;
        let mut seen = st.selection.seen();
        seen.pictures.insert(custom, 0xe7ab_0022);
        st.selection.set_seen(seen);
        assert_eq!(rows.picture(2), Some(0xe7ab_0022), "custom level artwork");
        assert_eq!(rows.picture(3), None, "outside the list");
        let infos = rows.infos();
        assert_eq!(infos[0].progress.unwrap().reached, 4, "Chief finished a50");
        assert!(infos[1].text().starts_with(crate::campaign_info::HALO2_DESCRIPTION));
        // Without the installed strings there is nowhere to write.
        assert!(rows.live_text().writes.is_empty());
        // A text is cut to its room at a character boundary.
        let long = "é".repeat(INFO_ROOM);
        let b = served(&long);
        assert!(b.len() <= INFO_ROOM && b.ends_with(&[0]));
        assert!(std::str::from_utf8(&b[..b.len() - 1]).is_ok());
    }

    #[test]
    fn served_campaign_cards_have_spaces_between_sentences() {
        let st = state();
        let halo1 = crate::campaign_info::Info::ours(
            &st.built[0],
            &[],
            vec![("Assets", true), ("HUD", true)],
        );
        assert_eq!(
            served(&halo1.text()).as_slice(),
            b"An ancient ring, and the secret it was built to keep. 2001, Bungie. \
              10 levels. Reached 1 of 10. Assets: Halo 1. HUD: Halo 1.\0"
        );
        let rows = PickerRows::new(Arc::new(st), None);
        for info in rows.infos() {
            let bytes = served(&info.text());
            let text = std::str::from_utf8(bytes.strip_suffix(&[0]).unwrap()).unwrap();
            assert!(!text.contains(['\r', '\n']), "{text:?}");
            assert!(
                !text
                    .chars()
                    .zip(text.chars().skip(1))
                    .any(|(a, b)| a == '.' && b.is_alphabetic()),
                "{text}"
            );
        }
    }

    /// The picker's spec: the level select's layout and art, each row's
    /// detail as its description, every id in a family.
    #[test]
    fn the_picker_is_laid_out_as_the_level_select() {
        use crate::ui_kit::spec::Describe;
        let rows = PickerRows::new(Arc::new(state()), None);
        let s = rows.spec();
        assert_eq!(s.rows.len(), 3);
        assert_eq!(s.rows[0].label, "Halo: Combat Evolved");
        assert!(s.rows[0].description.as_deref().unwrap().contains("2001, Bungie. 10 levels."));
        assert_eq!(s.art.len(), 2);
        assert_eq!(s.art[1].at, [39, 177]);
        let f = crate::ui_kit::spec::check_fallback(&s);
        assert!(
            f.iter().all(|f| matches!(f, crate::ui_kit::spec::Finding::Wide { .. }
                | crate::ui_kit::spec::Finding::Description { .. })),
            "{f:#?}"
        );
    }

    #[test]
    fn the_hooks_are_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let picker: Arc<dyn Fn() -> u32 + Send + Sync> = Arc::new(|| 0);
        r.add(&Campaigns {
            state: Arc::new(state()),
            picker,
        })
        .unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        for line in [
            "0x0019c6d0 CAMPAIGN_LEVEL_LIST: before [campaigns:the shown campaign's list] -> original",
            "0x0019c270 CAMPAIGN_LEVEL_LOOKUP: before [campaigns:the lists, the map's campaign] -> original",
            "0x0019c440 NEXT_LEVEL: before [campaigns:the lists, the map's campaign] -> original",
            "0x0023643f CAMPAIGN_START: before [campaigns:the level's campaign] -> original",
            "0x0019043f PROGRESS_RAISE: before [campaigns:our campaign's progress] -> original",
            "0x0019040d LAST_LEVEL_SET: before [campaigns:our campaign's last level] -> original",
            "0x00149f49 SCREEN_REQUEST: before [campaigns:the campaign picker first] -> original",
            "0x00190565 PROGRESS_MAX: original -> after [campaigns:our campaign's progress]",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
    }
}
