//! Context-only observers for effect leaves. No handler or simulation skip.
use crate::native_particles::{LOG_LIMIT, Producer, HOST};
use guest_layout::memory::GuestMemory;
use guest_layout::points::scene_effects::*;
use guest_layout::points::{Point, Sig};
use native_render::effect_immediate::Site;
use std::sync::atomic::{AtomicU64, Ordering};

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
static CALLBACK_QUEUE: Point = Point::new("EFFECT_CALLBACK_QUEUE", 0x0004_1490, Sig::Site, DOC);
static CALLBACK_WRAPPER: Point =
    Point::new("EFFECT_CALLBACK_WRAPPER", 0x0004_8e40, Sig::Site, DOC);

pub(crate) fn build(b: &mut hooks::Builder) {
    for (index, (site, start, end)) in BOUNDARIES.iter().enumerate() {
        let site = *site;
        b.raw().observe_before(start, "native_particles_effect_begin", move |o| {
            let ordinal = CALLS[index].fetch_add(1, Ordering::Relaxed);
            let mut mem = o.guest();
            let caller = site.caller_offset().and_then(|offset| o.cpu.esp.checked_add(offset)).and_then(|at| mem.u32(at).ok());
            if site == Site::Billboard && crate::native_flares::reflection_caller(caller) {
                if let Some(h) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() {
                    h.arm_flare(&mut *mem, native_render::flares::Site::Reflection);
                }
                return;
            }
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
                    // End returned and the helper popped ESI before this site.
                    let reflection = site == Site::Billboard && crate::native_flares::reflection_caller(
                        o.cpu.esp.checked_add(0x20).and_then(|at| mem.u32(at).ok())
                    );
                    let producer = if reflection {
                        Producer::Flare(native_render::flares::Site::Reflection)
                    } else {
                        Producer::Immediate(site)
                    };
                    h.finish(&mut *mem, producer);
                }
            })
            .context_only();
    }
    // Log the runtime callbacks whose geometry producer is unknown.
    let queued = AtomicU64::new(0);
    b.raw().observe_before(&CALLBACK_QUEUE, "native_particles_callback_queue", move |o| {
        if queued.load(Ordering::Relaxed) >= LOG_LIMIT {
            return;
        }
        let mem = o.guest();
        let callback = o.cpu.esp.checked_add(0x18).and_then(|at| mem.u32(at).ok());
        if !matches!(callback, Some(0x0024_4610 | 0x002b_e650)) && queued.fetch_add(1, Ordering::Relaxed) < LOG_LIMIT {
            eprintln!("native-particles-refused: queue=0x00041490 callback={callback:?} reason=UnownedMaterialCallback");
        }
    }).context_only().optional();
    b.raw().observe_before(&CALLBACK_WRAPPER, "native_particles_callback_wrapper", move |o| {
        let callback = wrapper_callback_at(&*o.guest(), o.cpu.esp);
        let wrapped = wrapper_callback(callback);
        WRAPPED[wrapped.index()].fetch_add(1, Ordering::Relaxed);
        if let Some(reason) = wrapped.refusal() {
            if REFUSED_WRAPPED.fetch_add(1, Ordering::Relaxed) < LOG_LIMIT {
                eprintln!("native-particles-refused: handler=0x00048e40 callback={callback:?} reason={reason}");
            }
        }
    }).context_only().optional();
}

/// The antenna widget's geometry callback. The `ant!` row of the widget
/// table `0x00467498` has draw `0x00116080`. For each antenna, that draw queues the sorted wrapper
/// `0x00048e40` through producer `0x00047870` (call `0x001160b3`). The callback is a bare
/// `ret 8`. It writes no vertex, sets no state and draws nothing, on the
/// Xbox or anywhere else.
pub const ANTENNA_GEOMETRY: u32 = 0x0024_dc50;

/// What the callback wrapper `0x00048e40` calls, after its own layer
/// setup (`[0x004670bc]` set, `0x00016b10` on `0x00485b48`). That setup stays the guest's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapperCallback {
    /// [`ANTENNA_GEOMETRY`]: the native path owns it. Its geometry is
    /// empty, so there is no draw to move to the GPU and none left on D3D.
    Antenna,
    /// A callback outside the researched set: refused by name.
    Unowned,
    /// The entry's data or callback word does not read.
    Unreadable,
}

