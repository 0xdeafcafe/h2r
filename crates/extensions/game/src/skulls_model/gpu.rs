//! The model picture on the GPU: the skull's render model read once from a
//! campaign map on the user's disc, in the background, and a pass that
//! draws it into the page's picture box in the game's own UI order
//! (`native_render::showcase_gpu`). docs/game/skulls.md, "The model
//! picture".

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use native_render::gpu::Image;
use native_render::showcase;
use native_render::showcase::{Axis, Orientation};
use native_render::showcase_gpu::{Showcase, ShowcaseMesh};

use super::Look;
use crate::skull_glyphs;

/// The campaign maps that place a skull, Cairo Station first: each holds
/// the skull weapon (`skulls_custom::SKULL_TAGS`; docs/game/skulls.md,
/// "Where each skull is"). The first that opens and has one is used.
pub const MAPS: [&str; 14] = [
    "01b_spacestation",
    "03a_oldmombasa",
    "03b_newmombasa",
    "04a_gasgiant",
    "04b_floodlab",
    "05a_deltaapproach",
    "05b_deltatowers",
    "06a_sentinelwalls",
    "06b_floodzone",
    "07a_highcharity",
    "07b_forerunnership",
    "08a_deltacliffs",
    "08b_deltacontrol",
    "01a_tutorial",
];

/// Which way the skull model faces in its own space: the picture turns
/// it so [`MODEL_FORWARD`] looks at the camera and [`MODEL_UP`] points
/// up, then by [`MODEL_ROLL_DEGREES`], [`MODEL_PITCH_DEGREES`] and
/// [`MODEL_YAW_DEGREES`] (`showcase::Orientation` has their signs). The
/// framing, the sway and the glyph's place all follow from the turned
/// model.
///
/// The model is authored well off every axis (it is a carried ball), so
/// the angles are large. They come from its geometry: its mirror plane
/// (the plane a reflection in which lands the surface back on itself)
/// gives the side-to-side axis, which is the picture's; the turn within
/// that plane is the one that puts the teeth (the second part) at the
/// bottom front and looks at the face from slightly above the level of
/// the teeth's row, so the forehead is in full view for the glyph. Turned
/// so, the eye sockets sit either side of the middle at one height, the
/// teeth under them and the dome over them. The preview checks the
/// mirror plane and the teeth on the disc's model and shows candidates
/// side by side (docs/game/skulls.md, "The model picture").
pub const MODEL_FORWARD: Axis = Axis::PosY;
pub const MODEL_UP: Axis = Axis::NegZ;
pub const MODEL_YAW_DEGREES: f32 = 37.0;
pub const MODEL_PITCH_DEGREES: f32 = 41.2;
pub const MODEL_ROLL_DEGREES: f32 = 8.6;

/// The model's axes and fine-tune together.
pub const MODEL_ORIENTATION: Orientation = Orientation {
    forward: MODEL_FORWARD,
    up: MODEL_UP,
    yaw: MODEL_YAW_DEGREES,
    pitch: MODEL_PITCH_DEGREES,
    roll: MODEL_ROLL_DEGREES,
};

/// The skull as read from the disc.
pub struct Skull {
    /// The model as authored (its own axes), for the preview's
    /// orientation sheet.
    pub rest: ShowcaseMesh,
    /// The model turned by [`MODEL_ORIENTATION`]: what the picture draws.
    pub mesh: ShowcaseMesh,
    /// Each opaque material's base map, by model-local material; a
    /// material missing here draws in the bone tint alone.
    pub bases: Vec<(u16, Image)>,
    /// Map and tag, for the log.
    pub source: String,
}

enum Asset {
    Idle,
    Loading,
    Ready(Arc<Skull>),
    Failed,
}

static ASSET: Mutex<Asset> = Mutex::new(Asset::Idle);

