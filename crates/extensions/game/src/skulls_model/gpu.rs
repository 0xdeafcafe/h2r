//! The model picture on the GPU: the skull's render model read once from a
//! campaign map on the user's disc, in the background, and a pass that
//! draws it into the page's picture box in the game's own UI order
//! (`native_render::showcase_gpu`). docs/game/skulls.md, "The model
//! picture".

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use engine::memory::GuestMemory;
use native_render::gpu::Image;
use native_render::showcase;
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

/// The skull as read from the disc.
pub struct Skull {
    pub mesh: ShowcaseMesh,
    pub base: Option<Image>,
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
    ) -> Result<(crate::native_map::NativeMap, assets::ResourceFiles), native_render::load::Error>
    + 'a;

#[cfg(test)]
mod preview;

/// Reads the skull weapon's render model and its first base map from the
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
        let mesh = ShowcaseMesh::from_model(&loaded.model)
            .ok_or_else(|| format!("{name} {tag}: nothing opaque to show"))?;
        let base = loaded.model.materials().into_iter().find_map(|m| {
            let shader = *loaded.shaders.get(usize::from(m))?;
            let bitmap = native_render::load::base_map(&mut map, shader)?;
            native_render::load::decode_first(&mut map, &mut raw.0, bitmap)
        });
        return Ok(Skull {
            mesh,
            base,
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
    masks: HashMap<usize, Vec<u8>>,
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

/// The picture is drawn at this many texels for each of its box's, then
/// filtered down into the box.
const SUPERSAMPLE: f32 = 1.5;
const LARGEST: u32 = 1024;

impl d3d::ExternalPass for Pass {
    fn colour_only(&self) -> bool {
        true
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }

    fn encode(&mut self, t: d3d::ExternalTarget<'_>) {
        let mut r = self.renderer.lock().unwrap_or_else(|e| e.into_inner());
        let r = &mut *r;
        if r.showcase.as_ref().is_none_or(|(d, _)| d != t.device) {
            r.showcase = Some((t.device.clone(), Showcase::new(t.device, t.queue)));
            r.mesh = None;
            r.glyph = None;
        }
        let (_, showcase) = r.showcase.as_mut().expect("made above");
        let key = Arc::as_ptr(&self.skull) as usize;
        if r.mesh != Some(key) {
            showcase.set_mesh(
                t.device,
                t.queue,
                &self.skull.mesh,
                self.skull.base.as_ref(),
            );
            r.mesh = Some(key);
        }
        let glyph = self.look.glyph().map(|_| self.look.skull);
        if r.glyph != Some(glyph) {
            match glyph.and_then(|i| skull_glyphs::glyph(i).map(|g| (i, g))) {
                Some((i, strokes)) => {
                    let mask = r
                        .masks
                        .entry(i)
                        .or_insert_with(|| skull_glyphs::rasterise(strokes, skull_glyphs::SIZE));
                    showcase.set_glyph(t.device, t.queue, skull_glyphs::SIZE, mask);
                }
                None => showcase.set_glyph(t.device, t.queue, 0, &[]),
            }
            r.glyph = Some(glyph);
        }
        let [x, y, w, h] = super::guest_rect(crate::skulls_menu::PICTURE_BOX, t.guest_size);
        let sx = t.size.0 as f32 / t.guest_size.0.max(1) as f32;
        let sy = t.size.1 as f32 / t.guest_size.1.max(1) as f32;
        let rect = [x * sx, y * sy, w * sx, h * sy];
        let side = |n: f32| ((n * SUPERSAMPLE).round() as u32).clamp(16, LARGEST);
        let look = showcase::Look {
            yaw: showcase::sway(self.seconds),
            tint: self.look.tint(),
            silhouette: self.look.silhouette(),
            glyph: glyph.is_some(),
        };
        if showcase.draw(
            t.device,
            &mut *t.encoder,
            side(rect[2]),
            side(rect[3]),
            &look,
        ) {
            showcase.composite(
                t.device,
                &mut *t.encoder,
                t.color,
                t.color_format,
                rect,
                self.alpha,
            );
        }
    }
}

/// Whether the feature is on (read once).
fn enabled() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| super::on(&hooks::Switches::from_env().unwrap_or_default()));
    *ON
}

/// Said once: the pass could not be queued.
static REFUSED: AtomicBool = AtomicBool::new(false);

/// The native UI walk is about to draw widget `widget`: when it is the
/// page's noted picture and the skull has loaded, queue the model picture
/// in its place and answer true (the game's draw is skipped). Anything
/// else answers false and the game draws the widget.
pub fn replace_widget<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
    mem: &mut dyn GuestMemory,
    widget: u32,
) -> bool {
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
    let Ok(mut d) = device.lock() else {
        return false;
    };
    match d.external_pass_now(
        &mut crate::native_lights_glue::DeviceMemory(mem),
        Box::new(pass),
    ) {
        Ok(true) => true,
        Ok(false) => false,
        Err(e) => {
            if !REFUSED.swap(true, Ordering::Relaxed) {
                eprintln!("skulls model: the picture's pass was refused: {e:?}");
            }
            false
        }
    }
}
