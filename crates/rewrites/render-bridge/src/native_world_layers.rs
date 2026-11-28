//! Layer context the native renderer pairs with the guest's opaque layer draws.

use std::cell::Cell;

use guest_layout::points;
use native_render::camera::GuestRead;
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, STENCIL_OFF_FLAGS};
use native_render::world_layers::{Site, Texaccum, layer_site, opaque_layer};

use crate::native_world::Mem;
use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-world-layers";

// The scene view index set by render_game_view (render-loop.md).
const SCENE_VIEW: u32 = 0x004b_a050;

pub fn on(_: &hooks::Switches) -> bool {
    true
}

#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub number: u32,
    pub view: u32,
    pub caller_return: u32,
    pub site: Site,
    pub list: u32,
    pub stencil_off: bool,
    pub texaccum: Option<Texaccum>,
}

thread_local! {
    static LAYER: Cell<Option<Layer>> = const { Cell::new(None) };
}

pub fn current() -> Option<Layer> {
    LAYER.get()
}

pub struct NativeWorldLayers;

impl Feature for NativeWorldLayers {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "World and object opaque layers").doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::LAYER_DRAW, "native_world_layers_begin", |o| {
                use crate::native_map_work::Stage;
                let lights = !Stage::Lights.standing_down();
                let mut guest = o.guest();
                let mem = Mem(&mut *guest);
                let layer = mem.read_u32(CURRENT_LAYER).filter(|&n| opaque_layer(n) || (lights && matches!(n, 12 | 13)));
                LAYER.set(layer.map(|number| {
                    let caller_return = if matches!(number, 12 | 13) {
                        engine::render::light_gather::passes::caller(&*mem.0, o.cpu.esp)
                    } else {
                        mem.read_u32(o.cpu.esp)
                    }.unwrap_or(0);
                    Layer {
                        number,
                        view: mem.read_u32(SCENE_VIEW).unwrap_or(u32::MAX),
                        caller_return,
                        site: layer_site(number, caller_return),
                        list: mem.read_u32(CURRENT_LIST).unwrap_or(0),
                        stencil_off: mem
                            .read_u32(STENCIL_OFF_FLAGS)
                            .is_some_and(|flags| flags & 0x00ff_ff00 != 0),
                        texaccum: Texaccum::read(&mem),
                    }
                }));
            })
            .context_only();
        b.raw()
            .observe_before(&points::LAYER_END, "native_world_layers_end", |_| {
                LAYER.set(None)
            })
            .context_only();
        // The sky's cut (docs/graphics/retire-d3d-r5.md, "The sky's cut"):
        // once its arm is ready the guest's `0x0003ede0` is passed over at
        // every pass, the native pass drawing the sky or its refusal
        // counted `scene_dropped_sky_<why>`.
        b.raw()
            .before(&points::RENDER_SKY, "native_sky_cut", |c| {
                let rt = std::sync::Arc::clone(c.ctx.runtime());
                let cut = {
                    let mut guest = rt.memory();
                    crate::native_sky::before_sky(&mut *guest, crate::native_map_work::bsp_generation())
                        .passes_over()
                };
                if cut { c.ret(0).map(hooks::Flow::Return) } else { Ok(hooks::Flow::Continue) }
            })
            .context_only()
            .optional();
        b.raw()
            .observe_after(&points::RENDER_SKY, "native_sky_cut_end", |_| crate::native_sky::after_sky())
            .context_only()
            .optional();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}
