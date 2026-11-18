//! The hooks and the picker screen of [`crate::campaigns`]
//! (docs/game/multiple-campaigns.md "What we do").
//!
//! - [`Campaigns`], a feature on the plan: the lists ([`campaigns::extend`])
//!   before each of their readers we hook runs; the campaign the level
//!   select shows; the campaign of the level started; our campaigns'
//!   progress kept apart from the profile's.
//! - The picker, our own list class ([`crate::list_class`]): Halo 2, then
//!   each installed campaign. The main menu's Campaign path opens it in
//!   place of the level select; a press picks the campaign, rebuilds the
//!   level select's list for it with the game's own `0x19c6d0`, and opens
//!   the retail level select.
//!
//! [`prepare`] runs before the layouts install (which serve the picker's
//! strings and register its class) and [`install`] after.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use engine::points::{
    CAMPAIGN_LEVEL_LIST, CAMPAIGN_LEVEL_LOOKUP, CAMPAIGN_START, LAST_LEVEL_SET, LEVEL_SELECT_NEW, NEXT_LEVEL,
    PROGRESS_MAX, PROGRESS_RAISE, SCREEN_REQUEST,
};
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::guest::Permissions;

use crate::campaigns::{
    self, LEVEL_ARRAY, Picker, RETAIL_CAMPAIGN, Region, SELECTED_CAMPAIGN, State,
};
use crate::layout_glue::Gm;
use crate::list_class::{ListClass, Rows};
use crate::ui::{Chosen, GuestArg};

/// The picker's screen id.
pub const SCREEN: &str = "campaigns";
/// The main menu's level-select loader (flag 0; the lobby's is
/// `0x2b136d`).
pub const LEVEL_SELECT_LOADER: u32 = 0x002b_130a;
/// Where the main menu's Campaign path fills its request for that loader
/// (the return address of its call to `0x149f49`).
pub const MAIN_MENU_REQUEST_RET: u32 = 0x002b_16f3;
/// Our strings: the title, then one label a row.
pub const ID_BASE: u32 = crate::ui_kit::ids::family("campaign picker").base;
pub const TITLE_ID: u32 = ID_BASE;
const TITLE: &str = "Campaign";
/// The most campaigns the picker lists (Halo 2 and 63 more).
pub const MAX_ROWS: usize = 64;

static PREPARED: OnceLock<Arc<State>> = OnceLock::new();
static STRINGS: OnceLock<Vec<(u32, &'static str)>> = OnceLock::new();

/// Reads the installed campaigns and builds their records against the
/// disc's lists. Returns the state when there is at least one, after
/// which [`strings`] and [`class`] answer. Retail ids come from the disc
/// (`retail`: campaign ids, map ids), so ours never meet them.
pub fn prepare(state: State) -> Arc<State> {
    let state = Arc::new(state);
    let picker = Picker::new(&state.built);
    let mut strings: Vec<(u32, &'static str)> = vec![(TITLE_ID, TITLE)];
    for (i, (_, name)) in picker.rows.iter().enumerate().take(MAX_ROWS) {
        // Once a boot: the labels live as long as the game.
        strings.push((
            ID_BASE + 1 + i as u32,
            Box::leak(name.clone().into_boxed_str()),
        ));
    }
    let _ = STRINGS.set(strings);
    let _ = PREPARED.set(Arc::clone(&state));
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
    Some(ListClass::with_rows(
        "campaigns-screen",
        SCREEN,
        Arc::new(PickerRows {
            picker: Picker::new(&state.built),
            state: Arc::clone(state),
        }),
    ))
}

/// The picker's rows.
pub struct PickerRows {
    pub picker: Picker,
    pub state: Arc<State>,
}

impl Rows for PickerRows {
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
    fn press(&self, row: i16) -> Chosen {
        let Some(id) = self.picker.campaign(row) else {
            return Chosen::Nothing;
        };
        self.state.selection.set(id);
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
            loader: LEVEL_SELECT_LOADER,
        }
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

impl feature::Feature for Campaigns {
    fn manifest(&self) -> Manifest {
        Manifest::new(campaigns::FEATURE, "More than one campaign in the menus")
            .doc("docs/game/multiple-campaigns.md")
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
                let Some(key) = s.key_of(map) else {
                    return Ok(Flow::Continue);
                };
                let profiles = crate::skulls::profiles_or_default(&*c.ctx.runtime().memory()).0;
                for p in &profiles {
                    s.progress.started(p, key, map);
                }
                c.ret(0).map(Flow::Return)
            });
        let picker = Arc::clone(&self.picker);
        b.raw()
            .before(&SCREEN_REQUEST, "the campaign picker first", move |c| {
                let ret = {
                    let rt = Arc::clone(c.ctx.runtime());
                    Gm::u32(&*rt.memory(), c.cpu.esp).unwrap_or(0)
                };
                if ret != MAIN_MENU_REQUEST_RET || stack(c, 3) != Some(LEVEL_SELECT_LOADER) {
                    return Ok(Flow::Continue);
                }
                let loader = picker();
                if loader != 0 && set_stack(c, 3, loader) {
                    eprintln!("campaigns: opening the campaign picker before the level select");
                }
                Ok(Flow::Continue)
            });
        // Progress goes to the profile files on the host disk, like a save
        // game, outside the snapshot.
        let s = Arc::clone(st);
        b.snapshot(feature::SnapshotPolicy::forbid_while(move || s.selection.snapshot_live()));
        Ok(())
    }
}

/// Adds the feature when at least one campaign is installed. Returns
/// whether it was added.
pub fn install(
    rt: &runtime::Runtime,
    registry: &mut feature::Registry,
    state: Option<Arc<State>>,
) -> anyhow::Result<bool> {
    let Some(state) = state else {
        return Ok(false);
    };
    rt.memory()
        .map_zeroed(Region::BASE, Region::LEN, Permissions::RW)?;
    REGION.store(Region::BASE, Ordering::SeqCst);
    let picker: Arc<dyn Fn() -> u32 + Send + Sync> =
        Arc::new(|| crate::list_class::loader_of(SCREEN).unwrap_or(0));
    let added = registry
        .add(&Campaigns { state, picker })
        .map_err(|e| anyhow::anyhow!("campaigns: {e}"))?;
    if !added {
        eprintln!("campaigns: features.campaigns is off; one campaign, as retail");
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::campaigns::{Progress, Selection};
    use content::campaign_list::{self as cl, CampaignManifest, FIXTURE, HALO1};

    fn state() -> State {
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
    fn a_press_picks_the_campaign_and_opens_the_level_select_for_it() {
        let st = Arc::new(state());
        let rows = PickerRows {
            picker: Picker::new(&st.built),
            state: Arc::clone(&st),
        };
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
                assert_eq!(loader, LEVEL_SELECT_LOADER);
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
    fn the_hooks_are_on_the_plan() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let picker: Arc<dyn Fn() -> u32 + Send + Sync> = Arc::new(|| 0);
        r.add(&Campaigns {
            state: Arc::new(state()),
            picker,
        })
        .unwrap();
        let text = r.build(&feature::Base).unwrap().text();
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
