//! A `wgit` for each of our screens, added to a decomposed map
//! (docs/game/ui-in-maps.md, "The tag").
//!
//! The template is the retail screen the tag is shaped like, read from the
//! same map (`tags::ui::MapSource`): its root (`0x68` bytes), its first pane
//! (`0x4c`), its first list (`0x18`) and its pane's bitmap blocks (`0x38`
//! each). A template's tag is pinned, so a pointer into its blocks stays
//! valid in the rebuilt map: the new tag's pane keeps the template's text,
//! button, table, shader and model blocks by address, as `copy_tag` kept
//! them in the copy, and only the list and the bitmaps are new blocks
//! (`map::build::Block`). Nothing of a template is written anywhere but
//! the map this builds.
//!
//! The screens are listed in the map's `wgtz` (`0x00148098` walks its
//! screens block, comparing `wgit +4`): each `wgtz`'s block is laid out
//! again past the pinned metadata (`TagSet::append_pinned_blocks`) with the
//! old entries first, so retail screens are found as before.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use anyhow::{Context, Result, anyhow, bail};
use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map, TagId};
use serde::{Deserialize, Serialize};
use tags::ui::{self, MapSource, Source, UiTag};

use super::{Entry, Shape, tracks};
use crate::list_class::{BITMAP_BLOCK_SIZE, LIST_BLOCK_SIZE, PANE_SIZE, SCREEN_ROOT_SIZE};
use crate::ui_kit::dialog_tag::{BASE_BITMAPS, PaneTracks};
use crate::ui_kit::spec::ListPlace;

/// A root's panes block, a pane's lists and bitmaps blocks, and the
/// fields `list_class::copy_tag` sets (docs/formats/ui.md).
const ROOT_PANES: u32 = 0x20;
const ROOT_ID: usize = 0x04;
const PANE_ANIMATION: usize = 0x02;
const PANE_LISTS: u32 = 0x0c;
const PANE_BITMAPS: u32 = 0x24;
const LIST_SKIN: usize = 0x04;
const LIST_VISIBLE: usize = 0x06;
const LIST_ANCHOR: usize = 0x08;
const LIST_ANIMATION: usize = 0x0c;
const BITMAP_POINT: usize = 0x0c;
/// Frame bitmaps a list screen keeps at most (`list_class::KEPT_BITMAPS`).
const KEPT_BITMAPS: usize = 4;
/// A `wgtz` root's screens block (count, address).
const WGTZ_SCREENS: usize = 8;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// What a template screen holds that the tag takes over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    /// The retail screen id.
    pub id: i16,
    pub root: Vec<u8>,
    /// `None` for a screen with no pane.
    pub pane: Option<PaneRaw>,
}

/// A template's first pane, and the blocks of it the tag copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRaw {
    pub bytes: Vec<u8>,
    pub list: Option<Vec<u8>>,
    pub bitmaps: Vec<Vec<u8>>,
}

/// The shared UI globals' sizes the tags are checked against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wigl {
    /// `wigl +0x120`: animation collections.
    pub animations: usize,
    /// `wigl +0x138`: list skins.
    pub skins: usize,
}

fn place_list(list: &mut Block, place: &ListPlace) {
    if let ListPlace::Moved {
        skin,
        anchor,
        visible,
        ..
    } = *place
    {
        list.put_u16(LIST_SKIN, skin as u16);
        if visible > 0 {
            list.put_u16(LIST_VISIBLE, visible as u16);
        }
        list.put_u16(LIST_ANCHOR, anchor[0] as u16);
        list.put_u16(LIST_ANCHOR + 2, anchor[1] as u16);
    }
}

/// `blocks` as one child block of `BITMAP_BLOCK_SIZE` elements.
fn bitmaps_child(blocks: &[&Vec<u8>]) -> Block {
    Block::new(blocks.iter().flat_map(|b| b.iter().copied()).collect())
}