/// The skull, once loaded. The first ask starts the load on a background
/// thread and answers `None`, as every ask does until it has finished (the
/// game's picture draws meanwhile) or if it failed (said once, with why).
pub fn skull() -> Option<Arc<Skull>> {
    let mut asset = ASSET.lock().unwrap_or_else(|e| e.into_inner());
    match &*asset {
        Asset::Ready(skull) => return Some(Arc::clone(skull)),
        Asset::Loading | Asset::Failed => return None,
        Asset::Idle => {}
    }
    *asset = Asset::Loading;
    drop(asset);
    let spawned = background::spawn("skulls model load", || {
        let result = load();
        let next = match result {
            Ok(skull) => {
                eprintln!("skulls model: the picture's skull is {}", skull.source);
                Asset::Ready(Arc::new(skull))
            }
            Err(why) => {
                eprintln!("skulls model: no skull model, the page keeps its picture: {why}");
                Asset::Failed
            }
        };
        *ASSET.lock().unwrap_or_else(|e| e.into_inner()) = next;
    });
    if let Err(e) = spawned {
        eprintln!("skulls model: could not start the load: {e}");
        *ASSET.lock().unwrap_or_else(|e| e.into_inner()) = Asset::Failed;
    }
    None
}

/// Opens a map by name: the mounted game's (`native_map::open`) or, for the
/// preview, a disc's (`native_map::open_from_vfs`).
pub type Open<'a> = dyn Fn(
        &str,
    )
        -> Result<(crate::native_map::NativeMap, assets::ResourceFiles), native_render::load::Error>
    + 'a;

#[cfg(test)]
mod preview;

/// Reads the skull weapon's render model and its base maps from the
/// first of [`MAPS`] that has it, in the mounted game.
fn load() -> Result<Skull, String> {
    load_with(&crate::native_map::open)
}

/// [`load`] with the maps opened by `open`.
pub fn load_with(open: &Open<'_>) -> Result<Skull, String> {
    let mut tried = Vec::new();
    for name in MAPS {
        let (mut map, files) = match open(name) {
            Ok(opened) => opened,
            Err(e) => {
                tried.push(format!("{name}: {e}"));
                continue;
            }
        };
        let found = crate::skulls_custom::SKULL_TAGS.iter().find_map(|want| {
            map.tags()
                .iter()
                .find(|t| t.group == map::FourCC(*b"weap") && t.name.as_deref() == Some(*want))
                .map(|t| (t.id, *want))
        });
        let Some((id, tag)) = found else {
            tried.push(format!("{name}: no skull weapon"));
            continue;
        };
        let ctx = tags::object::Context::read(&mut map).map_err(|e| format!("{name}: {e}"))?;
        let mut raw = crate::native_world_glue::Raw(files);
        let loaded = native_render::model_load::load_object_model(&mut map, &ctx, &mut raw, id)
            .map_err(|e| format!("{name} {tag}: {e}"))?;
        let rest = ShowcaseMesh::from_model(&loaded.model)
            .ok_or_else(|| format!("{name} {tag}: nothing opaque to show"))?;
        let bases = loaded
            .model
            .materials()
            .into_iter()
            .filter_map(|m| {
                let shader = *loaded.shaders.get(usize::from(m))?;
                let bitmap = native_render::load::base_map(&mut map, shader)?;
                let image = native_render::load::decode_first(&mut map, &mut raw.0, bitmap)?;
                Some((m, image))
            })
            .collect();
        return Ok(Skull {
            mesh: rest.oriented(&MODEL_ORIENTATION),
            rest,
            bases,
            source: format!("{tag} in {name}"),
        });
    }
    Err(tried.join("; "))
}

/// The renderer, made on the first device that draws the picture.
#[derive(Default)]
struct Renderer {
    showcase: Option<(wgpu::Device, Showcase)>,
    /// The skull uploaded (by address), and the glyph set (by skull index,
    /// `None` for no glyph).
    mesh: Option<usize>,
    glyph: Option<Option<usize>>,
    /// Each glyph's distance field, made when first shown.
    fields: HashMap<usize, Vec<f32>>,
    /// The picture's size in the window's copy as last said in the log.
    said: Option<(u32, u32)>,
}

static RENDERER: LazyLock<Arc<Mutex<Renderer>>> = LazyLock::new(Arc::default);

/// The page's picture, drawn into the UI's colour target at its box.
struct Pass {
    skull: Arc<Skull>,
    look: Look,
    seconds: f64,
    alpha: f32,
    renderer: Arc<Mutex<Renderer>>,
}

