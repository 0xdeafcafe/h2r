//! Context-only observers for effect leaves. No handler or simulation skip.
use crate::native_particles::{LOG_LIMIT, Producer, HOST};
use engine::points::{Point, Sig};
use native_render::effect_immediate::Site;
use std::sync::atomic::{AtomicU64, Ordering};

const DOC: &str = "docs/graphics/native-particles.md";
static CALLS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

const fn points(site: Site) -> (Site, Point, Point) {
    let (begin, end) = site.boundaries();
    (
        site,
        Point::new("EFFECT_IMMEDIATE_BEGIN", begin, Sig::Site, DOC),
        Point::new("EFFECT_IMMEDIATE_END", end, Sig::Site, DOC),
    )
}
static BOUNDARIES: [(Site, Point, Point); 5] = [
    points(Site::Billboard),
    points(Site::DepthBillboard),
    points(Site::WeatherLayer),
    points(Site::WeatherParticles),
    points(Site::MaterialStrip),
];
static REFUSALS: [(Point, &str); 3] = [
    (
        Point::new("EFFECT_QUERY", 0x0002_e9b6, Sig::Site, DOC),
        "VisibilityQuery",
    ),
    (
        Point::new("EFFECT_LAYERED_A", 0x0004_1cc0, Sig::Site, DOC),
        "OtherLaneLayeredGeometry",
    ),
    (
        Point::new("EFFECT_LAYERED_B", 0x0004_23c0, Sig::Site, DOC),
        "OtherLaneLayeredGeometry",
    ),
];
static CALLBACK_QUEUE: Point = Point::new("EFFECT_CALLBACK_QUEUE", 0x0004_1490, Sig::Site, DOC);
static CALLBACK_WRAPPER: Point =
    Point::new("EFFECT_CALLBACK_WRAPPER", 0x0004_8e40, Sig::Site, DOC);
pub(crate) fn instruction_entries() -> impl Iterator<Item = u32> {
    BOUNDARIES.iter().flat_map(|(_, begin, end)| [begin.base, end.base])
        .chain(REFUSALS.iter().map(|(point, _)| point.base))
}

pub(crate) fn build(b: &mut crate::Builder) {
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
        let count = AtomicU64::new(0);
        b.raw()
            .observe_before(point, "native_particles_effect_refused", move |_| {
                if count.fetch_add(1, Ordering::Relaxed) < LOG_LIMIT {
                    eprintln!("native-particles-refused: handler={address:#010x} reason={reason}");
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
