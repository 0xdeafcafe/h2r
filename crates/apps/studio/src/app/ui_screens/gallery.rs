//! The UI Screens tool's grid of previews: what a folder or a root shows
//! ([`Section`]s of [`Tile`]s), the thumbnails drawn for it on the CPU with
//! the same path as the picture (lazily, a bounded time per frame, cached),
//! and the contact sheet the agent API writes of it.
//! docs/editor/editor.md "UI Screens".

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use boot::ui_kit::preview::{self, Canvas, Disc, Retail};
use boot::ui_kit::screens;

use super::browse::{self, Browse, Folder, Item, Ours, Root};
use super::{Pick, UiScreens};

/// A thumbnail fits in this many pixels (half the screen).
pub const THUMB: [u32; 2] = [320, 240];

/// Thumbnails kept before those off screen are dropped.
const KEEP: usize = 512;

/// One preview in the grid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tile {
    pub pick: Pick,
    /// What the tile says inside its folder.
    pub label: String,
    /// The full name (screen, screen/state, or the tag) for a tooltip.
    pub name: String,
}

/// Tiles under a heading (empty for the folder's own screens).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub tiles: Vec<Tile>,
}

/// A drawn thumbnail.
pub struct Thumb {
    /// Bumped on every draw, so a texture knows to upload.
    pub id: u64,
    /// At most [`THUMB`]; none when the screen did not draw.
    pub canvas: Option<Canvas>,
    pub findings: usize,
    /// Each finding's line (`layout: ...`, `consistency: ...`).
    pub found: Vec<String>,
    /// Whether the layout checks ran (ours do, retail screens do not).
    pub checked: bool,
    pub error: Option<String>,
}

/// The disc's screens as folders.
pub struct RetailTree {
    pub tree: Folder,
    /// The folders every tag shares, left off the tree.
    pub stripped: String,
}

/// The grid's state.
pub struct Gallery {
    /// The grid shown in place of the picture (none: the picture).
    pub at: Option<Browse>,
    /// One tile per state of our screens, not just the first.
    pub states: bool,
    /// Tile size as a fraction of [`THUMB`].
    pub zoom: f32,
    thumbs: HashMap<Pick, Thumb>,
    drawn: u64,
    retail: Option<Arc<RetailTree>>,
}

impl Default for Gallery {
    fn default() -> Self {
        Gallery {
            at: None,
            states: false,
            zoom: 0.6,
            thumbs: HashMap::new(),
            drawn: 0,
            retail: None,
        }
    }
}

impl Gallery {
    /// The disc changed: nothing drawn from the old one stands.
    pub(super) fn new_disc(&mut self) {
        self.thumbs.clear();
        self.retail = None;
    }

    pub fn thumb(&self, pick: &Pick) -> Option<&Thumb> {
        self.thumbs.get(pick)
    }

    /// How many thumbnails are drawn.
    pub fn drawn(&self) -> usize {
        self.thumbs.len()
    }
}

/// The tiles of one of our screens: its first state, or each.
fn ours_tiles(o: &Ours, item: &Item, all_states: bool) -> Vec<Tile> {
    let states = o.states.get(&item.name).map(Vec::as_slice).unwrap_or(&[]);
    let shown = if all_states { states } else { states.get(..1).unwrap_or(&[]) };
    shown
        .iter()
        .map(|s| Tile {
            pick: Pick::Native {
                screen: item.name.clone(),
                state: s.clone(),
            },
            label: if all_states {
                format!("{} / {s}", item.label)
            } else {
                item.label.clone()
            },
            name: format!("{}/{s}", item.name),
        })
        .collect()
}

impl UiScreens {
    /// The disc's retail screens as folders (none without a disc).
    pub fn retail_tree(&mut self) -> Option<Arc<RetailTree>> {
        if self.gallery.retail.is_none() {
            let tags = self.retail();
            if tags.is_empty() {
                return None;
            }
            let (tree, stripped) = browse::retail(&tags);
            self.gallery.retail = Some(Arc::new(RetailTree { tree, stripped }));
        }
        self.gallery.retail.clone()
    }

    /// Whether `at` names a folder that exists (retail needs the disc).
    pub fn has_folder(&mut self, at: &Browse) -> bool {
        match at.root {
            Root::Ours => browse::ours_tree().tree.find(&at.folder).is_some(),
            Root::Retail => self
                .retail_tree()
                .is_some_and(|r| r.tree.find(&at.folder).is_some()),
        }
    }