/// The tag for `e`, from `t` (module docs), as the block tree the builder
/// lays out. Says what is wrong when the template or the layout cannot
/// make one.
pub fn screen_block(e: &Entry, t: &Template, wigl: &Wigl) -> Result<Block, String> {
    let why = |what: String| format!("{}: {what}", e.name);
    let want = tracks(e.animation, wigl.animations);
    if let ListPlace::Moved { skin, .. } = e.list
        && usize::try_from(skin).map_or(true, |s| s >= wigl.skins)
    {
        return Err(why(format!(
            "list skin {skin} is past the map's {} skins",
            wigl.skins
        )));
    }
    if t.root.len() != SCREEN_ROOT_SIZE as usize {
        return Err(why("the template's root is the wrong size".into()));
    }
    let mut root = Block::new(t.root.clone());
    root.put_u16(ROOT_ID, e.id as u16);
    let pane_of = || {
        t.pane
            .as_ref()
            .ok_or_else(|| why(format!("template screen {} has no pane", t.id)))
    };
    match e.shape {
        Shape::List => {
            if matches!(e.list, ListPlace::Controls) && want.is_none() {
                // Retail's own: copy_tag serves the retail tag as it is.
                return Ok(root);
            }
            let p = pane_of()?;
            let mut pane = Block::new(p.bytes.clone());
            if let Some((pane_track, _)) = want {
                pane.put_u16(PANE_ANIMATION, pane_track);
            }
            if let Some(raw) = &p.list {
                let mut list = Block::new(raw.clone());
                if let Some((_, rows)) = want {
                    list.put_u16(LIST_ANIMATION, rows);
                }
                place_list(&mut list, &e.list);
                pane.block(PANE_LISTS, 1, list);
            }
            if let ListPlace::Moved {
                bitmaps_left_of, ..
            } = e.list
            {
                let kept: Vec<&Vec<u8>> = p
                    .bitmaps
                    .iter()
                    .filter(|b| (u16_at(b, BITMAP_POINT) as i16) < bitmaps_left_of)
                    .take(KEPT_BITMAPS)
                    .collect();
                let n = kept.len() as u32;
                pane.block(PANE_BITMAPS, n, bitmaps_child(&kept));
            }
            root.block(ROOT_PANES, 1, pane);
        }
        Shape::Dialog { .. } => {
            let p = pane_of()?;
            let Some(raw) = &p.list else {
                return Err(why(format!("template screen {} has no list", t.id)));
            };
            let motion = match want {
                Some((pane, rows)) => (pane, rows),
                None => {
                    let m = PaneTracks::inherited(u16_at(&p.bytes, PANE_ANIMATION), u16_at(raw, LIST_ANIMATION));
                    (m.pane, m.rows)
                }
            };
            let mut pane = Block::zeroed(PANE_SIZE as usize);
            pane.bytes[..4].copy_from_slice(&p.bytes[..4]);
            pane.put_u16(PANE_ANIMATION, motion.0);
            let mut list = Block::new(raw.clone());
            list.put_u16(LIST_ANIMATION, motion.1);
            place_list(&mut list, &e.list);
            pane.block(PANE_LISTS, 1, list);
            let kept: Vec<&Vec<u8>> = p.bitmaps.iter().take(BASE_BITMAPS as usize).collect();
            pane.block(PANE_BITMAPS, kept.len() as u32, bitmaps_child(&kept));
            root.block(ROOT_PANES, 1, pane);
        }
        Shape::Bare => {
            let p = pane_of()?;
            let mut pane = Block::zeroed(PANE_SIZE as usize);
            pane.bytes[..4].copy_from_slice(&p.bytes[..4]);
            if let Some((pane_track, _)) = want {
                pane.put_u16(PANE_ANIMATION, pane_track);
            }
            root.block(ROOT_PANES, 1, pane);
        }
    }
    Ok(root)
}

/// One `wgtz` of the map: where its screens block is, and which retail
/// screens it lists (the first listing of an id wins, as the lookup's).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WgtzFacts {
    pub tag: TagId,
    /// The screens block's entries as they are (8 bytes each).
    pub screens: Vec<u8>,
    pub by_id: BTreeMap<i16, TagId>,
}

/// What the map's own UI tags say that the tags are made from.
pub struct Facts {
    pub wgtz: Vec<WgtzFacts>,
    pub wigl: Wigl,
    /// The templates the entries can use, by tag: a template that cannot
    /// be read says why.
    pub templates: BTreeMap<TagId, Result<Template, String>>,
}

