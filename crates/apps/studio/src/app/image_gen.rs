//! Image generation on the app (docs/editor/editor-guide.md, "Generating
//! an image"): the slot and the references read from the open map, a job
//! started on [`crate::imagegen::ImageGen`], and the picture applied,
//! accepted or discarded through the Textures tool's own edits, so the
//! window, `image.*` and `bitmap.*` make the same undo steps and saves.
//!
//! - **Apply** writes the fitted picture into its slot as one
//!   `BitmapAction::Import` (one undo step), after keeping the slot's
//!   pixels as `original.png` in the job's directory. From there it shows
//!   where the slot is used: the Textures canvas, the UI Screens tool's
//!   picture and `ui.render` (through the disc's picture of the bitmap,
//!   `UiScreens::show_image`), and the paired game (the Textures tool's
//!   live sync, `play::raw_sync`).
//! - **Accept** keeps it: an applied picture stays as the bitmap's edit
//!   and saves with the map (`doc.save`). The provenance is written into
//!   the job's directory, logged, and with `save` copied beside the art.
//! - **Discard** takes it out: the import's undo step while it is still
//!   the newest step of that bitmap, else the original pixels imported
//!   back.

use std::path::{Path, PathBuf};

use bitmap_edit::LayerId;
use map::TagId;

use super::App;
use super::textures::BitmapAction;
use crate::imagegen::{Alpha, Applied, RefOrigin, Reference, Slot, Spec};

/// A reference as asked: a PNG file, or an image of a bitmap of the open
/// map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefInput {
    File(PathBuf),
    Map { bitmap: String, image: usize },
}

/// A generation as the window or `image.generate` asks for it.
#[derive(Clone, Debug, PartialEq)]
pub struct GenInput {
    pub prompt: String,
    pub references: Vec<RefInput>,
    /// The bitmap (named as `bitmap.*` names one) and image the picture
    /// is for.
    pub slot: Option<(String, usize)>,
    pub size: Option<(u32, u32)>,
    pub alpha: Alpha,
    pub out: Option<PathBuf>,
    pub from: Option<u64>,
}

/// One image generation command.
#[derive(Clone, Debug, PartialEq)]
pub enum ImageAction {
    /// Show the Generate window for an image of a bitmap.
    Open { bitmap: String, image: usize },
    /// Add an image to the bitmap, shaped like image `like`, and make it
    /// the window's slot.
    NewSlot { bitmap: String, like: usize },
    /// Start a generation.
    Generate(Box<GenInput>),
    /// Write a finished picture into its slot.
    Apply(u64),
    /// Keep it; `save` is a directory the art and its provenance are
    /// copied into.
    Accept { id: u64, save: Option<PathBuf> },
    /// Take it out of its slot.
    Discard(u64),
}

/// The Generate window's state.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    pub open: bool,
    /// The slot: the bitmap's path and the image.
    pub slot: Option<(String, usize)>,
    pub prompt: String,
    /// The images of the slot's bitmap picked as references.
    pub picked: Vec<usize>,
    /// PNG files added as references.
    pub files: Vec<PathBuf>,
    pub alpha: Alpha,
    /// The job shown.
    pub job: Option<u64>,
}

impl App {
    /// The image generation jobs; finished ones collected first.
    pub fn image_jobs(&mut self) -> &crate::imagegen::ImageGen {
        self.imagegen.poll();
        &self.imagegen
    }

    /// Image `image` of bitmap `bitmap` of the open map as a slot: its
    /// size, and the format a save writes it in (a reshape's when one is
    /// asked).
    pub fn image_slot(&mut self, bitmap: &str, image: usize) -> Result<Slot, String> {
        self.sync_textures();
        let tag = self.resolve_bitmap(bitmap)?;
        self.ensure_bitmap(tag)?;
        let open = &self.textures.open[&tag];
        let info = open.images.get(image).ok_or_else(|| {
            format!("image {image}: {} has {} images", open.name, open.images.len())
        })?;
        if let Some(why) = info.locked.as_ref().or(info.error.as_ref()) {
            return Err(format!("image {image} of {} is not written: {why}", open.name));
        }
        if info.depth > 1 || info.kind == "Cube" {
            return Err(format!(
                "image {image} of {} is a {} image: a generated picture goes into a 2D one",
                open.name, info.kind
            ));
        }
        let format = match self.textures.reshape.get(&(tag, image)) {
            Some(r) => r.format,
            None => info
                .source
                .as_ref()
                .map(|s| s.format)
                .ok_or_else(|| format!("image {image} of {} has no record", open.name))?,
        };
        if !crate::imagegen::fit::writable(format) {
            return Err(format!(
                "image {image} of {} is {format:?}, not a picture format Studio writes",
                open.name
            ));
        }
        Ok(Slot {
            tag,
            bitmap: open.name.clone(),
            image,
            width: info.width,
            height: info.height,
            format,
        })
    }

