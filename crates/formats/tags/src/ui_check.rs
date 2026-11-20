//! Whole-map consistency check of the UI tags, shared by `h2 ui --all` and the
//! `verify_ui` example. Reports counts and tag IDs only, never names or text.
//!
//! Beyond strict decoding of every `wgit`, `wgtz`, `wigl` and `skin` tag it
//! checks what the engine indexes without a bound check:
//! - the map has exactly one `wigl` (the engine reads the one `wgtz +0` names);
//! - widget animation indices fall inside the `wigl` animation collections;
//! - shader-bitmap animation values (stored without the minus one) do too,
//!   read either way: `value <= animations`;
//! - list skin indices fall inside the `wigl` skin list, and every skin a
//!   list uses has the three item animations `0x0022e78a`, `0x0024ac9b` and
//!   `0x0024ad63` index directly (0, 1 and 2);
//! - table skins resolve within [`MAX_SKIN_DEPTH`](super::MAX_SKIN_DEPTH) and
//!   every table cell the engine places has an origin that fits in 16 bits.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Item animations the list code indexes directly (0, 1 and 2).
pub const LIST_ITEM_ANIMATIONS: usize = 3;

/// Per-group totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupCounts {
    pub tags: usize,
    pub failures: usize,
    pub stats: Stats,
    /// Widget definitions (pane and skin blocks).
    pub widgets: usize,
}
impl Stats {
    fn add(&mut self, o: &Stats) {
        self.blocks += o.blocks;
        self.nonempty_blocks += o.nonempty_blocks;
        self.elements += o.elements;
        self.references += o.references;
        self.string_ids += o.string_ids;
        self.bytes += o.bytes;
        self.aliased_blocks += o.aliased_blocks;
        self.foreign_aliases += o.foreign_aliases;
    }
}

/// Result of [`check_map`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapCheck {
    pub groups: BTreeMap<FourCC, GroupCounts>,
    /// One line per failure: map-level problems, or `<tag id> <group>: error`.
    pub failures: Vec<String>,
    /// Screens whose shape-group index is past the `wigl` list (skipped by
    /// the engine, `0x0022facd`).
    pub ignored_shape_groups: usize,
    /// Screens a `wgtz` screen list names more than once.
    pub duplicate_listings: usize,
    /// Payload spans that start inside another span.
    pub overlapping_spans: usize,
    /// Deepest skin → table → skin nesting seen.
    pub max_skin_depth: usize,
    /// Largest budget share one decode used, in bytes, and the budget.
    pub max_decode_bytes: u64,
    pub budget: u64,
    /// Nonempty raw `wigl` blocks at +0x88, +0x130 and +0x158.
    pub raw_wigl_blocks: [usize; 3],
    /// Shader bitmaps with a nonzero animation value.
    pub shader_animations: usize,
    /// Table cells (`0..cells`) outside the rows × columns grid, which the
    /// engine does not place (`0x002afdbc`).
    pub skipped_cells: usize,
}
impl MapCheck {
    pub fn total(&self) -> GroupCounts {
        let mut t = GroupCounts::default();
        for g in self.groups.values() {
            t.tags += g.tags;
            t.failures += g.failures;
            t.stats.add(&g.stats);
            t.widgets += g.widgets;
        }
        t
    }
    /// Fold another map's check into this one (counts add, maxima combine).
    pub fn merge(&mut self, o: MapCheck) {
        for (g, c) in o.groups {
            let t = self.groups.entry(g).or_default();
            t.tags += c.tags;
            t.failures += c.failures;
            t.stats.add(&c.stats);
            t.widgets += c.widgets;
        }
        self.failures.extend(o.failures);
        self.ignored_shape_groups += o.ignored_shape_groups;
        self.duplicate_listings += o.duplicate_listings;
        self.overlapping_spans += o.overlapping_spans;
        self.max_skin_depth = self.max_skin_depth.max(o.max_skin_depth);
        if o.max_decode_bytes * self.budget.max(1) >= self.max_decode_bytes * o.budget.max(1) {
            self.max_decode_bytes = o.max_decode_bytes;
            self.budget = o.budget;
        }
        for i in 0..3 {
            self.raw_wigl_blocks[i] += o.raw_wigl_blocks[i];
        }
        self.shader_animations += o.shader_animations;
        self.skipped_cells += o.skipped_cells;
    }
}

