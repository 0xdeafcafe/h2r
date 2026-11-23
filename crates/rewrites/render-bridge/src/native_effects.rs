//! Context-only observers for effect leaves. No handler or simulation skip.
use crate::native_particles::{LOG_LIMIT, Producer, HOST};
use engine::points::scene_effects::*;
use engine::points::{Point, Sig};
use native_render::effect_immediate::Site;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DOC: &str = "docs/graphics/native-particles.md";
static CALLS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

const fn points(site: Site, leaf: EffectLeaf) -> (Site, Point, Point) {
    (site, leaf.begin, leaf.end)
}
static BOUNDARIES: [(Site, Point, Point); 5] = [
    points(Site::Billboard, EFFECT_BILLBOARD),
    points(Site::DepthBillboard, EFFECT_DEPTH_BILLBOARD),
    points(Site::WeatherLayer, EFFECT_WEATHER_LAYER),
    points(Site::WeatherParticles, EFFECT_WEATHER_PARTICLES),
    points(Site::MaterialStrip, EFFECT_MATERIAL_STRIP),
];
static REFUSALS: [(Point, &str); 3] = [
    (EFFECT_QUERY, "VisibilityQuery"),
    (EFFECT_LAYERED_A, "OtherLaneLayeredGeometry"),
    (EFFECT_LAYERED_B, "OtherLaneLayeredGeometry"),
];
static CALLBACK_QUEUE: Point = Point::new("EFFECT_CALLBACK_QUEUE", 0x0004_1490, Sig::Site, DOC);
static CALLBACK_WRAPPER: Point =
    Point::new("EFFECT_CALLBACK_WRAPPER", 0x0004_8e40, Sig::Site, DOC);

/// A handler refusal's line: printed on the first call and then at most
/// every [`REFUSAL_EVERY`], with the calls so far, however often the
/// handler runs (a handler that refuses on every frame logs once per
/// window, not per frame).
pub(crate) struct RefusalLog {
    calls: AtomicU64,
    last: Mutex<Option<Instant>>,
}

pub(crate) const REFUSAL_EVERY: Duration = Duration::from_secs(10);

impl RefusalLog {
    pub(crate) const fn new() -> Self {
        Self { calls: AtomicU64::new(0), last: Mutex::new(None) }
    }

    /// Counts a call; the call count when this one should print.
    pub(crate) fn note(&self, now: Instant) -> Option<u64> {
        let calls = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| now.duration_since(t) < REFUSAL_EVERY) {
            return None;
        }
        *last = Some(now);
        Some(calls)
    }
}

pub(crate) fn build(b: &mut hooks::Builder) {
    for (index, (site, start, end)) in BOUNDARIES.iter().enumerate() {
        let site = *site;
        b.raw().observe_before(start, "native_particles_effect_begin", move |o| {
            let ordinal = CALLS[index].fetch_add(1, Ordering::Relaxed);
            let mut mem = o.guest();
            let caller = site.caller_offset().and_then(|offset| o.cpu.esp.checked_add(offset)).and_then(|at| mem.u32(at).ok());
            if !site.accepts_caller(caller) {
                if ordinal < LOG_LIMIT {
                    eprintln!("native-particles-refused: site={site:?} caller={caller:?} reason=UnownedCaller");
                }
                return;
            }
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(h) = host { h.arm_effect(&mut *mem, site); }
        }).context_only().optional();
        b.raw()
            .observe_before(end, "native_particles_effect_end", move |o| {
                let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Some(h) = host {
                    let mut mem = o.guest();
                    h.finish(&mut *mem, Producer::Immediate(site));
                }
            })
            .context_only();
    }
    // Explicit refusal diagnostics, never drawing or skipping these bodies.
    for (point, reason) in &REFUSALS {
        let address = point.base;
        let log = RefusalLog::new();
        b.raw()
            .observe_before(point, "native_particles_effect_refused", move |_| {
                if let Some(calls) = log.note(Instant::now()) {
                    eprintln!("native-particles-refused: handler={address:#010x} reason={reason} ({calls} calls so far)");
                }
            })
            .context_only();
    }
    let queued = AtomicU64::new(0);
    b.raw().observe_before(&CALLBACK_QUEUE, "native_particles_callback_queue", move |o| {
        let mem = o.guest();
        let callback = o.cpu.esp.checked_add(0x18).and_then(|at| mem.u32(at).ok());
        if !matches!(callback, Some(0x0024_4610 | 0x002b_e650)) && queued.fetch_add(1, Ordering::Relaxed) < LOG_LIMIT {
            eprintln!("native-particles-refused: queue=0x00041490 callback={callback:?} reason=UnownedMaterialCallback");
        }
    }).context_only().optional();
    let wrapped = AtomicU64::new(0);
    b.raw().observe_before(&CALLBACK_WRAPPER, "native_particles_callback_wrapper", move |o| {
        let mem = o.guest();
        let callback = o.cpu.esp.checked_add(4).and_then(|at| mem.u32(at).ok()).and_then(|data| data.checked_add(8)).and_then(|at| mem.u32(at).ok());
        if wrapped.fetch_add(1, Ordering::Relaxed) < LOG_LIMIT {
            let reason = if callback == Some(0x0024_dc50) { "NoGeometryCallback" } else { "UnownedMaterialCallback" };
            eprintln!("native-particles-refused: handler=0x00048e40 callback={callback:?} reason={reason}");
        }
    }).context_only().optional();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_prints_first_then_once_per_window_with_its_count() {
        let log = RefusalLog::new();
        let t = Instant::now();
        assert_eq!(log.note(t), Some(1), "the first call prints");
        // A handler that refuses every frame for nine seconds prints nothing more.
        for frame in 1..=540 {
            assert_eq!(log.note(t + Duration::from_millis(frame * 16)), None, "frame {frame}");
        }
        assert_eq!(log.note(t + REFUSAL_EVERY), Some(542), "the window's end prints the total");
        assert_eq!(log.note(t + REFUSAL_EVERY + Duration::from_secs(1)), None);
    }
}
