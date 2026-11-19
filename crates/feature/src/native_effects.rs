//! Context-only observers for effect leaves. No handler or simulation skip.
use crate::native_particles::{compare_budget, Mode, Producer, HOST};
use engine::points::{Point, Sig};
use native_render::effect_immediate::Site;
use std::sync::atomic::{AtomicU64, Ordering};

const DOC: &str = "docs/graphics/native-particles.md";
static CALLS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

const fn points(site: Site) -> (Site, Point, Point) {
    let (begin, end) = site.boundaries();
    (
        site,
        Point::new("EFFECT_IMMEDIATE_BEGIN", begin, Sig::Opaque, DOC),
        Point::new("EFFECT_IMMEDIATE_END", end, Sig::Opaque, DOC),
    )
}
static BOUNDARIES: [(Site, Point, Point); 4] = [
    points(Site::Billboard),
    points(Site::DepthBillboard),
    points(Site::WeatherLayer),
    points(Site::WeatherParticles),
];
static REFUSALS: [(Point, &str); 5] = [
    (
        Point::new("EFFECT_QUERY", 0x0002_e9b6, Sig::Opaque, DOC),
        "VisibilityQuery",
    ),
    (
        Point::new("EFFECT_LAYERED_A", 0x0004_1cc0, Sig::Opaque, DOC),
        "OtherLaneLayeredGeometry",
    ),
    (
        Point::new("EFFECT_LAYERED_B", 0x0004_23c0, Sig::Opaque, DOC),
        "OtherLaneLayeredGeometry",
    ),
    (
        Point::new("EFFECT_CALLBACK_A", 0x0024_4610, Sig::Opaque, DOC),
        "UnownedMaterialCallback",
    ),
    (
        Point::new("EFFECT_CALLBACK_B", 0x002b_e650, Sig::Opaque, DOC),
        "UnownedMaterialCallback",
    ),
];
pub(crate) fn build(b: &mut crate::Builder) {
    for (index, (site, start, end)) in BOUNDARIES.iter().enumerate() {
        let site = *site;
        b.raw().observe_before(start, "native_particles_effect_begin", move |o| {
            let ordinal = CALLS[index].fetch_add(1, Ordering::Relaxed);
            let compare = compare_budget();
            if compare.is_some_and(|budget| ordinal >= budget) { return; }
            let mut mem = o.guest();
            let caller = site.caller_offset().and_then(|offset| o.cpu.esp.checked_add(offset)).and_then(|at| mem.u32(at).ok());
            if !site.accepts_caller(caller) {
                if ordinal < compare.unwrap_or(8) {
                    eprintln!("native-particles-refused: site={site:?} caller={caller:?} reason=UnownedCaller");
                }
                return;
            }
            let mode = if compare.is_some() { Mode::Compare } else { Mode::Native };
            let host = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(h) = host { h.arm_effect(&mut *mem, site, mode); }
        }).context_only();
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
        let count = AtomicU64::new(0);
        b.raw()
            .observe_before(point, "native_particles_effect_refused", move |_| {
                if count.fetch_add(1, Ordering::Relaxed) < compare_budget().unwrap_or(8) {
                    eprintln!("native-particles-refused: handler={address:#010x} reason={reason}");
                }
            })
            .context_only();
    }
}