    /// Shows the grid of `at` in place of the picture (none: back to it).
    pub fn browse_to(&mut self, at: Option<Browse>) {
        self.gallery.at = at;
    }

    /// Everything in `at` and under it, as tiles by folder. Ours show
    /// their first state, or all of them with `states`; retail screens
    /// as the disc draws them.
    pub fn sections(&mut self, at: &Browse, states: bool) -> Vec<Section> {
        match at.root {
            Root::Ours => {
                let o = browse::ours_tree();
                let Some(f) = o.tree.find(&at.folder) else {
                    return Vec::new();
                };
                f.sections()
                    .into_iter()
                    .map(|(title, items)| Section {
                        title,
                        tiles: items.into_iter().flat_map(|i| ours_tiles(o, i, states)).collect(),
                    })
                    .collect()
            }
            Root::Retail => {
                let Some(r) = self.retail_tree() else {
                    return Vec::new();
                };
                let Some(f) = r.tree.find(&at.folder) else {
                    return Vec::new();
                };
                f.sections()
                    .into_iter()
                    .map(|(title, items)| Section {
                        title,
                        tiles: items
                            .into_iter()
                            .map(|i| Tile {
                                pick: Pick::retail(i.name.clone()),
                                label: i.label.clone(),
                                name: i.name.clone(),
                            })
                            .collect(),
                    })
                    .collect()
            }
        }
    }

    /// Draws the thumbnails of `wanted` that are not drawn yet, in order,
    /// until `budget` is spent (at least one, so the grid always moves).
    /// Returns how many it drew.
    pub fn draw_thumbs(&mut self, wanted: &[Pick], budget: Duration) -> usize {
        let start = Instant::now();
        let mut n = 0;
        for pick in wanted {
            if self.gallery.thumbs.contains_key(pick) {
                continue;
            }
            if n > 0 && start.elapsed() >= budget {
                break;
            }
            if self.gallery.thumbs.len() >= KEEP {
                self.gallery.thumbs.retain(|p, _| wanted.contains(p));
            }
            self.gallery.drawn += 1;
            let id = self.gallery.drawn;
            let thumb = match render_thumb(self.disc(), pick) {
                Ok((canvas, found, checked)) => Thumb {
                    id,
                    canvas: Some(canvas),
                    findings: found.len(),
                    found,
                    checked,
                    error: None,
                },
                Err(e) => Thumb {
                    id,
                    canvas: None,
                    findings: 0,
                    found: Vec::new(),
                    checked: false,
                    error: Some(format!("{e:#}")),
                },
            };
            self.gallery.thumbs.insert(pick.clone(), thumb);
            n += 1;
        }
        n
    }

    /// One contact sheet of `tiles` (drawn now, all of them), `columns`
    /// across, with where each tile sits.
    pub fn sheet(&mut self, tiles: &[Tile], columns: usize) -> (Canvas, Vec<SheetTile>) {
        let picks: Vec<Pick> = tiles.iter().map(|t| t.pick.clone()).collect();
        self.draw_thumbs(&picks, Duration::MAX);
        let cells: Vec<Cell> = tiles
            .iter()
            .map(|t| {
                let th = self.gallery.thumb(&t.pick);
                Cell {
                    label: t.label.clone(),
                    findings: th.map_or(0, |t| t.findings),
                    canvas: th.and_then(|t| t.canvas.as_ref()),
                }
            })
            .collect();
        let (canvas, rects) = contact_sheet(&cells, columns);
        let placed = tiles
            .iter()
            .zip(rects)
            .map(|(t, rect)| {
                let th = self.gallery.thumb(&t.pick);
                SheetTile {
                    pick: t.pick.clone(),
                    rect,
                    findings: th.map_or(0, |t| t.findings),
                    found: th.map_or_else(Vec::new, |t| t.found.clone()),
                    checked: th.is_some_and(|t| t.checked),
                    error: th.and_then(|t| t.error.clone()),
                }
            })
            .collect();
        (canvas, placed)
    }
}

