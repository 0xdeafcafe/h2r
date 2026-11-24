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
//! belongs to, and whether it changed pixels or the bitmap's shape (an
//! image added, an image resized, a sequence added). Each step is numbered
//! among the steps of its bitmap's document (`TagDoc::next_step`), so a
//! bitmap's tab undoes its own in the order they were made beside the
//! bitmap's own writes (docs/editor/studio-flow.md, "Shared patterns").
//!
//! A bitmap that gained images or sequences no longer fits where it is
//! stored: the save writes the whole tag again at the end of the metadata
//! and points its index entry there (`tags::bitmap_grow`,
//! `scenario_edit::TagMove`, docs/editor/modeling.md, "Adding images"),
//! with every image record's writes made in the new copy.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use assets::encode::{Dirty, PixelRect, Planes, Span};
use assets::write::{Class, Edit, Reshape, plan_image};
use bitmap_edit::{
    Applied, Bitmap, BitmapOp, Brush, Canvas, Channels, LayerId, Rect, decode_png, encode_png,
    resample,
};
use map::TagId;
use scenario_edit::{GeometryChanges, MovedRaw, RawAppend, RawPatch, TagMove};
use tags::bitmap_grow::{self, IMAGE_LEN, Parts, Sequence, SpriteView};
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
    /// For an added image, the record it starts as.
    pub source: Option<tags::Image>,
    /// `Some` for an image added in Studio, not yet in the file.
    pub added: Option<Added>,
}

/// An image added in Studio: the image it was made from and its record
/// (`tags::bitmap_grow::added_image`, at the size and format asked).
#[derive(Clone, Debug)]
pub struct Added {
    pub from: usize,
    pub record: Vec<u8>,
}

impl ImageInfo {
    /// How saving the image would write: `none` (read only), `in_place`
    /// (every stored LOD is in this map), `appended` (a LOD in a shared
    /// cache is copied into this map) or `new` (an added image: written
    /// whole, its record in the moved tag).
    pub fn write_class(&self) -> &'static str {
        if self.locked.is_some() || self.error.is_some() {
            return "none";
        }
        if self.added.is_some() {
            return "new";
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
    /// The tag's records as stored, which a save that moves it lays out
    /// again, or why it cannot move (`tags::bitmap_grow::Parts::split`).
    pub parts: Result<Parts, String>,
    /// Sequences added in Studio, after the tag's own.
    pub new_sequences: Vec<Sequence>,
    /// The tag's sequences as stored, so an edit to one (its label, range
    /// or sprites) is known by comparison.
    pub stored_sequences: Vec<Sequence>,
}

/// One sequence of a bitmap, as `bitmap.read` and the pickers show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceInfo {
    pub index: usize,
    pub label: String,
    pub first: i16,
    pub count: u16,
    pub sprites: usize,
    pub added: bool,
}

impl OpenBitmap {
    /// Images the tag has in the file (the rest are added).
    pub fn stored_images(&self) -> usize {
        self.images.iter().filter(|i| i.added.is_none()).count()
    }

    /// The bitmap has images or sequences the file does not: a save moves
    /// the tag.
    pub fn grown(&self) -> bool {
        self.images.iter().any(|i| i.added.is_some())
            || !self.new_sequences.is_empty()
            || self
                .parts
                .as_ref()
                .is_ok_and(|p| p.sequences != self.stored_sequences)
    }

    /// Sequence `index` (the tag's own, then the added ones).
    pub fn sequence(&self, index: usize) -> Option<&Sequence> {
        let stored = self.parts.as_ref().map_or(0, |p| p.sequences.len());
        match index.checked_sub(stored) {
            None => self.parts.as_ref().ok()?.sequences.get(index),
            Some(k) => self.new_sequences.get(k),
        }
    }

    fn sequence_mut(&mut self, index: usize) -> Option<&mut Sequence> {
        let stored = self.parts.as_ref().map_or(0, |p| p.sequences.len());
        match index.checked_sub(stored) {
            None => self.parts.as_mut().ok()?.sequences.get_mut(index),
            Some(k) => self.new_sequences.get_mut(k),
        }
    }

    /// Sprite `sprite` of sequence `index`, as the sprite editor shows it.
    pub fn sprite(&self, index: usize, sprite: usize) -> Option<SpriteView> {
        self.sequence(index)?.sprites.get(sprite).map(|r| SpriteView::read(r))
    }

