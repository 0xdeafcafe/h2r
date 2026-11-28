//! The game's UI callbacks' host (`render_bridge::native_hud`). The Rust
//! HUD draws its quads, widgets and strings from their shaders' Rust
//! materials (`crate::hud_native`), and the menus' runs, model views,
//! skull widgets and the modal blur are stages placed on the overlay. A
//! draw the guest itself would issue (a captured fan, bar or outline) has
//! no device behind it and panics by name.

use std::sync::Arc;

use guest_layout::memory::GuestMemory;
use render_bridge::native_hud::{Host, HudTagQuad, HudVertices, UiScope};

struct Glue;

/// A guest UI draw with no typed stage.
fn guest(what: &str) -> ! {
    unimplemented!("hud: the guest's {what} from the device")
}

impl Host for Glue {
    fn draw_tag_quad(&self, _: &mut dyn GuestMemory, at: &HudTagQuad, _: &HudVertices) -> bool {
        guest(&format!("tag quad of shader {:#x}", at.shader))
    }

    fn rust_hud_quad(&self, mem: &mut dyn GuestMemory, at: &HudTagQuad, vertices: &HudVertices) -> bool {
        crate::hud_native::draw(mem, at, vertices)
    }

    fn rust_hud_widget(&self, mem: &mut dyn GuestMemory, widget: &render_bridge::native_hud::WalkWidget) -> bool {
        crate::hud_native::draw_widget(mem, widget)
    }

    fn rust_hud_text(&self, mem: &mut dyn GuestMemory, text: &render_bridge::native_hud::WalkText) -> bool {
        crate::hud_native::draw_text(mem, text)
    }

    fn rust_string(&self, mem: &mut dyn GuestMemory, run: &render_bridge::native_hud::strings::StringRun) -> bool {
        crate::hud_native::draw_string(mem, run)
    }

    fn draw_quad(&self, _: &mut dyn GuestMemory, _: &HudVertices) -> bool {
        guest("screen quad")
    }

    fn fade_fan(&self, _: &mut dyn GuestMemory, _: &[[(u8, [f32; 4]); 1]; 4]) -> bool {
        guest("fade fan")
    }

    fn loading_bitmap_fan(&self, _: &mut dyn GuestMemory, _: &[[(u8, [f32; 4]); 2]; 4]) -> bool {
        guest("loading bitmap fan")
    }

    fn loading_spinner(&self, _: &mut dyn GuestMemory, _: &HudVertices) -> bool {
        guest("loading spinner")
    }

    fn backdrop_wash(&self, _: &mut dyn GuestMemory, _: &HudVertices) -> bool {
        guest("backdrop wash")
    }

    fn backdrop_composite(&self, _: &mut dyn GuestMemory, _: &engine::ui_backdrop::Fan<2>) -> bool {
        guest("backdrop composite")
    }

    fn sensor_blip(&self, _: &mut dyn GuestMemory, _: &HudVertices) -> bool {
        guest("motion sensor blip")
    }

    fn inline_ui_quad(&self, _: &mut dyn GuestMemory, primitive: u32, _: Vec<u8>) -> bool {
        guest(&format!("inline bar (primitive {primitive})"))
    }

    fn replace_widget(&self, mem: &mut dyn GuestMemory, widget: u32) -> bool {
        crate::skulls_model::gpu::replace_widget(mem, widget)
    }

    fn model_view(&self, mem: &mut dyn GuestMemory, bounds: [u16; 4], view: &frame_scene::OverlayModelView) {
        crate::monitor_model::gpu::appearance_view(mem, bounds, view);
    }

    fn ui_overlay(&self, _: &mut dyn GuestMemory) -> Option<bool> {
        Some(true)
    }

    fn ui_text(&self, _: &dyn GuestMemory) -> Option<Box<dyn render_bridge::native_hud::ui_overlay::UiText>> {
        Some(Box::new(crate::hud_overlay::menu_text::MenuText(crate::hud_overlay::fonts()?)))
    }

    fn ui_custom_widget(&self, mem: &dyn GuestMemory, widget: u32) -> bool {
        widget
            .checked_add(crate::list_class::WIDGET_BLOCK)
            .and_then(|addr| mem.u32(addr).ok())
            .is_some_and(|block| crate::skulls_model::noted(widget, block).is_some())
    }

    fn ui_list_arrows(&self, vtable: u32, height: i32) -> Option<[[i16; 2]; 2]> {
        crate::list_class::arrows_at(vtable, height).map(|a| [a.up, a.down])
    }

    fn ui_pass(&self, mem: &mut dyn GuestMemory, part: &Arc<frame_scene::Overlay>) -> bool {
        crate::hud_overlay::place_menu(mem, part)
    }

    fn modal_blur(&self, mem: &mut dyn GuestMemory, call: &render_bridge::native_hud::modal_blur::ModalBlur) -> bool {
        crate::modal_blur_glue::arm(mem, call)
    }

    fn finish_modal_blur(&self, mem: &mut dyn GuestMemory, esp: u32) -> bool {
        crate::modal_blur_glue::finish(mem, esp)
    }

    fn enter_ui(&self, _: &mut dyn GuestMemory, _: UiScope, _: u32, _: bool) {}

    fn leave_ui(&self, _: &mut dyn GuestMemory, _: UiScope, _: u32) {}
}

pub fn install() {
    render_bridge::native_hud::set_host(Arc::new(Glue));
}
