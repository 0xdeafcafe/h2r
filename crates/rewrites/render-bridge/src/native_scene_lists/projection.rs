//! The view projection builder `0x0002fb70` in Rust
//! (docs/graphics/native-visibility.md, "The view records"): the port is
//! `engine::view_projection`, held to the lifted original by
//! `engine`'s `view_projection_differential`. A call the port declines
//! (another float state, a used x87 stack, a NaN, an aliased buffer) runs
//! the guest's body. The caller's own code then reaches
//! `VIEW_PROJECTION_TERMS` (`0x0002fda3`), so the display fit applies to
//! the Rust terms as to the guest's.
use super::*;
use engine::view_projection::{self as projection, Call};

impl From<projection::Refusal> for Refusal {
    fn from(r: projection::Refusal) -> Self {
        match r {
            projection::Refusal::Memory => Self::Memory,
            projection::Refusal::Layout => Self::Layout,
            projection::Refusal::Unsupported => Self::Unsupported,
        }
    }
}

pub(super) fn build(m: &dyn GuestMemory, cpu: &CpuAbi) -> (SceneList, Result<Output>) {
    (SceneList::ViewProjection, output(m, cpu))
}

fn output(m: &dyn GuestMemory, cpu: &CpuAbi) -> Result<Output> {
    // `fld` and `fptan` push two values: the port assumes an empty stack.
    if cpu.x87.tag != u16::MAX {
        return Err(Refusal::Unsupported);
    }
    let call = Call {
        camera: cpu.ecx,
        extents: cpu.edx,
        output: cpu.eax,
    };
    let built = projection::build(m, call, cpu.mxcsr, cpu.x87.control)?;
    let mut out = Output::default();
    let bytes: Vec<u8> = built.words.iter().flat_map(|w| w.to_le_bytes()).collect();
    out.array(call.output, bytes);
    out.ecx = Some(built.ecx);
    out.edx = Some(built.edx);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;
    use engine::view_projection::guest as g;

    const CAMERA: u32 = 0x0010_0000;
    const OUT: u32 = 0x0010_2000;

    fn image() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_f32(g::ONE, 1.0);
        m.put_f32(g::EXTENT_MIN, -1.0);
        m.put_f32(g::HALF, 0.5);
        m.put_f32(g::OFFSET_SCALE, 0.5);
        m.put_f32(g::ANAMORPHIC, 4.0 / 3.0);
        m.put_f32(g::ANAMORPHIC_TAN, 0.75);
        m.put_f32(g::DISPLAY_EXTENT, 1.0 / 480.0);
        m.put_u32(g::CAMERA_OVERRIDE, 0);
        m.put_u8(g::WIDESCREEN, 0);
        m.put_u32(g::DISPLAY, 0);
        m.put_f32(CAMERA + g::CAMERA_FOV, 1.2);
        for (k, v) in [0u16, 0, 480, 640].into_iter().enumerate() {
            m.put_u16(CAMERA + g::CAMERA_VIEWPORT + 2 * k as u32, v);
        }
        m
    }

    fn cpu() -> CpuAbi {
        CpuAbi {
            eax: OUT,
            ecx: CAMERA,
            edx: 0,
            ..CpuAbi::xbox_thread()
        }
    }

    #[test]
    fn a_call_writes_the_whole_buffer_and_the_two_registers() {
        let m = image();
        let out = output(&m, &cpu()).unwrap();
        assert_eq!(out.writes.len(), 1);
        assert_eq!(out.writes[0].addr, OUT);
        assert_eq!(out.writes[0].bytes.slice().len(), 0x40);
        let t = projection::tangent(1.2, 0.5).unwrap();
        assert_eq!(out.ecx, Some(t.to_bits()));
        assert_eq!(out.edx, Some(0));
        assert_eq!(out.eax, None, "EAX keeps the buffer");
        assert_eq!(out.pop, 0);
    }

    #[test]
    fn a_used_x87_stack_or_another_rounding_runs_the_guest() {
        let m = image();
        let mut c = cpu();
        c.x87.tag = 0x3fff;
        assert!(matches!(output(&m, &c), Err(Refusal::Unsupported)));
        let mut c = cpu();
        c.mxcsr = 0x1f80 | 0x2000;
        assert!(matches!(output(&m, &c), Err(Refusal::Unsupported)));
    }
}