/// Limits taken from the map's shared globals.
struct Limits<'g> {
    shared: &'g SharedGlobals,
    skipped_cells: &'g std::cell::Cell<usize>,
}
impl Limits<'_> {
    fn anim(&self, a: AnimationRef) -> std::result::Result<(), String> {
        match a.index() {
            Some(i) if self.shared.animation(a).is_none() => {
                Err(format!("animation {i} >= {}", self.shared.animations.len()))
            }
            _ => Ok(()),
        }
    }
    fn shader(&self, b: &ShaderBitmap) -> std::result::Result<(), String> {
        let n = self.shared.animations.len();
        if usize::from(b.animation_raw) > n {
            return Err(format!("shader bitmap animation {} > {n}", b.animation_raw));
        }
        Ok(())
    }
    fn widgets(
        &self,
        text: &[TextBlock],
        bitmaps: &[BitmapBlock],
        shaders: &[ShaderBitmap],
        tables: &[Table],
    ) -> std::result::Result<(), String> {
        for a in text
            .iter()
            .map(|t| t.animation)
            .chain(bitmaps.iter().map(|b| b.animation))
        {
            self.anim(a)?;
        }
        for s in shaders {
            self.shader(s)?;
        }
        for t in tables {
            for i in 0..i32::from(t.cells) {
                if t.cell_grid(i).is_none() {
                    self.skipped_cells.set(self.skipped_cells.get() + 1);
                } else if t.cell_origin(i).is_none() {
                    return Err(format!("table cell {i} origin overflows 16 bits"));
                }
            }
        }
        Ok(())
    }
}

fn pane_widgets(p: &Pane) -> usize {
    p.buttons.len()
        + p.lists.len()
        + p.text_blocks.len()
        + p.bitmaps.len()
        + p.model_scenes.len()
        + p.shader_bitmaps.len()
        + p.tables.len()
}