    /// A new slot for a picture: an image added to `bitmap` after its last
    /// (`BitmapAction::AddImage`, one Textures undo step), shaped like
    /// image `like` or at `size`. Its index; `image.generate` with
    /// `image: "new"` makes one, then generates into it.
    pub fn image_new_slot(
        &mut self,
        bitmap: &str,
        like: usize,
        size: Option<(u32, u32)>,
    ) -> Result<usize, String> {
        self.sync_textures();
        let tag = self.resolve_bitmap(bitmap)?;
        self.bitmap_action(BitmapAction::AddImage {
            tag,
            from: Some(like),
            width: size.map(|s| s.0),
            height: size.map(|s| s.1),
            format: None,
        })?;
        Ok(self.textures.open[&tag].images.len() - 1)
    }

    /// Starts a generation: the slot read, map references exported into
    /// the job's directory, the job started. Its id.
    pub fn image_generate(&mut self, input: GenInput) -> Result<u64, String> {
        self.imagegen.poll();
        if self.imagegen.busy() {
            return Err("an image is still being generated: wait for it (image.result)".into());
        }
        let slot = match &input.slot {
            Some((bitmap, image)) => Some(self.image_slot(bitmap, *image)?),
            None => None,
        };
        if let Some((w, h)) = input.size
            && (w == 0 || h == 0 || w > 4096 || h > 4096)
        {
            return Err("size is [w, h], each 1 to 4096".into());
        }
        let (_, dir) = self.imagegen.next_dir();
        let refs_dir = dir.join("references");
        let mut references = Vec::new();
        for (i, r) in input.references.iter().enumerate() {
            references.push(match r {
                RefInput::File(path) => {
                    if !path.is_file() {
                        return Err(format!("reference {} is not a file", path.display()));
                    }
                    Reference {
                        path: path.clone(),
                        origin: RefOrigin::File,
                    }
                }
                RefInput::Map { bitmap, image } => {
                    let tag = self.resolve_bitmap(bitmap)?;
                    self.ensure_bitmap(tag)?;
                    let name = self.textures.open[&tag].name.clone();
                    std::fs::create_dir_all(&refs_dir)
                        .map_err(|e| format!("{}: {e}", refs_dir.display()))?;
                    let path = refs_dir.join(format!("{i}-{}-{image}.png", short(&name)));
                    self.bitmap_action(BitmapAction::Export {
                        tag,
                        layer: LayerId::new(*image as u32, 0, 0),
                        mip: 0,
                        path: path.clone(),
                    })?;
                    Reference {
                        path,
                        origin: RefOrigin::Map {
                            bitmap: name,
                            image: *image,
                        },
                    }
                }
            });
        }
        let spec = Spec {
            prompt: input.prompt,
            references,
            slot,
            size: input.size,
            alpha: input.alpha,
            out: input.out,
            from: input.from,
        };
        let what = spec
            .slot
            .as_ref()
            .map(|s| format!(" for {} image {}", s.bitmap, s.image))
            .unwrap_or_default();
        let n = spec.references.len();
        let id = self.imagegen.start(spec)?;
        self.info(format!(
            "image {id}: generating{what} with {} and {n} reference{}",
            self.imagegen.source_name(),
            if n == 1 { "" } else { "s" }
        ));
        Ok(id)
    }

    /// A finished job's slot (none for a job made to a size), or why it
    /// has no picture.
    fn done(&mut self, id: u64) -> Result<Option<Slot>, String> {
        self.imagegen.poll();
        let job = self
            .imagegen
            .job(id)
            .ok_or_else(|| format!("no image {id} (image.result lists the last ones)"))?;
        if let Some(e) = job.error() {
            return Err(format!("image {id} failed: {e}"));
        }
        if job.outcome().is_none() {
            return Err(format!("image {id} is still being generated"));
        }
        Ok(job.spec.slot.clone())
    }

