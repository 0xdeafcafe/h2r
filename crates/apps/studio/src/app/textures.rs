//! The Textures tool on the app (docs/editor/modeling.md, "Texture
//! editing"): the open map's bitmaps decoded into `bitmap-edit` canvases,
//! the edits made to them with undo, and PNG in and out.
//!
//! A bitmap is decoded when it is first selected or named
//! (`assets::decode_image`, every face and depth slice at mip 0) and kept,
//! with its edits, until another map opens. Edits stay in the editor until
//! a save: the map's save ([`App::push_bitmap_changes`] runs first) encodes
//! what changed (`assets::write`, docs/editor/modeling.md, "Texture
//! editing") and hands it to the scenario session, which writes it with the
//! map: bytes patched in place for pixels in this map, new raw blocks for
//! pixels in a shared cache or for a new size or format.
//!
//! Every edit is an [`Action::Bitmap`], dispatched by the window and
//! performed by the API alike. A drag in the canvas is `StrokeBegin`,
//! `StrokeTo` and `StrokeEnd`: many touched rectangles (each a dirty
//! rectangle for the live preview and the later live push), one undo step.
//! The undo stack is across bitmaps: `history` names which bitmap each step
//! belongs to.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use assets::encode::{Dirty, PixelRect, Planes, Span};
use assets::write::{Class, Edit, Reshape, plan_image};
use bitmap_edit::{
    Applied, Bitmap, BitmapOp, Brush, Canvas, Channels, LayerId, Rect, decode_png, encode_png,
    resample,
};
use map::TagId;
use scenario_edit::{GeometryChanges, RawAppend, RawPatch};
use tags::{PixelFormat, TagData};

use super::App;

/// Why an export is refused inside a Git checkout.
const GIT_REFUSAL: &str =
    "maps and renders from your disc stay out of repositories; pick a path outside it";

/// The canvas tools (docs/editor/modeling.md, the keys: B, E, G, I; the
/// selection is a rectangle that fill, adjust and import are limited to).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaintTool {
    #[default]
    Brush,
    Eraser,
    Fill,
    Eyedropper,
    Select,
}

impl PaintTool {
    pub const ALL: [PaintTool; 5] = [
        PaintTool::Brush,
        PaintTool::Eraser,
        PaintTool::Fill,
        PaintTool::Eyedropper,
        PaintTool::Select,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PaintTool::Brush => "Brush",
            PaintTool::Eraser => "Eraser",
            PaintTool::Fill => "Fill",
            PaintTool::Eyedropper => "Eyedropper",
            PaintTool::Select => "Select",
        }
    }
}

/// One image of a bitmap tag, as it was read.
#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub format: String,
    /// `D2`, `D3` or `Cube`.
    pub kind: String,
    pub mips: u16,
    /// Where LOD 0's pixels are: `local`, `shared`, `mainmenu`...
    pub pixels: String,
    pub layers: Vec<LayerId>,
    /// Why it cannot be edited, or `None`.
    pub locked: Option<String>,
    /// Why it could not be decoded, or `None`.
    pub error: Option<String>,
    /// The image record as the map has it: what its write is planned from.
    pub source: Option<tags::Image>,
}

impl ImageInfo {
    /// How saving the image would write: `none` (read only), `in_place`
    /// (every stored LOD is in this map) or `appended` (a LOD in a shared
    /// cache is copied into this map).
    pub fn write_class(&self) -> &'static str {
        if self.locked.is_some() || self.error.is_some() {
            return "none";
        }
        let shared = self.source.as_ref().is_some_and(|i| {
            i.lods
                .iter()
                .flatten()
                .any(|r| r.pointer.file != map::ResourceFile::Local)
        });
        let class = if shared {
            Class::Appended
        } else {
            Class::InPlace
        };
        class.name()
    }
}

/// A bitmap tag opened for editing.
pub struct OpenBitmap {
    pub tag: TagId,
    pub name: String,
    pub images: Vec<ImageInfo>,
    pub bitmap: Bitmap,
    /// The virtual address of the tag's first `BitmapImage` record.
    pub images_at: u32,
}