impl WrapperCallback {
    fn index(self) -> usize {
        self as usize
    }

    /// The named refusal, `None` for an owned callback.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            Self::Antenna => None,
            Self::Unowned => Some("UnownedWrapperCallback"),
            Self::Unreadable => Some("UnreadableWrapperCallback"),
        }
    }
}

/// The callback the wrapper is about to call, read at its entry. The
/// wrapper's argument (ESP+4) is the entry's data `+0x24`: its `+0` and
/// `+4` are the callback's two arguments, and its `+8` is the callback.
pub fn wrapper_callback_at(mem: &dyn GuestMemory, esp: u32) -> Option<u32> {
    let data = mem.u32(esp.checked_add(4)?).ok()?;
    mem.u32(data.checked_add(8)?).ok()
}

/// The wrapper's callback by its address.
pub fn wrapper_callback(callback: Option<u32>) -> WrapperCallback {
    match callback {
        Some(ANTENNA_GEOMETRY) => WrapperCallback::Antenna,
        Some(_) => WrapperCallback::Unowned,
        None => WrapperCallback::Unreadable,
    }
}

static WRAPPED: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static REFUSED_WRAPPED: AtomicU64 = AtomicU64::new(0);

/// Wrapper calls seen so far, as `(antenna, unowned, unreadable)`.
pub fn wrapper_counts() -> (u64, u64, u64) {
    let n = |w: WrapperCallback| WRAPPED[w.index()].load(Ordering::Relaxed);
    (n(WrapperCallback::Antenna), n(WrapperCallback::Unowned), n(WrapperCallback::Unreadable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_antenna_callback_is_owned_with_no_geometry() {
        assert_eq!(wrapper_callback(Some(0x0024_dc50)), WrapperCallback::Antenna);
        assert_eq!(WrapperCallback::Antenna.refusal(), None);
    }

    #[test]
    fn other_wrapper_callbacks_keep_a_named_refusal() {
        for callback in [0x0024_dc53, 0x0024_4610, 0x0011_6080, 0] {
            assert_eq!(wrapper_callback(Some(callback)), WrapperCallback::Unowned);
        }
        assert_eq!(WrapperCallback::Unowned.refusal(), Some("UnownedWrapperCallback"));
        assert_eq!(wrapper_callback(None), WrapperCallback::Unreadable);
        assert_eq!(WrapperCallback::Unreadable.refusal(), Some("UnreadableWrapperCallback"));
    }

    #[test]
    fn the_callback_is_read_through_the_wrappers_argument() {
        use guest_layout::memory::FlatMemory;
        const ESP: u32 = 0x0010_0000;
        const DATA: u32 = 0x0010_0100;
        let mut m = FlatMemory::new(0x0010_0000, 0x1000);
        m.set_u32(ESP + 4, DATA).unwrap();
        m.set_u32(DATA, 0x0001_2345).unwrap();
        m.set_u32(DATA + 4, 0x0006_789a).unwrap();
        m.set_u32(DATA + 8, ANTENNA_GEOMETRY).unwrap();
        assert_eq!(wrapper_callback_at(&m, ESP), Some(ANTENNA_GEOMETRY));
        assert_eq!(wrapper_callback(wrapper_callback_at(&m, ESP)), WrapperCallback::Antenna);
        m.set_u32(ESP + 4, 0x0020_0000).unwrap();
        assert_eq!(wrapper_callback(wrapper_callback_at(&m, ESP)), WrapperCallback::Unreadable);
    }

    #[test]
    fn the_counts_index_each_variant_once() {
        let all = [WrapperCallback::Antenna, WrapperCallback::Unowned, WrapperCallback::Unreadable];
        let mut seen = all.map(WrapperCallback::index);
        seen.sort_unstable();
        assert_eq!(seen, [0, 1, 2]);
        assert!(seen.iter().all(|&i| i < WRAPPED.len()));
    }
}