fn read_template(src: &mut impl Source, id: i16, tag: TagId) -> Result<Template, String> {
    fn bad(id: i16, what: &str, e: impl std::fmt::Display) -> String {
        format!("screen {id}: {what}: {e}")
    }
    let address = src
        .address(tag)
        .ok_or_else(|| format!("screen {id} has no data"))?;
    let root = src
        .read(address, SCREEN_ROOT_SIZE)
        .map_err(|e| bad(id, "root", e))?
        .to_vec();
    let (panes, pane_at) = (u32_at(&root, ROOT_PANES as usize), u32_at(&root, ROOT_PANES as usize + 4));
    if panes == 0 {
        return Ok(Template {
            id,
            root,
            pane: None,
        });
    }
    let bytes = src
        .read(pane_at, PANE_SIZE)
        .map_err(|e| bad(id, "pane", e))?
        .to_vec();
    let (lists, list_at) = (
        u32_at(&bytes, PANE_LISTS as usize),
        u32_at(&bytes, PANE_LISTS as usize + 4),
    );
    let list = if lists > 0 {
        Some(
            src.read(list_at, LIST_BLOCK_SIZE)
                .map_err(|e| bad(id, "list", e))?
                .to_vec(),
        )
    } else {
        None
    };
    let (n, bitmaps_at) = (
        u32_at(&bytes, PANE_BITMAPS as usize),
        u32_at(&bytes, PANE_BITMAPS as usize + 4),
    );
    let bitmaps = if n > 0 {
        src.read(bitmaps_at, n.min(64) * BITMAP_BLOCK_SIZE)
            .map_err(|e| bad(id, "bitmaps", e))?
            .chunks_exact(BITMAP_BLOCK_SIZE as usize)
            .map(<[u8]>::to_vec)
            .collect()
    } else {
        Vec::new()
    };
    Ok(Template {
        id,
        root,
        pane: Some(PaneRaw {
            bytes,
            list,
            bitmaps,
        }),
    })
}

/// Reads the map's `wgtz` tags, its `wigl` and the templates `plan` names.
pub fn read_facts<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    plan: &[Entry],
) -> Result<Facts> {
    let mut src = MapSource::new(map);
    let all: Vec<(TagId, FourCC)> = src
        .map_ref()
        .tags()
        .iter()
        .map(|t| (t.id, t.group))
        .collect();
    let wigl_tags: Vec<TagId> = all
        .iter()
        .filter(|t| t.1 == ui::SHARED_GLOBALS)
        .map(|t| t.0)
        .collect();
    let [wigl_tag] = wigl_tags.as_slice() else {
        bail!("the map has {} wigl tags, expected 1", wigl_tags.len());
    };
    let wigl = match src
        .decode_tag(*wigl_tag)
        .map_err(|e| anyhow!("wigl: {e}"))?
        .tag
    {
        UiTag::SharedGlobals(g) => Wigl {
            animations: g.animations.len(),
            skins: g.skins.len(),
        },
        _ => bail!("the wigl did not decode as shared globals"),
    };
    let mut wgtz = Vec::new();
    for (tag, _) in all.iter().filter(|t| t.1 == ui::UI_GLOBALS) {
        let address = src.address(*tag).context("a wgtz has no data")?;
        let root = src
            .read(address, 0x20)
            .map_err(|e| anyhow!("wgtz root: {e}"))?
            .to_vec();
        let (count, at) = (u32_at(&root, WGTZ_SCREENS), u32_at(&root, WGTZ_SCREENS + 4));
        let screens = if count > 0 {
            src.read(at, count.saturating_mul(8))
                .map_err(|e| anyhow!("wgtz screens: {e}"))?
                .to_vec()
        } else {
            Vec::new()
        };
        let mut by_id = BTreeMap::new();
        for entry in screens.chunks_exact(8) {
            let datum = u32_at(entry, 4);
            if datum == u32::MAX {
                continue;
            }
            let id = TagId(datum);
            if src.group(id) != Some(ui::SCREEN) {
                continue;
            }
            let Some(a) = src.address(id) else { continue };
            let Ok(head) = src.read(a, 8) else { continue };
            by_id.entry(u16_at(&head, ROOT_ID) as i16).or_insert(id);
        }
        wgtz.push(WgtzFacts {
            tag: *tag,
            screens,
            by_id,
        });
    }
    let wanted: BTreeSet<i16> = plan.iter().flat_map(Entry::templates).collect();
    let mut templates = BTreeMap::new();
    for w in &wgtz {
        for id in &wanted {
            if let Some(&tag) = w.by_id.get(id) {
                templates
                    .entry(tag)
                    .or_insert_with(|| read_template(&mut src, *id, tag));
            }
        }
    }
    Ok(Facts {
        wgtz,
        wigl,
        templates,
    })
}

