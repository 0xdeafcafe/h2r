//! Native submission of the UI's packed outline after line state and Begin.
use std::sync::Arc;

use engine::{memory::GuestMemory, points::ui_render::*};
use runtime::abi::TransferAbi;
use timeline::native::{Metric, Stage, Ui};

use crate::{Builder, Call, Flow};

use super::{host, ui};

const RETURN_FROM_FRAME: u32 = 0x10;
const POINTS_FROM_FRAME: u32 = 0x14;
const PRIMITIVE_TAG: u32 = 0x004b_6298;
const LINE_STRIP_TAG: u32 = 2;
const POINTS: usize = 5;
pub const PUSH_BYTES: u32 = ((POINTS + 2) * 8) as u32;

pub type Vertices = [[(u8, [f32; 4]); 2]; POINTS];

fn vertices(mem: &dyn GuestMemory, points: u32, argb: u32) -> Option<Vertices> {
    if points == 0 {
        return None;
    }
    let [b, g, r, a] = argb.to_le_bytes();
    let color = (9, [r, g, b, a].map(|v| f32::from(v) / 255.0));
    let mut vertices = [[color; 2]; POINTS];
    for (i, vertex) in vertices.iter_mut().enumerate() {
        let at = points.checked_add(i as u32 * 4)?;
        let x = mem.u16(at).ok()? as i16;
        let y = mem.u16(at.checked_add(2)?).ok()? as i16;
        vertex[1] = (0, [f32::from(x), f32::from(y), 0.0, 1.0]);
    }
    Some(vertices)
}

fn draw(c: &Call<'_>) -> Option<TransferAbi> {
    let rt = Arc::clone(c.ctx.runtime());
    let mut mem = rt.memory();
    let esp = c.cpu.esp;
    if mem.u32(esp.checked_add(RETURN_FROM_FRAME)?).ok()? != OUTLINE_SUBMIT_RETURN {
        return None;
    }
    Stage::Ui.count(Metric::WalkExamined, 1);
    if !rt.read_dispatch(|d| d.has_generated(OUTLINE_VERTICES_TAIL)) {
        Ui::OutlineEntry.count();
        return None;
    }
    let Some(points) = esp.checked_add(POINTS_FROM_FRAME).and_then(|at| mem.u32(at).ok()) else {
        return refuse_inputs();
    };
    if usize::from(c.cpu.ebp as u16) != POINTS || mem.u32(PRIMITIVE_TAG).ok() != Some(LINE_STRIP_TAG) {
        return refuse_inputs();
    }
    let Some(vertices) = vertices(&*mem, points, c.cpu.ebx) else {
        return refuse_inputs();
    };
    let drawn = host().is_some_and(|h| h.outline(&mut *mem, &vertices));
    ui::record(drawn);
    if drawn {
        Ui::OutlineNative.count();
        Some(TransferAbi::jump(OUTLINE_VERTICES_TAIL))
    } else {
        Ui::OutlineReservation.count();
        None
    }
}

fn refuse_inputs() -> Option<TransferAbi> {
    Ui::OutlineInputs.count();
    Stage::Ui.count(Metric::WalkRefusedInputs, 1);
    None
}

pub(super) fn place(b: &mut Builder<'_>) {
    b.raw()
        .before(&OUTLINE_VERTICES, "native_ui_outline_vertices", |c| {
            Ok(draw(c).map_or(Flow::Continue, Flow::Return))
        })
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::testing::FakeGuest;

    #[test]
    fn outline_keeps_signed_positions_and_register_color() {
        let mut mem = FakeGuest::new();
        let points = 0x1000;
        for i in 0..POINTS as u32 {
            mem.put_u16(points + 4 * i, (i as i16 - 2) as u16)
                .put_u16(points + 4 * i + 2, 10 + i as u16);
        }
        let geometry = vertices(&mem, points, 0xff_80_40_20).unwrap();
        assert_eq!(geometry[0][0], (9, [128., 64., 32., 255.].map(|v| v / 255.0)));
        assert_eq!(geometry[0][1], (0, [-2.0, 10.0, 0.0, 1.0]));
        assert_eq!(geometry[4][1], (0, [2.0, 14.0, 0.0, 1.0]));
        assert!(vertices(&mem, 0x2000, 0).is_none());
    }
}