/// Where a tile sits on the contact sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct SheetTile {
    pub pick: Pick,
    /// x, y, width, height of the picture, in sheet pixels.
    pub rect: [u32; 4],
    pub findings: usize,
    /// Each finding's line.
    pub found: Vec<String>,
    pub checked: bool,
    /// Why it did not draw (its cell is a dark red box).
    pub error: Option<String>,
}

/// Screen `pick` drawn as the picture draws it (no animation, no presses),
/// shrunk to fit [`THUMB`], with its findings and whether the checks
/// ran. This repeats [`UiScreens::draw_native`]'s drawing and checks so a
/// thumbnail never disturbs the page shown.
fn render_thumb(disc: Option<&Disc>, pick: &Pick) -> Result<(Canvas, Vec<String>, bool)> {
    match pick {
        Pick::Native { screen, state } => {
            let mut live =
                screens::live(screen).with_context(|| format!("no screen {screen:?}"))?;
            if !live.go(state) {
                bail!("{screen} has no state {state:?}");
            }
            let spec = live.spec();
            let from = match disc {
                Some(d) => preview::From::Disc(d),
                None => preview::From::Synthetic,
            };
            let src = from.source();
            let view = from.view(&spec, state)?;
            let found = boot::ui_kit::findings::of_state(&from, &spec, &view)?
                .findings
                .iter()
                .map(ToString::to_string)
                .collect();
            Ok((shrink(preview::render_view(&view, src)), found, true))
        }
        Pick::Retail { tag, sample } => {
            let d = disc.context("retail screens need the disc")?;
            let canvas = d.render_retail(&Retail {
                tag,
                pane: 0,
                text: &|_| None,
                cells: &[],
                footer: None,
                rows: None,
                sample: sample.as_deref(),
            })?;
            Ok((shrink(canvas), Vec::new(), false))
        }
    }
}

/// `c` averaged down by a whole factor until it fits [`THUMB`].
pub fn shrink(c: Canvas) -> Canvas {
    let k = c
        .width
        .div_ceil(THUMB[0])
        .max(c.height.div_ceil(THUMB[1]))
        .max(1);
    if k == 1 {
        return c;
    }
    let (w, h) = (c.width / k, c.height / k);
    let mut out = Canvas::new(w.max(1), h.max(1), [0, 0, 0, 255]);
    for y in 0..out.height {
        for x in 0..out.width {
            for ch in 0..4 {
                let mut sum = 0;
                for dy in 0..k {
                    for dx in 0..k {
                        sum += u32::from(
                            c.rgba[(((y * k + dy) * c.width + x * k + dx) * 4 + ch) as usize],
                        );
                    }
                }
                out.rgba[((y * out.width + x) * 4 + ch) as usize] = (sum / (k * k)) as u8;
            }
        }
    }
    out
}

/// A cell of the contact sheet.
pub struct Cell<'a> {
    pub label: String,
    pub findings: usize,
    /// None: the screen did not draw (a dark red box).
    pub canvas: Option<&'a Canvas>,
}

const GAP: u32 = 8;
const LABEL: u32 = 22;
/// [`Canvas::label`]'s advance.
const GLYPH: u32 = 8;