/// The Textures tool's state.
pub struct Textures {
    /// The bitmaps decoded so far, with their edits.
    pub open: HashMap<TagId, OpenBitmap>,
    /// The bitmap on the canvas, its image, face and depth slice.
    pub selected: Option<TagId>,
    pub image: usize,
    pub face: u32,
    pub slice: u32,
    pub tool: PaintTool,
    /// Size, hardness, opacity and spacing; the colour and the eraser
    /// come from `fg` and `tool` (`paint_brush`).
    pub brush: Brush,
    pub fg: [u8; 4],
    pub bg: [u8; 4],
    pub channels: Channels,
    /// Fill's tolerance, 0 to 1.
    pub tolerance: f32,
    /// The rectangle fill, adjust and import are limited to.
    pub selection: Option<Rect>,
    /// Canvas zoom in screen pixels per image pixel.
    pub zoom: f32,
    /// Changes when a bitmap is decoded again or dropped, so a texture
    /// uploaded from the old pixels is not taken for the new.
    pub generation: u64,
    /// Substring filter of the bitmap list.
    pub filter: String,
    /// The bitmap a drag is painting into.
    pub stroke: Option<TagId>,
    /// Which bitmap each undo step belongs to, oldest first, and the same
    /// for redo.
    history: Vec<TagId>,
    redo: Vec<TagId>,
    /// The last edit's outcome, for the API.
    pub last: Option<(TagId, Applied)>,
    /// Where the last export went.
    pub last_export: Option<PathBuf>,
    for_rev: Option<u64>,
    /// A new size or format asked for an image (`bitmap.reshape`), which
    /// the save writes whole. Not an undo step: [`BitmapAction::Revert`]
    /// drops it.
    pub reshape: HashMap<(TagId, usize), Reshape>,
    reshape_rev: u64,
    /// What each bitmap was when its changes were last handed to the save
    /// ([`Textures::stamp`]): the tool is unsaved while the bitmap differs
    /// from it, or the map holds changes it has not written.
    pushed: HashMap<TagId, (u64, u64)>,
}

impl Default for Textures {
    fn default() -> Self {
        Textures {
            open: HashMap::new(),
            selected: None,
            image: 0,
            face: 0,
            slice: 0,
            tool: PaintTool::Brush,
            brush: Brush::default(),
            fg: [255, 255, 255, 255],
            bg: [0, 0, 0, 255],
            channels: Channels::ALL,
            tolerance: 0.1,
            selection: None,
            zoom: 4.0,
            generation: 0,
            filter: String::new(),
            stroke: None,
            history: Vec::new(),
            redo: Vec::new(),
            last: None,
            last_export: None,
            for_rev: None,
            reshape: HashMap::new(),
            reshape_rev: 0,
            pushed: HashMap::new(),
        }
    }
}

impl Textures {
    /// Any open bitmap differs from how it was read, or has a new size or
    /// format asked for.
    pub fn is_edited(&self) -> bool {
        self.open.values().any(|o| self.edited(o))
    }

    /// Bitmap `o` differs from how it was read.
    pub fn edited(&self, o: &OpenBitmap) -> bool {
        o.bitmap.is_edited() || self.reshape.keys().any(|(t, _)| *t == o.tag)
    }

    /// An edit counter for a bitmap: its pixels' revision and the reshapes'.
    fn stamp(&self, o: &OpenBitmap) -> (u64, u64) {
        (o.bitmap.rev(), self.reshape_rev)
    }

    /// The tool has edits the map's save has not been handed, or the map
    /// has changes it has not written (`map_dirty`).
    pub fn is_unsaved(&self, map_dirty: bool) -> bool {
        self.open.values().any(|o| {
            self.edited(o) && (map_dirty || self.pushed.get(&o.tag) != Some(&self.stamp(o)))
        })
    }

    pub fn can_undo(&self) -> bool {
        self.stroke.is_none() && !self.history.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        self.stroke.is_none() && !self.redo.is_empty()
    }