    /// The tag's sequences, then the added ones.
    pub fn sequences(&self) -> Vec<SequenceInfo> {
        let stored = self.parts.as_ref().map_or(&[][..], |p| &p.sequences[..]);
        stored
            .iter()
            .map(|s| (s, false))
            .chain(self.new_sequences.iter().map(|s| (s, true)))
            .enumerate()
            .map(|(index, (s, added))| SequenceInfo {
                index,
                label: s.label(),
                first: s.first_image(),
                count: s.image_count(),
                sprites: s.sprites.len(),
                added,
            })
            .collect()
    }
}

/// One undo step of the Textures tool: the bitmap it belongs to, what it
/// changed, and its number among the steps of the bitmap's document
/// (`TagDoc::next_step`).
#[derive(Clone, Debug)]
struct Step {
    serial: u64,
    kind: EditKind,
}

#[derive(Clone, Debug)]
enum EditKind {
    /// A pixel edit, in the bitmap's own history.
    Pixels(TagId),
    /// A change to the bitmap's shape.
    Shape(TagId, Shape),
}

impl Step {
    fn tag(&self) -> TagId {
        match &self.kind {
            EditKind::Pixels(t) | EditKind::Shape(t, _) => *t,
        }
    }
}

#[derive(Clone, Debug)]
enum Shape {
    /// The last image was added. `kept` holds it while it is undone.
    AddImage {
        kept: Option<Box<(ImageInfo, Vec<bitmap_edit::Layer>)>>,
    },
    /// An image's size or format asked for (`None`: as stored).
    Reshape {
        image: usize,
        was: Option<Reshape>,
        now: Option<Reshape>,
    },
    /// The last sequence was added. `kept` holds it while it is undone.
    AddSequence { kept: Option<Sequence> },
    /// Sequence `index` (its label, range or sprites) changed from `was`
    /// to `now`.
    EditSequence {
        index: usize,
        was: Box<Sequence>,
        now: Box<Sequence>,
    },
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
    /// The undo steps, oldest first, each naming its bitmap, and the same
    /// for redo.
    history: Vec<Step>,
    redo: Vec<Step>,
    /// The last edit's outcome, for the API.
    pub last: Option<(TagId, Applied)>,
    /// Where the last export went.
    pub last_export: Option<PathBuf>,
    for_rev: Option<u64>,
    /// A new size or format asked for an image (`bitmap.reshape`,
    /// `bitmap.image.resize`), which the save writes whole. An undo step;
    /// [`BitmapAction::Revert`] drops it.
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
        o.bitmap.is_edited() || o.grown() || self.reshape.keys().any(|(t, _)| *t == o.tag)
    }

    /// An edit counter for a bitmap: its pixels' revision and the shape
    /// changes' (reshapes, images and sequences added).
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

    /// The bitmap the newest undo step belongs to, when that step changed
    /// pixels.
    pub fn last_step(&self) -> Option<TagId> {
        match &self.history.last()?.kind {
            EditKind::Pixels(t) => Some(*t),
            EditKind::Shape(..) => None,
        }
    }

    /// The number of bitmap `tag`'s newest step not undone. `None` while a
    /// stroke is being drawn: it is a step when it ends.
    pub fn step_serial(&self, tag: TagId) -> Option<u64> {
        if self.stroke.is_some() {
            return None;
        }
        let i = self.history.iter().rposition(|s| s.tag() == tag)?;
        Some(self.history[i].serial)
    }

    /// The number of bitmap `tag`'s oldest undone step, the next a redo of
    /// its tab takes.
    pub fn redo_serial(&self, tag: TagId) -> Option<u64> {
        if self.stroke.is_some() {
            return None;
        }
        let i = self.redo.iter().rposition(|s| s.tag() == tag)?;
        Some(self.redo[i].serial)
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
    /// whole. `None` keeps that one. One undo step.
    Reshape {
        tag: TagId,
        image: usize,
        width: Option<u32>,
        height: Option<u32>,
        format: Option<PixelFormat>,
    },
    /// Add an image after the bitmap's last, its record made from image
    /// `from` (default 0) at `width`, `height` and `format` (default
    /// `from`'s), its pixels transparent black. It is selected. One undo
    /// step; the save moves the tag.
    AddImage {
        tag: TagId,
        from: Option<usize>,
        width: Option<u32>,
        height: Option<u32>,
        format: Option<PixelFormat>,
    },
    /// Add a sequence of `count` images from `first`, labelled `label`,
    /// after the bitmap's last (shaped like its first sequence, with a
    /// sprite per image when that one has sprites). One undo step; the
    /// save moves the tag.
    AddSequence {
        tag: TagId,
        first: usize,
        count: u16,
        label: String,
    },
    /// Change a sequence's label, first image and/or image count (`None`
    /// keeps that one). One undo step; the save moves the tag.
    SetSequence {
        tag: TagId,
        sequence: usize,
        label: Option<String>,
        first: Option<usize>,
        count: Option<u16>,
    },
    /// Change sprite `sprite` of a sequence, or add one after its last
    /// (`sprite: None`; it starts as a copy of the last, or over all of
    /// its first image). `None` keeps that field. One undo step; the save
    /// moves the tag.
    SetSprite {
        tag: TagId,
        sequence: usize,
        sprite: Option<usize>,
        image: Option<usize>,
        /// Left, right, top, bottom.
        bounds: Option<[f32; 4]>,
        registration: Option<[f32; 2]>,
    },
    /// Remove a sprite from a sequence. One undo step; the save moves the
    /// tag.
    RemoveSprite {
        tag: TagId,
        sequence: usize,
        sprite: usize,
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
        let parts = (|| {
            let address = map
                .tag(tag)
                .ok()
                .and_then(|t| t.location)
                .map(|l| l.address)
                .ok_or_else(|| "it has no payload".to_owned())?;
            let bytes = map.read_tag_bytes(tag).map_err(|e| e.to_string())?;
            Parts::split(&bytes, address)
        })()
        .map_err(|why| format!("{name} cannot be laid out again: {why}"));
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
                added: None,
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
        let stored_sequences = parts.as_ref().map_or_else(|_| Vec::new(), |p| p.sequences.clone());
        Ok(OpenBitmap {
            tag,
            name,
            images,
            bitmap,
            images_at: b.header.images.address,
            parts,
            new_sequences: Vec::new(),
            stored_sequences,
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
        // Every save runs this first: the redrawn mouth curves ride with it.
        self.push_sound_changes();
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
            if open.grown() {
                match self.moved_bitmap(open, &mut files) {
                    Ok((moved, raw)) => {
                        c.raw_patches.extend(raw);
                        c.moved.push(moved);
                    }
                    Err(e) => {
                        c.refusal = Some(format!("{}: {e}", open.name));
                        return c;
                    }
                }
                continue;
            }
            for info in &open.images {
                let reshape = self.textures.reshape.get(&(tag, info.index)).copied();
                let image_at = open.images_at + IMAGE_LEN as u32 * info.index as u32;
                match plan_for(open, info, reshape, image_at, &mut files) {
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

    /// The write of a bitmap that gained images or sequences: the whole
    /// tag laid out again (`tags::bitmap_grow`) with every image's record
    /// writes made in the copy and its new pixel blocks pointed from it,
    /// and the in-place pixel patches of its stored images.
    fn moved_bitmap(
        &self,
        open: &OpenBitmap,
        files: &mut assets::ResourceFiles,
    ) -> Result<(TagMove, Vec<RawPatch>), String> {
        let mut parts = open.parts.clone()?;
        parts.images.extend(
            open.images
                .iter()
                .filter_map(|i| i.added.as_ref().map(|a| a.record.clone())),
        );
        parts.sequences.extend(open.new_sequences.iter().cloned());
        let layout = parts.layout();
        let mut bytes = layout.bytes;
        let mut raw = Vec::new();
        let mut patches = Vec::new();
        for info in &open.images {
            let reshape = self.textures.reshape.get(&(open.tag, info.index)).copied();
            let image_at = layout.images_at + IMAGE_LEN as u32 * info.index as u32;
            let Some(plan) = plan_for(open, info, reshape, image_at, files)? else {
                continue;
            };
            patches.extend(plan.raw.into_iter().map(|w| RawPatch {
                block: w.block,
                block_size: w.block_size,
                offset: w.offset,
                bytes: w.bytes,
            }));
            for (at, b) in plan.tag {
                let at = at as usize;
                bytes
                    .get_mut(at..at + b.len())
                    .ok_or_else(|| format!("image {}: a record write lies past the tag", info.index))?
                    .copy_from_slice(&b);
            }
            raw.extend(plan.appended.into_iter().map(|a| MovedRaw {
                bytes: a.bytes,
                align: a.align,
                pointer: a.pointer_address,
            }));
        }
        Ok((
            TagMove {
                tag: open.tag,
                bytes,
                rebase: layout.rebase,
                raw,
            },
            patches,
        ))
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
        if info.added.is_some() {
            return Err(format!(
                "image {image} is new: the save writes it whole, so it has no stored bytes to patch"
            ));
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
            self.textures.history.push(Step {
                serial: self.tags.next_step(),
                kind: EditKind::Pixels(tag),
            });
            self.textures.redo.clear();
        }
        self.textures.last = Some((tag, applied));
    }

    /// Records a change to a bitmap's shape as an undo step. The redo
    /// steps, the Textures tool's and the bitmap's own, are dropped.
    fn note_shape(&mut self, tag: TagId, shape: Shape) {
        self.textures.history.push(Step {
            serial: self.tags.next_step(),
            kind: EditKind::Shape(tag, shape),
        });
        self.textures.redo.clear();
        for o in self.textures.open.values_mut() {
            o.bitmap.clear_redo();
        }
        self.textures.reshape_rev += 1;
        self.textures.generation += 1;
    }

    /// Adds an image to bitmap `tag` (see [`BitmapAction::AddImage`]).
    /// Its index.
    fn add_image(
        &mut self,
        tag: TagId,
        from: Option<usize>,
        width: Option<u32>,
        height: Option<u32>,
        format: Option<PixelFormat>,
    ) -> Result<usize, String> {
        self.ensure_bitmap(tag)?;
        let open = self.open_mut(tag)?;
        let name = open.name.clone();
        if let Err(why) = &open.parts {
            return Err(format!("no image can be added: {why}"));
        }
        let from = from.unwrap_or(0);
        let src_info = open.images.get(from).ok_or_else(|| {
            format!(
                "image {from}: {name} has {} images to copy a new one from",
                open.images.len()
            )
        })?;
        let src = src_info
            .source
            .clone()
            .ok_or_else(|| format!("image {from} has no record"))?;
        if src.kind != tags::TextureKind::D2 {
            return Err(format!(
                "image {from} is a {:?} image: a new image copies a 2D one",
                src.kind
            ));
        }
        if src.lods[0].is_none() {
            return Err(format!(
                "image {from} stores only a lower mip in this map: copy another image"
            ));
        }
        let record = match &src_info.added {
            Some(a) => a.record.clone(),
            None => open
                .parts
                .as_ref()
                .ok()
                .and_then(|p| p.images.get(from))
                .cloned()
                .ok_or_else(|| format!("image {from} has no stored record"))?,
        };
        let want = Reshape {
            width: width.unwrap_or(u32::from(src.dimensions[0])),
            height: height.unwrap_or(u32::from(src.dimensions[1])),
            format: format.unwrap_or(src.format),
        };
        bitmap_edit::check_size(want.width, want.height).map_err(|e| e.to_string())?;
        let (mut record, img) = bitmap_grow::added_image(&record, &src)?;
        let (img, fields) = assets::write::reshaped(&img, want).map_err(|e| e.to_string())?;
        for (at, b) in fields {
            let at = at as usize;
            record[at..at + b.len()].copy_from_slice(&b);
        }
        let index = open.images.len();
        let canvas = bitmap_edit::Canvas::new(want.width, want.height, [0, 0, 0, 0])
            .map_err(|e| e.to_string())?;
        let id = LayerId::new(index as u32, 0, 0);
        open.bitmap.add_layer(id, canvas, None);
        open.images.push(ImageInfo {
            index,
            width: want.width,
            height: want.height,
            depth: 1,
            format: format!("{:?}", want.format),
            kind: "D2".to_owned(),
            mips: img.mip_count,
            pixels: "new".to_owned(),
            layers: vec![id],
            locked: None,
            error: None,
            source: Some(img),
            added: Some(Added { from, record }),
        });
        self.note_shape(tag, Shape::AddImage { kept: None });
        self.textures.selected = Some(tag);
        self.textures.image = index;
        self.textures.face = 0;
        self.textures.slice = 0;
        self.textures.selection = None;
        self.info(format!(
            "{name}: image {index} added, {}x{} {:?} from image {from}; the save moves the tag",
            want.width, want.height, want.format
        ));
        Ok(index)
    }

    /// Adds a sequence to bitmap `tag` (see [`BitmapAction::AddSequence`]).
    fn add_sequence(
        &mut self,
        tag: TagId,
        first: usize,
        count: u16,
        label: &str,
    ) -> Result<usize, String> {
        self.ensure_bitmap(tag)?;
        let open = self.open_mut(tag)?;
        let parts = open
            .parts
            .as_ref()
            .map_err(|why| format!("no sequence can be added: {why}"))?;
        let images = open.images.len();
        if count == 0 || first + usize::from(count) > images {
            return Err(format!(
                "images {first} to {}: the bitmap has {images}",
                (first + usize::from(count)).saturating_sub(1)
            ));
        }
        if label.len() > 31 {
            return Err(format!("label {label:?} is over 31 bytes"));
        }
        let first16 =
            i16::try_from(first).map_err(|_| format!("image {first} is past a sequence's reach"))?;
        let like = parts
            .sequences
            .first()
            .or(open.new_sequences.first())
            .cloned();
        let s = bitmap_grow::sequence(like.as_ref(), label, first16, count);
        let index = parts.sequences.len() + open.new_sequences.len();
        open.new_sequences.push(s);
        let name = open.name.clone();
        self.note_shape(tag, Shape::AddSequence { kept: None });
        self.info(format!(
            "{name}: sequence {index} {label:?} added, images {first} to {}",
            first + usize::from(count) - 1
        ));
        Ok(index)
    }

    /// Changes sequence `index` of bitmap `tag` with `edit`, as one undo
    /// step. An edit that changes nothing makes no step.
    fn edit_sequence(
        &mut self,
        tag: TagId,
        index: usize,
        edit: impl FnOnce(&mut Sequence, usize) -> Result<String, String>,
    ) -> Result<(), String> {
        self.ensure_bitmap(tag)?;
        let open = self.open_mut(tag)?;
        if let Err(why) = &open.parts {
            return Err(format!("no sequence can be edited: {why}"));
        }
        let images = open.images.len();
        let name = open.name.clone();
        let count = open.stored_sequences.len() + open.new_sequences.len();
        let seq = open
            .sequence_mut(index)
            .ok_or_else(|| format!("sequence {index}: the bitmap has {count}"))?;
        let was = seq.clone();
        let what = match edit(seq, images) {
            Ok(w) => w,
            Err(e) => {
                *seq = was;
                return Err(e);
            }
        };
        if *seq == was {
            return Ok(());
        }
        let now = seq.clone();
        self.note_shape(
            tag,
            Shape::EditSequence {
                index,
                was: Box::new(was),
                now: Box::new(now),
            },
        );
        self.info(format!("{name}: sequence {index} {what}"));
        Ok(())
    }

    fn set_sequence(
        &mut self,
        tag: TagId,
        index: usize,
        label: Option<String>,
        first: Option<usize>,
        count: Option<u16>,
    ) -> Result<(), String> {
        if label.as_ref().is_some_and(|l| l.len() > 31) {
            return Err("label is over 31 bytes".to_owned());
        }
        self.edit_sequence(tag, index, |s, images| {
            let f = first.map_or(Ok(i32::from(s.first_image())), |f| {
                i16::try_from(f)
                    .map(i32::from)
                    .map_err(|_| format!("image {f} is past a sequence's reach"))
            })?;
            let c = count.unwrap_or(s.image_count());
            if f < 0 || f as usize + usize::from(c) > images {
                return Err(format!("images {f} for {c}: the bitmap has {images}"));
            }
            let label = label.unwrap_or_else(|| s.label());
            let e = bitmap_grow::sequence(Some(s), &label, f as i16, c).entry;
            s.entry = e;
            Ok(format!("now {label:?}, images {f} for {c}"))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn set_sprite(
        &mut self,
        tag: TagId,
        index: usize,
        sprite: Option<usize>,
        image: Option<usize>,
        bounds: Option<[f32; 4]>,
        registration: Option<[f32; 2]>,
    ) -> Result<(), String> {
        self.edit_sequence(tag, index, |s, images| {
            let target = match sprite {
                Some(k) if k < s.sprites.len() => k,
                Some(k) => return Err(format!("sprite {k}: the sequence has {}", s.sprites.len())),
                None => {
                    let base = match s.sprites.last() {
                        Some(last) => last.clone(),
                        None => bitmap_grow::new_sprite(s.first_image().max(0)),
                    };
                    s.sprites.push(base);
                    s.sprites.len() - 1
                }
            };
            let mut v = SpriteView::read(&s.sprites[target]);
            if let Some(i) = image {
                if i >= images {
                    return Err(format!("image {i}: the bitmap has {images}"));
                }
                v.image = i16::try_from(i).map_err(|_| format!("image {i} is too far"))?;
            }
            if let Some([l, r, t, b]) = bounds {
                (v.left, v.right, v.top, v.bottom) = (l, r, t, b);
            }
            if let Some(g) = registration {
                v.registration = g;
            }
            v.check()?;
            v.write(&mut s.sprites[target]);
            Ok(format!("sprite {target}: image {}", v.image))
        })
    }

    fn remove_sprite(&mut self, tag: TagId, index: usize, sprite: usize) -> Result<(), String> {
        self.edit_sequence(tag, index, |s, _| {
            if sprite >= s.sprites.len() {
                return Err(format!("sprite {sprite}: the sequence has {}", s.sprites.len()));
            }
            s.sprites.remove(sprite);
            Ok(format!("sprite {sprite} removed"))
        })
    }

    /// Undoes (or redoes) a shape step of bitmap `tag`; the step to put
    /// on the other stack.
    fn swap_shape(&mut self, tag: TagId, shape: Shape, undo: bool) -> Option<Shape> {
        let open = self.textures.open.get_mut(&tag)?;
        let out = match shape {
            Shape::AddImage { kept } => {
                if undo {
                    let info = open.images.pop()?;
                    let layers = open.bitmap.remove_image(info.index as u32);
                    self.textures.reshape.remove(&(tag, info.index));
                    let (name, index) = (open.name.clone(), info.index);
                    self.ui_screens.show_image(&name, index, None);
                    Shape::AddImage {
                        kept: Some(Box::new((info, layers))),
                    }
                } else {
                    let (info, layers) = *kept?;
                    open.bitmap.restore_layers(layers);
                    open.images.push(info);
                    Shape::AddImage { kept: None }
                }
            }
            Shape::Reshape { image, was, now } => {
                let set = if undo { was } else { now };
                match set {
                    Some(r) => self.textures.reshape.insert((tag, image), r),
                    None => self.textures.reshape.remove(&(tag, image)),
                };
                Shape::Reshape { image, was, now }
            }
            Shape::AddSequence { kept } => {
                if undo {
                    Shape::AddSequence {
                        kept: Some(open.new_sequences.pop()?),
                    }
                } else {
                    open.new_sequences.push(kept?);
                    Shape::AddSequence { kept: None }
                }
            }
            Shape::EditSequence { index, was, now } => {
                let set = if undo { &was } else { &now };
                *open.sequence_mut(index)? = (**set).clone();
                Shape::EditSequence { index, was, now }
            }
        };
        if let Some(open) = self.textures.open.get(&tag)
            && self.textures.image >= open.images.len()
            && self.textures.selected == Some(tag)
        {
            self.textures.image = open.images.len().saturating_sub(1);
            self.textures.face = 0;
            self.textures.slice = 0;
        }
        self.textures.reshape_rev += 1;
        self.textures.generation += 1;
        Some(out)
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
                let was = self.textures.reshape.get(&(tag, image)).copied();
                let now = (!same).then_some(want);
                if was == now {
                    return Ok(());
                }
                match now {
                    Some(r) => self.textures.reshape.insert((tag, image), r),
                    None => self.textures.reshape.remove(&(tag, image)),
                };
                self.note_shape(tag, Shape::Reshape { image, was, now });
                self.info(format!(
                    "image {image}: {w}x{h} {:?} when saved",
                    want.format
                ));
            }
            BitmapAction::AddImage {
                tag,
                from,
                width,
                height,
                format,
            } => {
                self.add_image(tag, from, width, height, format)?;
            }
            BitmapAction::AddSequence {
                tag,
                first,
                count,
                label,
            } => {
                self.add_sequence(tag, first, count, &label)?;
            }
            BitmapAction::SetSequence {
                tag,
                sequence,
                label,
                first,
                count,
            } => {
                self.set_sequence(tag, sequence, label, first, count)?;
            }
            BitmapAction::SetSprite {
                tag,
                sequence,
                sprite,
                image,
                bounds,
                registration,
            } => {
                self.set_sprite(tag, sequence, sprite, image, bounds, registration)?;
            }
            BitmapAction::RemoveSprite {
                tag,
                sequence,
                sprite,
            } => {
                self.remove_sprite(tag, sequence, sprite)?;
            }
            BitmapAction::Revert(tag) => {
                if self.textures.stroke == Some(tag) {
                    self.textures.stroke = None;
                }
                self.textures.generation += 1;
                self.textures.open.remove(&tag);
                self.textures.reshape.retain(|(t, _), _| *t != tag);
                self.textures.pushed.remove(&tag);
                self.textures.history.retain(|s| s.tag() != tag);
                self.textures.redo.retain(|s| s.tag() != tag);
                self.textures.reshape_rev += 1;
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
        let step = if undo {
            self.textures.history.pop()
        } else {
            self.textures.redo.pop()
        };
        step.is_some_and(|step| self.textures_take(step, undo))
    }

    /// Undo (or redo) the last Textures edit of bitmap `tag`, a step of its
    /// tab's document (docs/editor/studio-flow.md, "Shared patterns"): the
    /// newest not undone, wherever it lies under other bitmaps' steps, or
    /// the oldest undone. Whether one moved.
    pub(super) fn textures_undo_in(&mut self, tag: TagId, undo: bool) -> bool {
        self.sync_textures();
        if self.textures.stroke.is_some() {
            return false;
        }
        let list = if undo {
            &mut self.textures.history
        } else {
            &mut self.textures.redo
        };
        let Some(i) = list.iter().rposition(|s| s.tag() == tag) else {
            return false;
        };
        self.textures_take(list.remove(i), undo)
    }

    /// Move `step` to the other stack, taking the bitmap with it: a pixel
    /// edit is undone (or redone) in the bitmap's own history, and a shape
    /// step is swapped for its opposite. A step whose bitmap has lost its
    /// side of it is dropped. Whether the step was taken.
    fn textures_take(&mut self, step: Step, undo: bool) -> bool {
        let tag = step.tag();
        let moved = match step.kind {
            EditKind::Shape(_, shape) => {
                let Some(other) = self.swap_shape(tag, shape, undo) else {
                    return false;
                };
                Step {
                    serial: step.serial,
                    kind: EditKind::Shape(tag, other),
                }
            }
            EditKind::Pixels(_) => {
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
                self.textures.last = Some((tag, applied));
                Step {
                    serial: step.serial,
                    kind: EditKind::Pixels(tag),
                }
            }
        };
        if undo {
            self.textures.redo.push(moved);
        } else {
            self.textures.history.push(moved);
        }
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

/// The write of one image of an open bitmap, its record at `image_at` (a
/// virtual address, or an offset into a moved tag's layout): `None` when
/// nothing about it changed. An added image is always written whole, at
/// the size and format asked.
fn plan_for(
    open: &OpenBitmap,
    info: &ImageInfo,
    reshape: Option<Reshape>,
    image_at: u32,
    files: &mut assets::ResourceFiles,
) -> Result<Option<assets::write::Plan>, String> {
    let layers: Vec<&bitmap_edit::Layer> = info
        .layers
        .iter()
        .filter_map(|id| open.bitmap.layer(*id))
        .collect();
    let src = info.source.as_ref().ok_or("the image has no record")?;
    let reshape = match (reshape, &info.added) {
        (None, Some(_)) => Some(Reshape {
            width: u32::from(src.dimensions[0]),
            height: u32::from(src.dimensions[1]),
            format: src.format,
        }),
        (r, _) => r,
    };
    if reshape.is_none() && !layers.iter().any(|l| l.is_edited()) {
        return Ok(None);
    }
    if let Some(why) = info.locked.as_ref().or(info.error.as_ref()) {
        return Err(format!("image {}: not written: {why}", info.index));
    }
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
        image_at,
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