    /// A finished job's slot and fitted picture, or why there is none.
    fn finished(&mut self, id: u64) -> Result<(Slot, PathBuf), String> {
        let slot = self.done(id)?.ok_or_else(|| {
            format!("image {id} has no slot: generate it with a bitmap to write it into one")
        })?;
        let fitted = self
            .imagegen
            .job(id)
            .and_then(|j| j.outcome())
            .map(|o| o.fitted.clone())
            .ok_or_else(|| format!("image {id} has no picture"))?;
        Ok((slot, fitted))
    }

    /// Writes job `id`'s fitted picture into its slot as one undo step.
    pub fn image_apply(&mut self, id: u64) -> Result<String, String> {
        let (slot, fitted) = self.finished(id)?;
        let job = self.imagegen.job(id).expect("finished checked it");
        if job.applied.is_some() {
            return Ok(format!("image {id} is already in {} image {}", slot.bitmap, slot.image));
        }
        let original = job.dir.join("original.png");
        let tag = self.slot_tag(&slot)?;
        let layer = LayerId::new(slot.image as u32, 0, 0);
        self.bitmap_action(BitmapAction::Select {
            tag,
            image: slot.image,
        })?;
        self.bitmap_action(BitmapAction::Export {
            tag,
            layer,
            mip: 0,
            path: original.clone(),
        })?;
        self.bitmap_action(BitmapAction::Import {
            tag,
            layer,
            rect: None,
            path: fitted.clone(),
        })?;
        let rev = self.textures.open.get(&tag).map_or(0, |o| o.bitmap.rev());
        self.show_in_ui(&slot, Some(&fitted));
        if let Some(job) = self.imagegen.job_mut(id) {
            job.applied = Some(Applied {
                tag,
                image: slot.image,
                rev,
                original,
            });
            job.discarded = false;
        }
        let line = format!(
            "image {id}: in {} image {} ({}x{} {:?}); accept keeps it, discard takes it out",
            slot.bitmap, slot.image, slot.width, slot.height, slot.format
        );
        self.info(line.clone());
        Ok(line)
    }