    /// The bitmap the newest undo step belongs to.
    pub fn last_step(&self) -> Option<TagId> {
        self.history.last().copied()
    }

    /// The brush a stroke paints with: the settings, the foreground colour
    /// and the eraser when that tool is active.
    pub fn paint_brush(&self) -> Brush {
        let mut b = self.brush.clone();
        b.color = self.fg;
        b.erase = self.tool == PaintTool::Eraser;
        b.channels = self.channels;
        b
    }

    /// The layer on the canvas: the selected bitmap, image, face and
    /// slice.
    pub fn layer(&self) -> Option<(TagId, LayerId)> {
        let tag = self.selected?;
        Some((tag, LayerId::new(self.image as u32, self.face, self.slice)))
    }

    /// Drop every open bitmap and its edits (they are decoded again when
    /// next needed).
    pub fn discard_edits(&mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        self.generation += 1;
        self.open.clear();
        self.reshape.clear();
        self.pushed.clear();
        self.selected = None;
        self.image = 0;
        self.face = 0;
        self.slice = 0;
        self.selection = None;
        self.stroke = None;
        self.history.clear();
        self.redo.clear();
        self.last = None;
    }
}

/// One Textures command.
#[derive(Clone, Debug, PartialEq)]
pub enum BitmapAction {
    /// Show an image of a bitmap (decoding the bitmap first).
    Select {
        tag: TagId,
        image: usize,
    },
    /// Show another face or depth slice of the image.
    Layer {
        face: u32,
        slice: u32,
    },
    /// An edit as a value, one undo step.
    Apply {
        tag: TagId,
        op: BitmapOp,
    },
    /// A drag in the canvas: begin, extend and end. One undo step.
    StrokeBegin {
        tag: TagId,
        layer: LayerId,
    },
    StrokeTo {
        x: f32,
        y: f32,
    },
    StrokeEnd,
    /// Replace a layer, or a rectangle of it, from a PNG.
    Import {
        tag: TagId,
        layer: LayerId,
        rect: Option<Rect>,
        path: PathBuf,
    },
    /// Write a layer's mip as a PNG (never into a Git checkout).
    Export {
        tag: TagId,
        layer: LayerId,
        mip: u32,
        path: PathBuf,
    },
    Undo,
    Redo,
    /// Drop a bitmap's edits (it is decoded again when next needed).
    Revert(TagId),
    /// Ask for a new size and/or format for an image; the save writes it
    /// whole. `None` keeps that one.
    Reshape {
        tag: TagId,
        image: usize,
        width: Option<u32>,
        height: Option<u32>,
        format: Option<PixelFormat>,
    },
}

impl App {
    /// Drop the tool's bitmaps when another map (or none) opens.
    pub(crate) fn sync_textures(&mut self) {
        let rev = Some(self.session.world_rev());
        if self.textures.for_rev != rev {
            self.textures.clear();
            self.textures.for_rev = rev;
        }
    }