/// What [`add_screens`] made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Added {
    #[serde(skip)]
    pub map: Vec<u8>,
    /// Each tag added: its name and its screen id.
    pub injected: Vec<(String, i16)>,
    /// Each screen left out, and why (no template in the map's interface,
    /// a skin the map lacks).
    pub skipped: Vec<(String, String)>,
}

/// `original` (a map) with a `wgit` for each entry of `plan` its interface
/// can hold, listed in every `wgtz`. An error when none could be added.
pub fn add_screens(original: &[u8], plan: &[Entry]) -> Result<Added> {
    let mut map = Map::from_reader(Cursor::new(original)).context("not a map")?;
    let facts = read_facts(&mut map, plan)?;
    if facts.wgtz.is_empty() {
        bail!("the map has no wgtz");
    }
    let mut set = TagSet::from_map(&mut map).context("decomposing the map")?;
    // The index takes more tags (as campaign_cards::add_cards does, and
    // for the same reason: the tag-memory bound is empirical and retail
    // maps already meet it; the tags add a few kilobytes).
    let pinned = set
        .pinned
        .as_mut()
        .context("a decomposed map has its image")?;
    pinned.clear_index_tail();
    pinned.index_entries_end = pinned.index.len() as u32;
    set.check_tag_memory = false;
    set.add_group(FourCC(*b"wgit"), None, None);

    let mut made: BTreeMap<(usize, TagId), TagId> = BTreeMap::new();
    let mut injected: BTreeMap<String, i16> = BTreeMap::new();
    let mut skipped: BTreeMap<String, String> = BTreeMap::new();
    let mut listed: Vec<(TagId, usize, Vec<TagId>)> = Vec::new();
    for w in &facts.wgtz {
        let mut refs = Vec::new();
        for (i, e) in plan.iter().enumerate() {
            let Some(template_tag) = e.templates().iter().find_map(|id| w.by_id.get(id).copied())
            else {
                skipped
                    .entry(e.name.clone())
                    .or_insert_with(|| format!("no screen {:?} in the map's interface", e.templates()));
                continue;
            };
            if let Some(&id) = made.get(&(i, template_tag)) {
                refs.push(id);
                continue;
            }
            let template = match &facts.templates[&template_tag] {
                Ok(t) => t,
                Err(why) => {
                    skipped.insert(e.name.clone(), why.clone());
                    continue;
                }
            };
            let block = match screen_block(e, template, &facts.wigl) {
                Ok(b) => b,
                Err(why) => {
                    skipped.insert(e.name.clone(), why);
                    continue;
                }
            };
            let twins = made.keys().filter(|(j, _)| *j == i).count();
            let name = if twins == 0 {
                e.tag.clone()
            } else {
                format!("{}~{}", e.tag, twins + 1)
            };
            let id = set.add_tag(FourCC(*b"wgit"), &name, TagPayload::Built(block))?;
            made.insert((i, template_tag), id);
            injected.insert(name, e.id);
            skipped.remove(&e.name);
            refs.push(id);
        }
        listed.push((w.tag, w.screens.len() / 8, refs));
    }
    if made.is_empty() {
        bail!(
            "no screen could be added: {}",
            skipped
                .iter()
                .map(|(n, w)| format!("{n}: {w}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    // Each wgtz's screens block, laid out again with ours after the
    // retail entries (the lookup takes the first match, so retail wins a
    // shared id, and ours have none).
    for (w, (wgtz, old, refs)) in facts.wgtz.iter().zip(listed) {
        let mut bytes = w.screens.clone();
        bytes.resize(bytes.len() + 8 * refs.len(), 0);
        let mut block = Block::new(bytes);
        for (k, id) in refs.iter().enumerate() {
            block.tag_ref(((old + k) * 8) as u32, *id);
        }
        let (address, _) = set.append_pinned_blocks(&[block])?[0];
        let tag = set.tag_mut(wgtz).context("a wgtz left the set")?;
        let TagPayload::Pinned { bytes, .. } = &mut tag.payload else {
            bail!("a wgtz is not pinned");
        };
        bytes[WGTZ_SCREENS..WGTZ_SCREENS + 4].copy_from_slice(&((old + refs.len()) as u32).to_le_bytes());
        bytes[WGTZ_SCREENS + 4..WGTZ_SCREENS + 8].copy_from_slice(&address.to_le_bytes());
    }
    let (map, _) = set.build().context("building the map")?;
    Ok(Added {
        map,
        injected: injected.into_iter().collect(),
        skipped: skipped.into_iter().collect(),
    })
}