    /// Keeps job `id`'s picture (applying it first when it has a slot and
    /// is not in it yet) and records its provenance; with `save`, copies
    /// the generator's picture, the fitted one and the provenance into
    /// that directory. A job made to a size has no slot: accepting it
    /// keeps the art and its provenance for a slot made later. The files
    /// written.
    pub fn image_accept(&mut self, id: u64, save: Option<&Path>) -> Result<Vec<PathBuf>, String> {
        let slot = self.done(id)?;
        if slot.is_some() && self.imagegen.job(id).is_some_and(|j| j.applied.is_none()) {
            self.image_apply(id)?;
        }
        let job = self.imagegen.job_mut(id).expect("finished checked it");
        job.accepted = Some(Vec::new());
        let mut written = vec![job.write_provenance()?];
        if let Some(dir) = save {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let o = job.outcome().expect("finished checked it").clone();
            let stem = match &slot {
                Some(s) => format!("{}-{}", short(&s.bitmap), s.image),
                None => format!("image-{id}"),
            };
            for (from, name) in [
                (&o.raw, format!("{stem}.png")),
                (&o.fitted, format!("{stem}.fitted.png")),
            ] {
                let to = dir.join(name);
                std::fs::copy(from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
                written.push(to);
            }
            let prov = dir.join(format!("{stem}.provenance.json"));
            let text = serde_json::to_string_pretty(&job.provenance()).map_err(|e| e.to_string())?;
            std::fs::write(&prov, text + "\n").map_err(|e| format!("{}: {e}", prov.display()))?;
            written.push(prov);
        }
        job.accepted = Some(written.clone());
        let _ = job.write_provenance();
        let refs = job.spec.references.len();
        let generator = job.outcome().map(|o| o.generator.clone()).unwrap_or_default();
        let prompt = job.spec.prompt.clone();
        let into = match &slot {
            Some(s) => format!("into {} image {} (saves with the map)", s.bitmap, s.image),
            None => "with no slot (the art and its provenance are kept)".to_owned(),
        };
        self.info(format!(
            "image {id}: accepted {into}; generator {generator}, {refs} reference{}, prompt {prompt:?}",
            if refs == 1 { "" } else { "s" },
        ));
        Ok(written)
    }

    /// Takes job `id`'s picture out of its slot.
    pub fn image_discard(&mut self, id: u64) -> Result<String, String> {
        self.imagegen.poll();
        let job = self
            .imagegen
            .job(id)
            .ok_or_else(|| format!("no image {id} (image.result lists the last ones)"))?;
        let Some(applied) = job.applied.clone() else {
            if let Some(job) = self.imagegen.job_mut(id) {
                job.discarded = true;
            }
            return Ok(format!("image {id} was not in a slot: nothing to take out"));
        };
        let slot = job.spec.slot.clone();
        self.sync_textures();
        let rev = self
            .textures
            .open
            .get(&applied.tag)
            .map(|o| o.bitmap.rev());
        let how = if rev == Some(applied.rev) && self.textures.last_step() == Some(applied.tag) {
            self.bitmap_action(BitmapAction::Undo)?;
            "undone"
        } else {
            self.bitmap_action(BitmapAction::Import {
                tag: applied.tag,
                layer: LayerId::new(applied.image as u32, 0, 0),
                rect: None,
                path: applied.original.clone(),
            })?;
            "its original pixels imported back (later edits came after it)"
        };
        if let Some(slot) = &slot {
            self.show_in_ui(slot, None);
        }
        if let Some(job) = self.imagegen.job_mut(id) {
            job.applied = None;
            job.accepted = None;
            job.discarded = true;
            let _ = job.write_provenance();
        }
        let line = format!("image {id}: taken out, {how}");
        self.info(line.clone());
        Ok(line)
    }

    /// The slot's tag in the open map now (the map may have been opened
    /// again since the job started).
    fn slot_tag(&mut self, slot: &Slot) -> Result<TagId, String> {
        let tag = self.resolve_bitmap(&slot.bitmap)?;
        self.ensure_bitmap(tag)?;
        Ok(tag)
    }

    /// Shows `png` for the slot in the UI Screens tool's pictures, or the
    /// disc's pixels again with `None`. A bitmap the disc's
    /// `mainmenu.map` does not have is left alone.
    fn show_in_ui(&mut self, slot: &Slot, png: Option<&Path>) {
        let rgba = png.and_then(|p| {
            let bytes = std::fs::read(p).ok()?;
            bitmap_edit::decode_png(&bytes).ok()
        });
        self.ui_screens.show_image(&slot.bitmap, slot.image, rgba);
    }

    /// Runs one [`ImageAction`], logging why it failed.
    pub(super) fn run_image(&mut self, action: ImageAction) {
        let r = match action {
            ImageAction::Open { bitmap, image } => {
                let slot = self.resolve_bitmap(&bitmap).and_then(|tag| {
                    self.ensure_bitmap(tag)?;
                    Ok(self.textures.open[&tag].name.clone())
                });
                match slot {
                    Ok(name) => {
                        let p = &mut self.image_panel;
                        if p.slot.as_ref() != Some(&(name.clone(), image)) {
                            p.picked.clear();
                            p.job = None;
                        }
                        p.slot = Some((name, image));
                        p.open = true;
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            }
            ImageAction::NewSlot { bitmap, like } => {
                self.image_new_slot(&bitmap, like, None).and_then(|image| {
                    let tag = self.resolve_bitmap(&bitmap)?;
                    let name = self.textures.open[&tag].name.clone();
                    let p = &mut self.image_panel;
                    p.picked.retain(|i| *i != image);
                    p.slot = Some((name, image));
                    p.job = None;
                    p.open = true;
                    Ok(())
                })
            }
            ImageAction::Generate(input) => self.image_generate(*input).map(|id| {
                self.image_panel.job = Some(id);
            }),
            ImageAction::Apply(id) => self.image_apply(id).map(drop),
            ImageAction::Accept { id, save } => self.image_accept(id, save.as_deref()).map(drop),
            ImageAction::Discard(id) => self.image_discard(id).map(drop),
        };
        if let Err(e) = r {
            self.error(format!("image: {e}"));
        }
    }
}

/// The last part of a tag path, for file names.
fn short(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}
