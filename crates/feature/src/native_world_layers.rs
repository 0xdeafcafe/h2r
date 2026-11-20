//! Layer context the native renderer pairs with the guest's opaque layer draws.

use std::cell::Cell;

use engine::points;
use native_render::camera::GuestRead;
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, STENCIL_OFF_FLAGS};
use native_render::world_layers::{Site, Texaccum, layer_site, opaque_layer};

use crate::native_world::Mem;
use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-world-layers";

// The scene view index set by render_game_view (render-loop.md).
const SCENE_VIEW: u32 = 0x004b_a050;

pub fn on(switches: &crate::Switches) -> bool {
    crate::switches::native_render_on(switches)
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

/// The pipelined executor stopped inside this layer. Its remaining
/// handlers will run on the thread that takes the suspended render stack.
pub(crate) fn replace(layer: Option<Layer>) -> Option<Layer> {
    LAYER.replace(layer)
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
                    let caller_return = mem.read_u32(o.cpu.esp).unwrap_or(0);
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
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