/// Check every UI tag of one map. Never panics on bad data; every problem is
/// a line in [`MapCheck::failures`].
pub fn check_map<R: Read + Seek>(map: &mut Map<R>) -> MapCheck {
    let mut out = MapCheck::default();
    let ids: Vec<(TagId, FourCC)> = map
        .tags()
        .iter()
        .filter(|t| supports(t.group))
        .map(|t| (t.id, t.group))
        .collect();
    if ids.is_empty() {
        out.failures.push("map has no UI tags".into());
        return out;
    }
    let mut src = MapSource::new(map);
    out.overlapping_spans = src.overlapping_spans();
    out.budget = src.budget();

    let wigl: Vec<TagId> = ids
        .iter()
        .filter(|t| t.1 == SHARED_GLOBALS)
        .map(|t| t.0)
        .collect();
    let shared = match wigl.as_slice() {
        [id] => match src.decode_tag(*id) {
            Ok(Decoded {
                tag: UiTag::SharedGlobals(g),
                ..
            }) => Some(g),
            Ok(_) => None,
            Err(e) => {
                out.failures.push(format!("{id} wigl: {e}"));
                None
            }
        },
        other => {
            out.failures.push(format!(
                "map has {} wigl tags, expected exactly 1",
                other.len()
            ));
            None
        }
    };
    if let Some(g) = &shared {
        for (i, b) in [g.unknown_088, g.unknown_130, g.unknown_158]
            .iter()
            .enumerate()
        {
            out.raw_wigl_blocks[i] += usize::from(b.count != 0);
        }
    }

    let skipped_cells = std::cell::Cell::new(0);
    let mut listed = BTreeMap::<u32, usize>::new();
    let mut list_skins = BTreeSet::<u32>::new();
    let mut item_animations = BTreeMap::<u32, usize>::new();
    for &(id, group) in &ids {
        let mut counts = GroupCounts {
            tags: 1,
            ..GroupCounts::default()
        };
        let mut check = || -> std::result::Result<(), String> {
            let d = src.decode_tag(id).map_err(|e| e.to_string())?;
            counts.stats.add(&d.stats);
            out.max_decode_bytes = out.max_decode_bytes.max(d.stats.bytes);
            let limits = shared.as_deref().map(|shared| Limits {
                shared,
                skipped_cells: &skipped_cells,
            });
            match &d.tag {
                UiTag::Screen(s) => {
                    if let Some(g) = &shared
                        && s.shape_group != 0
                        && g.shape_group(s.shape_group).is_none()
                    {
                        out.ignored_shape_groups += 1;
                    }
                    for p in &s.panes {
                        counts.widgets += pane_widgets(p);
                        let Some(l) = &limits else { continue };
                        l.anim(p.animation)?;
                        for a in p
                            .buttons
                            .iter()
                            .map(|b| b.animation)
                            .chain(p.lists.iter().map(|b| b.animation))
                            .chain(p.model_scenes.iter().map(|b| b.animation))
                        {
                            l.anim(a)?;
                        }
                        l.widgets(&p.text_blocks, &p.bitmaps, &p.shader_bitmaps, &p.tables)?;
                        for list in &p.lists {
                            let skin = l.shared.skin(list.skin).ok_or_else(|| {
                                format!("list skin {} >= {}", list.skin, l.shared.skins.len())
                            })?;
                            if let Some(s) = skin.id {
                                list_skins.insert(s.0);
                            }
                        }
                    }
                }
                UiTag::Skin(s) => {
                    counts.widgets += s.text_blocks.len()
                        + s.bitmaps.len()
                        + s.shader_bitmaps.len()
                        + s.tables.len();
                    item_animations.insert(id.0, s.item_animations.len());
                    if let Some(l) = &limits {
                        l.widgets(&s.text_blocks, &s.bitmaps, &s.shader_bitmaps, &s.tables)?;
                    }
                    let (tree, _) = resolve_skin(&mut src, id).map_err(|e| e.to_string())?;
                    out.max_skin_depth = out.max_skin_depth.max(tree.depth());
                    out.shader_animations += s
                        .shader_bitmaps
                        .iter()
                        .filter(|b| b.animation_raw != 0)
                        .count();
                }
                UiTag::UiGlobals(g) => {
                    if !wigl.is_empty() && g.shared.id.is_some_and(|w| !wigl.contains(&w)) {
                        return Err("wgtz names a wigl this map does not hold".into());
                    }
                    for r in g.screens.iter().filter_map(|r| r.id) {
                        *listed.entry(r.0).or_default() += 1;
                    }
                }
                UiTag::SharedGlobals(g) => {
                    if let Some(l) = &limits {
                        for a in &g.shape_groups {
                            for b in &a.bitmaps {
                                l.anim(b.animation)?;
                            }
                            for m in &a.model_scenes {
                                l.anim(m.animation)?;
                            }
                        }
                    }
                }
            }
            if let UiTag::Screen(s) = &d.tag {
                out.shader_animations += s
                    .panes
                    .iter()
                    .flat_map(|p| &p.shader_bitmaps)
                    .filter(|b| b.animation_raw != 0)
                    .count();
            }
            Ok(())
        };
        if let Err(e) = check() {
            counts.failures += 1;
            out.failures.push(format!("{id} {group}: {e}"));
        }
        let total = out.groups.entry(group).or_default();
        total.tags += counts.tags;
        total.failures += counts.failures;
        total.stats.add(&counts.stats);
        total.widgets += counts.widgets;
    }
    for s in list_skins {
        let n = item_animations.get(&s).copied().unwrap_or(0);
        if n < LIST_ITEM_ANIMATIONS {
            out.failures.push(format!(
                "{} skin: list skin has {n} item animations, the list code reads {LIST_ITEM_ANIMATIONS}",
                TagId(s)
            ));
        }
    }
    out.duplicate_listings = listed.values().filter(|n| **n > 1).count();
    out.skipped_cells = skipped_cells.get();
    out
}