    /// The open map's bitmap tags, by id and path.
    pub fn texture_tags(&mut self) -> Vec<(TagId, String)> {
        self.sync_tags();
        self.tags
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"bitm")
            .map(|t| (t.id, t.name.clone()))
            .collect()
    }

    /// A bitmap named by tag id (`0x…`), by its path, by the end of its
    /// path (`floor` for `levels\test\bitmaps\floor`) or, when that is
    /// one tag, by a part of it.
    pub fn resolve_bitmap(&mut self, key: &str) -> Result<TagId, String> {
        let all = self.texture_tags();
        if all.is_empty() {
            return Err(if self.session.is_open() {
                "the open map has no bitmaps".to_owned()
            } else {
                "no map is open".to_owned()
            });
        }
        if let Ok(n) = u32::from_str_radix(key.strip_prefix("0x").unwrap_or(key), 16)
            && let Some((id, _)) = all.iter().find(|(id, _)| id.0 == n)
        {
            return Ok(*id);
        }
        let key = key.replace('/', "\\");
        if let Some((id, _)) = all.iter().find(|(_, n)| *n == key) {
            return Ok(*id);
        }
        let tail = format!("\\{key}");
        let mut m: Vec<&(TagId, String)> = all.iter().filter(|(_, n)| n.ends_with(&tail)).collect();
        if m.is_empty() {
            m = all.iter().filter(|(_, n)| n.contains(&key)).collect();
        }
        match m.len() {
            0 => Err(format!("no bitmap is named {key:?}")),
            1 => Ok(m[0].0),
            n => Err(format!(
                "{key:?} names {n} bitmaps ({}, ...): name more of the path or give the tag id",
                m[0].1
            )),
        }
    }

    /// Decode bitmap `tag` and keep it, unless it is already open.
    pub(crate) fn ensure_bitmap(&mut self, tag: TagId) -> Result<(), String> {
        self.sync_textures();
        if self.textures.open.contains_key(&tag) {
            return Ok(());
        }
        let open = self.load_bitmap(tag)?;
        self.textures.generation += 1;
        self.textures.open.insert(tag, open);
        Ok(())
    }

    fn load_bitmap(&mut self, tag: TagId) -> Result<OpenBitmap, String> {
        if self.tag_browse.is_some() {
            return Err(
                "a shared cache is open in the Tags tool: close it to edit the map's bitmaps"
                    .to_owned(),
            );
        }
        self.sync_tags();
        let name = self
            .tags
            .tag(tag)
            .filter(|t| t.group.0 == *b"bitm")
            .map(|t| t.name.clone())
            .ok_or_else(|| format!("{:#010x} is not a bitmap of the open map", tag.0))?;
        let source = self.session.source().cloned().ok_or("no map is open")?;
        let mut map = source.open().map_err(|e| e.to_string())?;
        let mut files = self.shown_raw_files()?;
        let decoded = tags::DecodedTag::read(&mut map, tag).map_err(|e| e.to_string())?;
        let TagData::Bitmap(b) = decoded.data else {
            return Err(format!("{name} is not a bitmap"));
        };
        let mut bitmap = Bitmap::new();
        let mut images = Vec::new();
        for (i, img) in b.images.iter().enumerate() {
            let mut info = ImageInfo {
                index: i,
                width: u32::from(img.dimensions[0]),
                height: u32::from(img.dimensions[1]),
                depth: u32::from(img.dimensions[2]).max(1),
                format: format!("{:?}", img.format),
                kind: format!("{:?}", img.kind),
                mips: img.mip_count,
                pixels: img.lods.iter().flatten().next().map_or_else(
                    || "none".to_owned(),
                    |r| format!("{:?}", r.pointer.file).to_lowercase(),
                ),
                layers: Vec::new(),
                locked: locked_reason(img.format),
                error: None,
                source: Some(img.clone()),
            };
            match assets::decode_image(img, &mut files) {
                Err(e) => info.error = Some(e.to_string()),
                Ok(d) => {
                    if d.lod != 0 && info.locked.is_none() {
                        info.locked = Some(format!(
                            "only a lower mip (LOD {}) of the image is stored in this map",
                            d.lod
                        ));
                    }
                    for l in d.levels.iter().filter(|l| l.level == 0) {
                        let plane = l.width as usize * l.height as usize * 4;
                        for s in 0..l.depth.max(1) {
                            let at = s as usize * plane;
                            let Some(px) = l.rgba.get(at..at + plane) else {
                                info.error = Some("a depth slice is short".to_owned());
                                continue;
                            };
                            match Canvas::from_rgba(l.width, l.height, px) {
                                Ok(c) => {
                                    let id = LayerId::new(i as u32, l.face, s);
                                    bitmap.add_layer(id, c, info.locked.clone());
                                    info.layers.push(id);
                                }
                                Err(e) => info.error = Some(e.to_string()),
                            }
                        }
                    }
                }
            }
            images.push(info);
        }
        Ok(OpenBitmap {
            tag,
            name,
            images,
            bitmap,
            images_at: b.header.images.address,
        })
    }

    /// Hand the map's save what the Textures tool has changed: every edited
    /// bitmap's images encoded (`assets::write::plan_image`) and set as the
    /// session's bitmap changes, replacing whatever it held. An edit that
    /// cannot be written (a locked image, a size or format the file cannot
    /// take) is the session's refusal, so the save fails by name before it
    /// writes. Nothing edited hands an empty set (undo back to the source
    /// is the source again). Runs before every map save.
    pub(crate) fn push_bitmap_changes(&mut self) {
        self.sync_textures();
        if !self.session.is_open() {
            return;
        }
        let changes = self.bitmap_changes();
        let stamps: Vec<_> = self
            .textures
            .open
            .values()
            .map(|o| (o.tag, self.textures.stamp(o)))
            .collect();
        self.textures.pushed = stamps.into_iter().collect();
        let r = self.session.set_bitmap_changes(changes);
        self.ok_or_log(r);
    }

    /// What [`Self::push_bitmap_changes`] hands the save.
    pub fn bitmap_changes(&self) -> GeometryChanges {
        let mut c = GeometryChanges::default();
        let mut tags: Vec<TagId> = self
            .textures
            .open
            .values()
            .filter(|o| self.textures.edited(o))
            .map(|o| o.tag)
            .collect();
        if tags.is_empty() {
            return c;
        }
        tags.sort_by_key(|t| t.0);
        let mut files = match self.shown_raw_files() {
            Ok(f) => f,
            Err(e) => {
                c.refusal = Some(e);
                return c;
            }
        };
        for tag in tags {
            let open = &self.textures.open[&tag];
            for info in &open.images {
                let reshape = self.textures.reshape.get(&(tag, info.index)).copied();
                match plan_for(open, info, reshape, &mut files) {
                    Ok(None) => {}
                    Ok(Some(plan)) => {
                        c.raw_patches.extend(plan.raw.into_iter().map(|w| RawPatch {
                            block: w.block,
                            block_size: w.block_size,
                            offset: w.offset,
                            bytes: w.bytes,
                        }));
                        c.raw_appends
                            .extend(plan.appended.into_iter().map(|a| RawAppend {
                                bytes: a.bytes,
                                align: a.align,
                                pointer_space: None,
                                pointer_address: a.pointer_address,
                            }));
                        c.patches.extend(plan.tag);
                    }
                    Err(e) => {
                        c.refusal = Some(format!("{}: {e}", open.name));
                        return c;
                    }
                }
            }
        }
        c
    }

    /// The stored bytes that changing `dirty` (rectangles of mip 0 pixels,
    /// by layer) of image `image` of bitmap `tag` changes, as the encoder
    /// writes them: byte spans per LOD, the same a save patches in place
    /// and a live push sends. The image keeps its size and format here;
    /// nothing is written.
    pub fn bitmap_tile_spans(
        &mut self,
        tag: TagId,
        image: usize,
        dirty: &[(LayerId, Rect)],
    ) -> Result<Vec<Span>, String> {
        self.ensure_bitmap(tag)?;
        let open = &self.textures.open[&tag];
        let info = open
            .images
            .get(image)
            .ok_or_else(|| format!("the bitmap has no image {image}"))?;
        if let Some(why) = info.locked.as_ref().or(info.error.as_ref()) {
            return Err(format!("image {image} is not written: {why}"));
        }
        let src = info.source.as_ref().ok_or("the image has no record")?;
        let (data, depth) = plane_data(open, info, None)?;
        let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
        let planes = Planes {
            width: u32::from(src.dimensions[0]),
            height: u32::from(src.dimensions[1]),
            depth,
            planes: &refs,
        };
        let dirty: Vec<Dirty> = dirty
            .iter()
            .filter(|(l, _)| l.image as usize == image)
            .map(|(l, r)| Dirty {
                face: l.face,
                slice: l.slice,
                rect: PixelRect::new(r.x, r.y, r.w, r.h),
            })
            .collect();
        assets::encode::encode_tiles(src, &planes, &dirty).map_err(|e| e.to_string())
    }

    pub(super) fn run_bitmap(&mut self, action: BitmapAction) {
        self.sync_textures();
        if let Err(e) = self.bitmap_action(action) {
            self.error(format!("textures: {e}"));
        }
    }

    fn open_mut(&mut self, tag: TagId) -> Result<&mut OpenBitmap, String> {
        self.textures
            .open
            .get_mut(&tag)
            .ok_or_else(|| format!("{:#010x} is not open", tag.0))
    }

    fn note_edit(&mut self, tag: TagId, applied: Applied) {
        if !applied.dirty.is_empty() {
            self.textures.history.push(tag);
            self.textures.redo.clear();
        }
        self.textures.last = Some((tag, applied));
    }

    pub(crate) fn bitmap_action(&mut self, action: BitmapAction) -> Result<(), String> {
        match action {
            BitmapAction::Select { tag, image } => {
                self.ensure_bitmap(tag)?;
                let n = self.open_mut(tag)?.images.len();
                self.textures.selected = Some(tag);
                self.textures.image = image.min(n.saturating_sub(1));
                self.textures.face = 0;
                self.textures.slice = 0;
                self.textures.selection = None;
            }
            BitmapAction::Layer { face, slice } => {
                self.textures.face = face;
                self.textures.slice = slice;
            }
            BitmapAction::Apply { tag, op } => {
                self.ensure_bitmap(tag)?;
                let open = self.open_mut(tag)?;
                let applied = open.bitmap.apply(&op).map_err(|e| e.to_string())?;
                let (name, dirty) = (open.name.clone(), applied.dirty);
                self.note_edit(tag, applied);
                self.info(format!(
                    "{name}: changed {}x{} pixels at {},{}",
                    dirty.w, dirty.h, dirty.x, dirty.y
                ));
            }
            BitmapAction::StrokeBegin { tag, layer } => {
                self.ensure_bitmap(tag)?;
                let brush = self.textures.paint_brush();
                self.open_mut(tag)?
                    .bitmap
                    .begin_stroke(layer, brush)
                    .map_err(|e| e.to_string())?;
                self.textures.stroke = Some(tag);
            }
            BitmapAction::StrokeTo { x, y } => {
                // A drag whose begin was refused has no stroke: nothing to extend.
                let Some(tag) = self.textures.stroke else {
                    return Ok(());
                };
                self.open_mut(tag)?
                    .bitmap
                    .stroke_to(x, y)
                    .map_err(|e| e.to_string())?;
            }
            BitmapAction::StrokeEnd => {
                if let Some(tag) = self.textures.stroke.take()
                    && let Some(applied) = self
                        .textures
                        .open
                        .get_mut(&tag)
                        .and_then(|o| o.bitmap.end_stroke())
                {
                    self.note_edit(tag, applied);
                }
            }
            BitmapAction::Import {
                tag,
                layer,
                rect,
                path,
            } => {
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let (width, height, rgba) =
                    decode_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                return self.bitmap_action(BitmapAction::Apply {
                    tag,
                    op: BitmapOp::Import {
                        layer,
                        rect,
                        width,
                        height,
                        rgba,
                    },
                });
            }
            BitmapAction::Export {
                tag,
                layer,
                mip,
                path,
            } => self.export_bitmap(tag, layer, mip, &path)?,
            BitmapAction::Undo => {
                if !self.textures_undo(true) {
                    self.info("nothing to undo");
                }
            }
            BitmapAction::Redo => {
                if !self.textures_undo(false) {
                    self.info("nothing to redo");
                }
            }
            BitmapAction::Reshape {
                tag,
                image,
                width,
                height,
                format,
            } => {
                self.ensure_bitmap(tag)?;
                let open = self.open_mut(tag)?;
                let info = open
                    .images
                    .get(image)
                    .ok_or_else(|| format!("the bitmap has no image {image}"))?;
                if let Some(why) = info.locked.as_ref().or(info.error.as_ref()) {
                    return Err(format!("image {image} is not written: {why}"));
                }
                let src = info.source.clone().ok_or("the image has no record")?;
                let (w, h) = (
                    width.unwrap_or(u32::from(src.dimensions[0])),
                    height.unwrap_or(u32::from(src.dimensions[1])),
                );
                let want = Reshape {
                    width: w,
                    height: h,
                    format: format.unwrap_or(src.format),
                };
                // Refuse now what the save would refuse later.
                assets::write::reshaped(&src, want).map_err(|e| e.to_string())?;
                bitmap_edit::check_size(w, h).map_err(|e| e.to_string())?;
                let same = want.width == u32::from(src.dimensions[0])
                    && want.height == u32::from(src.dimensions[1])
                    && want.format == src.format;
                if same {
                    self.textures.reshape.remove(&(tag, image));
                } else {
                    self.textures.reshape.insert((tag, image), want);
                }
                self.textures.reshape_rev += 1;
                self.info(format!(
                    "image {image}: {w}x{h} {:?} when saved",
                    want.format
                ));
            }
            BitmapAction::Revert(tag) => {
                if self.textures.stroke == Some(tag) {
                    self.textures.stroke = None;
                }
                self.textures.generation += 1;
                self.textures.open.remove(&tag);
                self.textures.reshape.retain(|(t, _), _| *t != tag);
                self.textures.pushed.remove(&tag);
                self.textures.history.retain(|t| *t != tag);
                self.textures.redo.retain(|t| *t != tag);
                if self.textures.selected == Some(tag) {
                    self.textures.selected = None;
                }
            }
        }
        Ok(())
    }

    fn export_bitmap(
        &mut self,
        tag: TagId,
        layer: LayerId,
        mip: u32,
        path: &Path,
    ) -> Result<(), String> {
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        if let Some(repo) = super::ui_screens::git_checkout(dir) {
            return Err(format!(
                "refused: {} is inside the Git checkout {}: {GIT_REFUSAL}",
                path.display(),
                repo.display()
            ));
        }
        self.ensure_bitmap(tag)?;
        let open = self.open_mut(tag)?;
        let l = open
            .bitmap
            .layer(layer)
            .ok_or_else(|| format!("the bitmap has no layer {layer}"))?;
        let (w, h) = (l.canvas.width(), l.canvas.height());
        let mut rgba = l.canvas.to_rgba();
        let (mut ow, mut oh) = (w, h);
        if mip > 0 {
            let m = mip.min(15);
            ow = (w >> m).max(1);
            oh = (h >> m).max(1);
            rgba = resample(&rgba, w, h, ow, oh).map_err(|e| e.to_string())?;
        }
        let png = encode_png(ow, oh, &rgba).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))?;
        self.textures.last_export = Some(path.to_path_buf());
        self.info(format!("exported {ow}x{oh} to {}", path.display()));
        Ok(())
    }

    /// Undo (or redo) the last Textures edit, whichever bitmap it was in.
    pub(crate) fn textures_undo(&mut self, undo: bool) -> bool {
        self.sync_textures();
        if self.textures.stroke.is_some() {
            return false;
        }
        let tag = if undo {
            self.textures.history.pop()
        } else {
            self.textures.redo.pop()
        };
        let Some(tag) = tag else {
            return false;
        };
        let applied = self.textures.open.get_mut(&tag).and_then(|o| {
            if undo {
                o.bitmap.undo()
            } else {
                o.bitmap.redo()
            }
        });
        let Some(applied) = applied else {
            return false;
        };
        if undo {
            self.textures.redo.push(tag);
        } else {
            self.textures.history.push(tag);
        }
        self.textures.last = Some((tag, applied));
        true
    }
}

