//! One D3D-only light supplies a reference even when no complete native
//! plan is drawable in the view. Compare at the original layer end,
//! while the per-light globals still belong to this call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use engine::light_constants::global as globals;
use engine::light_passes::{self, global::PASS_FLAGS};
use engine::memory::GuestMemory;
use native_render::lights::Light;
use native_render::skip;

static ENABLED: AtomicBool = AtomicBool::new(false);
static SAMPLE: Mutex<Sample> = Mutex::new(Sample { pending: None, reported: false });

struct Sample {
    pending: Option<Light>,
    reported: bool,
}

pub(super) fn enable(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub(super) fn next_light() {
    if ENABLED.load(Ordering::Relaxed) {
        SAMPLE.lock().unwrap_or_else(|e| e.into_inner()).pending = None;
    }
}

pub(super) fn refused(light: Light) {
    if ENABLED.load(Ordering::Relaxed) && light.constants.is_some() {
        let mut sample = SAMPLE.lock().unwrap_or_else(|e| e.into_inner());
        if !sample.reported {
            sample.pending = Some(light);
        }
    }
}

pub(super) fn layer_end(mem: &dyn GuestMemory, ret: u32) {
    // The tail zeroes the flags at 0x00031511. Object shadows and
    // first-person lighting also use this list, from different callers.
    if !(light_passes::RENDER_LIGHT_PASSES..0x0003_1511).contains(&ret)
        || mem.u32(skip::CURRENT_LIST).ok() != Some(super::LIGHT_LIST)
    {
        return;
    }
    let mut sample = SAMPLE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(light) = sample.pending.take() else { return };
    let Some(c) = light.constants else { return };
    let differences = differences(mem, c.flags, &c.frame);
    sample.reported = true;
    eprintln!(
        "native-lights compare: D3D fallback light {:#010x}, guest pass flags and frame for c[31..39], {} differing words",
        light.handle,
        differences.len()
    );
    for difference in differences {
        eprintln!("  {difference}");
    }
}

fn differences(mem: &dyn GuestMemory, flags: u32, frame: &[u32; 36]) -> Vec<String> {
    let mut differences = Vec::new();
    let mut field = |address, expected, label: String| {
        match mem.u32(address) {
            Ok(actual) if expected == actual => {}
            Ok(actual) => differences.push(format!("{label} D3D {actual:#010x} native {expected:#010x}")),
            Err(_) => differences.push(format!("{label} unreadable on D3D, native {expected:#010x}")),
        }
    };
    field(PASS_FLAGS, flags, "pass flags".into());
    for (i, &expected) in frame.iter().enumerate() {
        let label = format!("c[{}].{}", 31 + i / 4, ["x", "y", "z", "w"][i % 4]);
        field(globals::FRAME + i as u32 * 4, expected, label);
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::FlatMemory;

    #[test]
    fn reference_identifies_the_exact_register_lane_and_keeps_signed_zero() {
        let mut mem = FlatMemory::new(globals::FRAME, (PASS_FLAGS - globals::FRAME + 4) as usize);
        let mut frame = [0; 36];
        frame[35] = (-0.0_f32).to_bits();
        assert_eq!(differences(&mem, 3, &frame), [
            "pass flags D3D 0x00000000 native 0x00000003",
            "c[39].w D3D 0x00000000 native 0x80000000",
        ]);
        mem.set_u32(PASS_FLAGS, 3).unwrap();
        mem.set_u32(globals::FRAME + 35 * 4, frame[35]).unwrap();
        assert!(differences(&mem, 3, &frame).is_empty());
    }
}
