//! The Mods screens described for the UI kit (docs/game/ui-builder.md):
//! what the list and a map's page show, as [`ScreenSpec`]s the preview,
//! the sheet and the layout checks read, over a sample repository whose
//! entries are invented ([`sample_repo`]).

use std::collections::BTreeSet;
use std::sync::Arc;

use content::MapId;
use live_hle::mods::{InstallDone, ListDone, ModEntry, ModPage, ModsError, Progress};

use super::{
    DETAIL, DETAIL_FOOTER, DETAIL_TITLE, LIST_FOOTER, LIST_TITLE, PANEL_TEXTS, Repo, SCREEN, Slots,
    Source, page_layout, prompts,
};
use crate::list_class::PictureSpec;
use crate::ui_kit::input::Input;
use crate::ui_kit::spec::{ListPlace, PaneText, Press, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::view::{Art, PaneArt};

/// A source for previews: the list comes from [`sample_entries`], nothing
/// is here, an install never ends, and nothing is removed.
pub struct SampleSource;

impl Source for SampleSource {
    fn list(&self, _limit: u32, _done: ListDone) -> Result<(), ModsError> {
        Ok(())
    }
    fn install(&self, _id: MapId, _p: Progress, _done: InstallDone) -> Result<(), ModsError> {
        Ok(())
    }
    fn installed(&self, _ids: &[MapId]) -> BTreeSet<MapId> {
        BTreeSet::new()
    }
    fn remove(&self, _id: MapId) -> Result<(), String> {
        Err("a sample".into())
    }
}

/// Three invented mods, read through the server's own JSON shape: one with
/// every field, one Map Pack mod with no checklist, one bare.
pub fn sample_entries() -> Vec<ModEntry> {
    let id = |n: u8| format!("{n:02x}").repeat(32);
    let page: ModPage = serde_json::from_value(serde_json::json!({
        "items": [
            {
                "id": id(1), "name": "Lockout Swords", "author": "Modder",
                "credits": { "authors": ["Blade", "Edge"] },
                "base": "lockout", "size": 3_500_000, "position": 30, "downloads": 12,
                "published_at": "2026-11-18T10:00:00Z", "version": "1.2",
                "features": ["weapons", "objects", "scripts"],
                "gametypes": ["slayer", "ctf"],
                "description": "Energy swords only, with sword stands on every ledge and the sniper tower closed off. Made for close fights."
            },
            {
                "id": id(2), "name": "Containment Night", "author": "Modder",
                "base": "containment", "size": 9_000_000, "position": 20,
                "published_at": "2026-11-17T09:00:00Z"
            },
            {
                "id": id(3), "name": "Midship Rockets", "author": "Rocketeer",
                "base": "midship", "size": 400_000, "position": 10
            }
        ],
        "position": 30
    }))
    .expect("the sample entries read");
    page.items
}

/// A repository over [`SampleSource`] with [`sample_entries`] listed and,
/// when `page` is set, the first one's page open.
pub fn sample_repo(page: bool) -> Arc<Repo> {
    let repo = Repo::new(Arc::new(SampleSource), None);
    repo.listed(Ok(sample_entries()));
    if page {
        repo.press_list(0);
    }
    repo
}

fn art(p: PictureSpec, size: [u16; 2]) -> PaneArt {
    PaneArt {
        art: Art::Screen {
            screen_id: p.screen_id as i16,
            pane: p.pane as u16,
            index: p.bitmap as u16,
            image: p.image,
        },
        at: p.at,
        size,
        depth: 0,
    }
}

/// The list: the level select's layout, every row a map (or the one row
/// that reads the list again), its summary as the description.
pub fn list_spec(repo: &Repo) -> ScreenSpec {
    let mut s = ScreenSpec::new(SCREEN, &repo.texts.get(LIST_TITLE).unwrap_or_default());
    s.list = ListPlace::from(Some(crate::skulls_menu::LIST_LAYOUT));
    s.style = ValueStyle::Inline;
    s.description = Some(crate::skulls_menu::DESCRIPTION_TEXT.into());
    s.scrolls = true;
    s.art = vec![
        art(crate::skulls_menu::FRAME, crate::skulls_menu::FRAME_SIZE),
        art(
            crate::campaign_menu::PICTURE,
            crate::campaign_menu::PICTURE_SIZE,
        ),
    ];
    s.prompts = prompts(SCREEN, false);
    s.footer = true;
    s.inputs = vec![Input::Up, Input::Down, Input::A, Input::B, Input::X];
    s.ids = vec![Slots::id(LIST_TITLE), Slots::id(LIST_FOOTER)];
    let has_entries = !repo.entries().is_empty();
    for row in 0..repo.list_count() as i16 {
        let line = repo.list_line(row).unwrap_or_default();
        let press = if has_entries {
            Press::Open(DETAIL.into())
        } else {
            Press::Step
        };
        let mut r = RowSpec::action(&line).press(press);
        if let Some(d) = repo.list_description(row) {
            r = r.describe(&d);
        }
        s.rows.push(r);
    }
    s
}

/// A map's page: the pregame lobby's layout ([`page_layout`]), its one row
/// the action, every other text in the panel.
pub fn detail_spec(repo: &Repo) -> ScreenSpec {
    let mut s = ScreenSpec::new(DETAIL, &repo.texts.get(DETAIL_TITLE).unwrap_or_default());
    s.list = ListPlace::from(Some(page_layout::LIST));
    s.style = ValueStyle::Inline;
    s.description = Some(page_layout::STATUS.into());
    s.panel = page_layout::ART
        .iter()
        .zip(page_layout::ART_SIZES)
        .map(|(p, size)| art(*p, size))
        .collect();
    s.art = vec![art(page_layout::PICTURE, page_layout::PICTURE_SIZE)];
    let texts = repo.panel();
    s.texts = page_layout::panel()
        .texts
        .iter()
        .enumerate()
        .map(|(i, t)| PaneText {
            id: page_layout::text_id(i),
            rect: t.block.rect,
            font: t.block.font,
            colour: t.block.colour,
            right: t.right,
            text: texts[i].clone(),
        })
        .collect();
    let here = repo.selected().is_some_and(|e| repo.is_installed(&e.id));
    s.prompts = prompts(DETAIL, here);
    s.footer = true;
    s.inputs = vec![Input::A, Input::B, Input::X];
    s.ids = vec![Slots::id(DETAIL_TITLE), Slots::id(DETAIL_FOOTER)];
    s.ids
        .extend((0..page_layout::TEXTS as u32).map(|i| Slots::id(PANEL_TEXTS + i)));
    if let Some(line) = repo.detail_line(0) {
        let mut r = RowSpec::action(&line).press(Press::Act);
        if let Some(d) = repo.detail_description(0) {
            r = r.describe(&d);
        }
        s.rows.push(r);
    }
    s
}