impl native_render::Stage for Pass {
    fn name(&self) -> &'static str {
        "skulls model"
    }

    fn record(&mut self, t: &gfx::external::ExternalAttachments, view: &native_render::program::ViewInputs) -> native_render::StageWork {
        let renderer = Arc::clone(&self.renderer);
        let mut guard = renderer.lock().unwrap_or_else(|e| e.into_inner());
        let r = &mut *guard;
        if r.showcase.as_ref().is_none_or(|(d, _)| *d != t.device) {
            r.showcase = Some((t.device.clone(), Showcase::new(&t.device, &t.queue)));
            r.mesh = None;
            r.glyph = None;
        }
        let (_, showcase) = r.showcase.as_mut().expect("made above");
        let key = Arc::as_ptr(&self.skull) as usize;
        if r.mesh != Some(key) {
            showcase.set_mesh(&t.device, &t.queue, &self.skull.mesh, &self.skull.bases);
            r.mesh = Some(key);
        }
        let glyph = self.look.glyph().map(|_| self.look.skull);
        if r.glyph != Some(glyph) {
            match glyph.and_then(|i| skull_glyphs::glyph(i).map(|g| (i, g))) {
                Some((i, strokes)) => {
                    let field = r.fields.entry(i).or_insert_with(|| {
                        skull_glyphs::distance_field(strokes, skull_glyphs::FIELD)
                    });
                    showcase.set_glyph(&t.device, &t.queue, skull_glyphs::FIELD, field);
                }
                None => showcase.set_glyph(&t.device, &t.queue, 0, &[]),
            }
            r.glyph = Some(glyph);
        }
        let guest = view.guest_size;
        let [x, y, w, h] = super::guest_rect(crate::skulls_menu::PICTURE_BOX, guest);
        let sx = t.size.0 as f32 / guest.0.max(1) as f32;
        let sy = t.size.1 as f32 / guest.1.max(1) as f32;
        // The box in the target's own texels: the target is the window's
        // pixels, so the picture is drawn at the size it is seen.
        let Some(fit) = showcase::Fit::of([x * sx, y * sy, w * sx, h * sy]) else {
            unimplemented!("skulls model: an empty picture box");
        };
        if r.said != Some((fit.width, fit.height)) {
            r.said = Some((fit.width, fit.height));
            eprintln!(
                "skulls model: the picture is {} by {} texels, {} samples each, for a {} by {} \
                 box in the {} by {} window",
                fit.width,
                fit.height,
                native_render::showcase_gpu::SAMPLES,
                fit.rect[2],
                fit.rect[3],
                t.size.0,
                t.size.1,
            );
        }
        let look = showcase::Look {
            yaw: showcase::sway(self.seconds),
            tint: self.look.tint(),
            silhouette: self.look.silhouette(),
            glyph: glyph.is_some(),
        };
        let (device, color, color_format, alpha) = (t.device.clone(), t.color.clone(), t.color_format, self.alpha);
        drop(guard);
        crate::native_targets_glue::encoded(move |encoder| {
            let mut r = renderer.lock().unwrap_or_else(|e| e.into_inner());
            let Some((_, showcase)) = r.showcase.as_mut() else { return };
            if showcase.draw(&device, encoder, fit.width, fit.height, &look) {
                showcase.composite(&device, encoder, &color, color_format, fit.rect, alpha);
            }
        })
    }
}

/// Whether the feature is on (read once).
fn enabled() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| super::on(&hooks::Switches::from_env().unwrap_or_default()));
    *ON
}

/// The native UI walk is about to draw widget `widget`: when it is the
/// page's noted picture and the skull has loaded, queue the model picture
/// in its place and answer true (the game's draw is skipped). Anything
/// else answers false and the game draws the widget.
pub fn replace_widget(mem: &mut dyn GuestMemory, widget: u32) -> bool {
    if !enabled() {
        return false;
    }
    let Ok(block) = mem.u32(widget.wrapping_add(crate::list_class::WIDGET_BLOCK)) else {
        return false;
    };
    let Some((look, seconds)) = super::noted(widget, block) else {
        return false;
    };
    let Some(skull) = skull() else { return false };
    let Ok(word) = mem.u32(widget.wrapping_add(super::WIDGET_ALPHA)) else {
        return false;
    };
    let alpha = super::alpha(word);
    if alpha <= 0.0 {
        // Faded out: the game's draw would show nothing either.
        return true;
    }
    let pass = Pass {
        skull,
        look,
        seconds,
        alpha,
        renderer: Arc::clone(&RENDERER),
    };
    crate::native_targets_glue::push(&*mem, frame_types::Slot::Overlay, pass);
    true
}
