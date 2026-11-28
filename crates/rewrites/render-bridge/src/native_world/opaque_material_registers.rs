//! Live shader input and constant banks used by the opaque material binder.

use guest_layout::entry::{Cx, EntryError};
use engine::light_constants::Error;
use guest_layout::memory::GuestMemory;
use engine::render_loop::world_registers as registers;
use native_render::camera::GuestRead;

struct Read<'a>(&'a dyn GuestMemory);
impl GuestRead for Read<'_> {
    fn read_u32(&self, addr:u32) -> Option<u32> { self.0.u32(addr).ok() }
}

fn fault(e:Error) -> EntryError {
    match e {
        Error::Memory(f) => EntryError::Fault(f),
        _ => EntryError::IsolatedCall(format!("opaque shader register evaluation: {e:?}")),
    }
}

pub(super) async fn inputs(cx:&Cx, cache:u32, fcw:u16, mxcsr:u32) -> Result<(),EntryError> {
    cx.write(|m| registers::inputs(m,cache,fcw,mxcsr,|m,h| {
        native_render::object_lighting::root_word(&Read(m),h)
    })).map_err(fault)
}

pub(super) fn write_colour(cx:&Cx,cache:u32,target:u16,mask_index:u8,packed:u32) -> Result<(),EntryError> {
    Ok(cx.write(|m| registers::write_colour(m,cache,target,mask_index,packed))?)
}

pub(super) async fn instance_colours(cx:&Cx,cache:u32,range:u32) -> Result<(),EntryError> {
    Ok(cx.write(|m| registers::instance_colours(m,cache,range))?)
}

pub(super) async fn vertex(cx:&Cx,cache:u32,range:u32,mxcsr:u32) -> Result<(),EntryError> {
    cx.write(|m| registers::vertex(m,cache,range,mxcsr)).map_err(fault)
}

pub(super) async fn pixel(cx:&Cx,cache:u32,range:u32,fcw:u16,mxcsr:u32) -> Result<(),EntryError> {
    cx.write(|m| registers::pixel(m,cache,range,fcw,mxcsr)).map_err(fault)
}