/// The pixels a layer changed since it was read, as one tight rectangle
/// per changed tile (the bounding box of the differing pixels), so a
/// block is re-encoded only where a pixel really changed.
pub(crate) fn tight_changes(l: &bitmap_edit::Layer) -> Vec<Rect> {
    let mut out = Vec::new();
    for (tx, ty) in l.changed_tiles() {
        let r = l.canvas.tile_rect(tx, ty);
        let (_, now) = l.canvas.read_rect(r);
        let (_, was) = l.source.read_rect(r);
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (i, (a, b)) in now.chunks_exact(4).zip(was.chunks_exact(4)).enumerate() {
            if a != b {
                let (x, y) = (r.x + i as u32 % r.w, r.y + i as u32 / r.w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        if x1 > x0 {
            out.push(Rect::new(x0, y0, x1 - x0, y1 - y0));
        }
    }
    out
}

/// An image's layers as RGBA planes in `(face, slice)` order, resampled to
/// the new size when `reshape` has one, and the volume depth.
fn plane_data(
    open: &OpenBitmap,
    info: &ImageInfo,
    reshape: Option<Reshape>,
) -> Result<(Vec<Vec<u8>>, u32), String> {
    let src = info.source.as_ref().ok_or("the image has no record")?;
    let mut layers: Vec<&bitmap_edit::Layer> = info
        .layers
        .iter()
        .filter_map(|id| open.bitmap.layer(*id))
        .collect();
    layers.sort_by_key(|l| (l.id.face, l.id.slice));
    let depth = if src.kind == tags::TextureKind::D3 {
        u32::from(src.dimensions[2]).max(1)
    } else {
        1
    };
    let mut data = Vec::with_capacity(layers.len());
    for l in layers {
        let (lw, lh) = (l.canvas.width(), l.canvas.height());
        let rgba = l.canvas.to_rgba();
        data.push(match reshape {
            Some(r) if (r.width, r.height) != (lw, lh) => {
                resample(&rgba, lw, lh, r.width, r.height).map_err(|e| e.to_string())?
            }
            _ => rgba,
        });
    }
    Ok((data, depth))
}

/// The write of one image of an open bitmap: `None` when nothing about it
/// changed.
fn plan_for(
    open: &OpenBitmap,
    info: &ImageInfo,
    reshape: Option<Reshape>,
    files: &mut assets::ResourceFiles,
) -> Result<Option<assets::write::Plan>, String> {
    let layers: Vec<&bitmap_edit::Layer> = info
        .layers
        .iter()
        .filter_map(|id| open.bitmap.layer(*id))
        .collect();
    if reshape.is_none() && !layers.iter().any(|l| l.is_edited()) {
        return Ok(None);
    }
    if let Some(why) = info.locked.as_ref().or(info.error.as_ref()) {
        return Err(format!("image {}: not written: {why}", info.index));
    }
    let src = info.source.as_ref().ok_or("the image has no record")?;
    let (data, depth) = plane_data(open, info, reshape)?;
    let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    let (width, height) = reshape.map_or(
        (u32::from(src.dimensions[0]), u32::from(src.dimensions[1])),
        |r| (r.width, r.height),
    );
    let dirty: Vec<Dirty> = layers
        .iter()
        .flat_map(|l| {
            tight_changes(l).into_iter().map(move |r| Dirty {
                face: l.id.face,
                slice: l.id.slice,
                rect: PixelRect::new(r.x, r.y, r.w, r.h),
            })
        })
        .collect();
    let edit = Edit {
        image: src,
        image_at: open.images_at + 0x74 * info.index as u32,
        planes: Planes {
            width,
            height,
            depth,
            planes: &refs,
        },
        dirty: &dirty,
        reshape,
    };
    plan_image(&edit, files)
        .map(Some)
        .map_err(|e| format!("image {}: {e}", info.index))
}

/// Why a format's image is shown but not edited, or `None`.
fn locked_reason(format: PixelFormat) -> Option<String> {
    match format {
        PixelFormat::P8 | PixelFormat::P8Bump | PixelFormat::V8U8 | PixelFormat::G8B8 => Some(
            format!("{format:?} is data, not colour: its channels are not decoded as a picture"),
        ),
        _ => None,
    }
}
