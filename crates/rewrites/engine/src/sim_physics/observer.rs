//! Camera port descriptor and physical alias checks for the camera frame.
use super::*;
use crate::sim_seats::camera;

pub(super) fn camera(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    camera::run(c, g, pc)
}

// The camera path keeps its private frame in Rust until the final stores.
// With aliased inputs or an unwriteable frame, run the original in order.
pub(crate) fn disjoint(c: &CpuAbi, g: &Guest) -> bool {
    use runtime::guest::Access;
    let Some(sp) = c.esp.checked_sub(84) else { return false };
    let Some(end) = c.esp.checked_add(4) else { return false };
    let Some(view_end) = c.esi.checked_add(84) else { return false };
    let Ok(axis) = g.u32(camera::ROTATION_AXIS) else { return false };
    let Some(axis_end) = axis.checked_add(12) else { return false };
    let (Some(stack), Some(view)) =
        (physical_span(g,sp,end-sp), physical_span(g,c.esi,view_end-c.esi))
    else {return false};
    if overlap(&stack,&view) {return false;}
    for input in [axis..axis_end, camera::HALF..camera::ONE+4, camera::ATAN_DENOMINATOR..camera::ATAN_DENOMINATOR+8,
                  camera::DOT_FLOOR..camera::DOT_FLOOR+4, camera::VIEW_SCALE..camera::VIEW_SCALE+4, camera::ROTATION_AXIS..camera::ROTATION_AXIS+4] {
        let Some(span) = physical_span(g,input.start,input.end-input.start) else {return false};
        if overlap(&span,&view) || overlap(&span,&stack)
            || g.check(input.start,input.len(),Access::Read,1).is_err() { return false; }
    }
    g.check(sp,84,Access::Write,1).is_ok() && g.check(c.esp,4,Access::Read,1).is_ok()
        && g.check(c.esi,84,Access::Read,1).is_ok() && g.check(c.esi+32,52,Access::Write,1).is_ok()
}