/// A contact sheet of `cells`, `columns` across: each under its label
/// (in [`Canvas::label`]'s pixel font, whatever the source) and a red
/// `!n` when it has findings. Also each picture's `[x, y, w, h]`.
pub fn contact_sheet(cells: &[Cell], columns: usize) -> (Canvas, Vec<[u32; 4]>) {
    let columns = columns.clamp(1, cells.len().max(1));
    let rows = cells.len().div_ceil(columns) as u32;
    let mut out = Canvas::new(
        columns as u32 * (THUMB[0] + GAP) + GAP,
        rows * (THUMB[1] + LABEL + GAP) + GAP,
        [40, 40, 40, 255],
    );
    let mut rects = Vec::new();
    for (i, cell) in cells.iter().enumerate() {
        let x = GAP + (i % columns) as u32 * (THUMB[0] + GAP);
        let y = GAP + (i / columns) as u32 * (THUMB[1] + LABEL + GAP);
        let badge = if cell.findings > 0 {
            format!("!{}", cell.findings)
        } else {
            String::new()
        };
        let room = (THUMB[0] / GLYPH) as usize - badge.len() - 1;
        let label: String = cell.label.chars().take(room).collect();
        out.label(&label, x + 2, y + 6, [230, 230, 230]);
        if !badge.is_empty() {
            let at = x + THUMB[0] - GLYPH * badge.len() as u32 - 2;
            out.label(&badge, at, y + 6, [255, 110, 100]);
        }
        let blank;
        let canvas = match cell.canvas {
            Some(c) => c,
            None => {
                blank = Canvas::new(THUMB[0], THUMB[1], [70, 30, 30, 255]);
                &blank
            }
        };
        out.blit(canvas, x, y + LABEL);
        rects.push([x, y + LABEL, canvas.width, canvas.height]);
    }
    (out, rects)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native(screen: &str, state: &str) -> Pick {
        Pick::Native {
            screen: screen.into(),
            state: state.into(),
        }
    }

    /// The first dotted segment of a tile's screen.
    fn head(t: &Tile) -> String {
        match &t.pick {
            Pick::Native { screen, .. } => screen.split('.').next().unwrap().to_owned(),
            Pick::Retail { .. } => String::new(),
        }
    }

    fn root() -> Browse {
        Browse::root(Root::Ours)
    }

    /// The root of ours shows each screen once (its first state), grouped
    /// by folder; all states shows a tile per state.
    #[test]
    fn the_root_lists_every_screen() {
        let mut s = UiScreens::default();
        let sections = s.sections(&root(), false);
        let tiles: Vec<&Tile> = sections.iter().flat_map(|s| &s.tiles).collect();
        assert_eq!(tiles.len(), screens::NAMES.len());
        assert!(tiles.iter().all(|t| matches!(t.pick, Pick::Native { .. })));
        let folder = &browse::ours_tree().tree.folders[0];
        let own = sections.iter().find(|s| s.title == folder.label).unwrap();
        assert!(own.tiles.iter().all(|t| head(t) == folder.id));
        let every: usize = s
            .sections(&root(), true)
            .iter()
            .map(|s| s.tiles.len())
            .sum();
        let states: usize = UiScreens::tree().iter().map(|(_, s)| s.len()).sum();
        assert_eq!(every, states);
        assert!(every > tiles.len());
    }

    /// A folder shows what is in it, and a folder that is not there shows
    /// nothing; retail has nothing without a disc.
    #[test]
    fn a_folder_shows_its_screens() {
        let mut s = UiScreens::default();
        let first = &browse::ours_tree().tree.folders[0];
        let forge = Browse {
            root: Root::Ours,
            folder: first.id.clone(),
        };
        assert!(s.has_folder(&forge));
        let sections = s.sections(&forge, false);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].tiles.iter().all(|t| head(t) == first.id));
        let nope = Browse {
            root: Root::Ours,
            folder: "nope".into(),
        };
        assert!(!s.has_folder(&nope));
        assert!(s.sections(&nope, false).is_empty());
        assert!(!s.has_folder(&Browse::root(Root::Retail)));
        assert!(s.sections(&Browse::root(Root::Retail), false).is_empty());
    }

    /// Thumbnails draw once, at most the thumbnail size, with their
    /// findings; a budget stops after the first that spends it.
    #[test]
    fn thumbnails_draw_once_within_a_budget() {
        let mut s = UiScreens::default();
        let wanted = [
            native("skulls", "focus-0"),
            native("account", "focus-1"),
            native("signin-code", "code"),
        ];
        assert_eq!(s.draw_thumbs(&wanted, Duration::ZERO), 1);
        assert!(s.gallery.thumb(&wanted[0]).is_some());
        assert!(s.gallery.thumb(&wanted[1]).is_none());
        assert_eq!(s.draw_thumbs(&wanted, Duration::from_secs(60)), 2);
        assert_eq!(s.draw_thumbs(&wanted, Duration::from_secs(60)), 0, "cached");
        let t = s.gallery.thumb(&wanted[2]).unwrap();
        let c = t.canvas.as_ref().unwrap();
        assert!(c.width <= THUMB[0] && c.height <= THUMB[1]);
        assert!(t.checked && t.error.is_none());
        // The page shown is not disturbed.
        assert!(s.pick.is_none() && s.summary().is_none());
    }

    /// A thumbnail finds what the picture finds.
    #[test]
    fn a_thumbnail_counts_the_pictures_findings() {
        let mut s = UiScreens::default();
        for (screen, state) in [("signin-code", "code"), ("skulls", "focus-0")] {
            s.select(native(screen, state)).unwrap();
            let shown = s.shot().unwrap().findings.len();
            s.draw_thumbs(&[native(screen, state)], Duration::from_secs(60));
            assert_eq!(s.gallery.thumb(&native(screen, state)).unwrap().findings, shown);
        }
    }

    /// A screen that will not draw leaves a thumbnail with the reason, and
    /// is not tried again; a new disc forgets the thumbnails.
    #[test]
    fn a_failed_screen_is_remembered_and_a_new_disc_forgets() {
        let mut s = UiScreens::default();
        let bad = [native("nope", "x"), Pick::retail("a\\b")];
        assert_eq!(s.draw_thumbs(&bad, Duration::from_secs(60)), 2);
        assert!(s.gallery.thumb(&bad[0]).unwrap().error.is_some());
        assert!(s.gallery.thumb(&bad[1]).unwrap().canvas.is_none());
        assert_eq!(s.draw_thumbs(&bad, Duration::from_secs(60)), 0);
        s.set_disc(Some(std::path::Path::new("/nonexistent/disc.iso")));
        assert_eq!(s.gallery.drawn(), 0);
    }

    #[test]
    fn shrinking_averages_down_to_the_thumbnail() {
        let mut big = Canvas::new(640, 480, [10, 20, 30, 255]);
        big.rgba[0..4].copy_from_slice(&[250, 250, 250, 255]);
        let small = shrink(big);
        assert_eq!([small.width, small.height], [320, 240]);
        assert_eq!(&small.rgba[0..4], &[70, 77, 85, 255]);
        let tiny = shrink(Canvas::new(100, 50, [1, 2, 3, 255]));
        assert_eq!([tiny.width, tiny.height], [100, 50]);
        let wide = shrink(Canvas::new(1280, 480, [0, 0, 0, 255]));
        assert_eq!([wide.width, wide.height], [320, 120]);
    }

    /// The sheet lays its cells in a grid and says where each sits.
    #[test]
    fn the_contact_sheet_places_its_tiles() {
        let a = Canvas::new(320, 240, [200, 0, 0, 255]);
        let b = Canvas::new(320, 240, [0, 200, 0, 255]);
        let cells = [
            Cell { label: "a".into(), findings: 0, canvas: Some(&a) },
            Cell { label: "b".into(), findings: 3, canvas: Some(&b) },
            Cell { label: "c".into(), findings: 0, canvas: None },
        ];
        let (sheet, rects) = contact_sheet(&cells, 2);
        assert_eq!(sheet.width, 2 * 328 + 8);
        assert_eq!(sheet.height, 2 * 270 + 8);
        assert_eq!(rects, [[8, 30, 320, 240], [336, 30, 320, 240], [8, 300, 320, 240]]);
        let px = |x: u32, y: u32| sheet.rgba[((y * sheet.width + x) * 4) as usize..][..3].to_vec();
        assert_eq!(px(10, 40), [200, 0, 0]);
        assert_eq!(px(340, 40), [0, 200, 0]);
        assert_eq!(px(10, 310), [70, 30, 30], "the failed cell");
        // Columns never exceed the cells; an empty sheet is just a margin.
        assert_eq!(contact_sheet(&cells, 9).0.width, 3 * 328 + 8);
        assert_eq!(contact_sheet(&[], 4).1.len(), 0);
    }

    /// The sheet of a folder: one cell per tile, in the grid's order.
    #[test]
    fn a_sheet_draws_the_tiles_it_is_given() {
        let mut s = UiScreens::default();
        let forge = Browse {
            root: Root::Ours,
            folder: browse::ours_tree().tree.folders[0].id.clone(),
        };
        let tiles: Vec<Tile> = s.sections(&forge, false).into_iter().flat_map(|s| s.tiles).take(3).collect();
        let (canvas, placed) = s.sheet(&tiles, 2);
        assert_eq!(placed.len(), tiles.len());
        assert_eq!(placed[0].pick, tiles[0].pick);
        assert!(placed.iter().all(|p| p.checked && p.error.is_none()));
        assert!(placed.iter().all(|p| p.rect[0] + p.rect[2] <= canvas.width));
        assert!(placed.iter().all(|p| p.rect[1] + p.rect[3] <= canvas.height));
    }
}
