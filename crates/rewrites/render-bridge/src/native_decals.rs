//! `native-decals`: scope dynamic decal callbacks for the ordered
//! `d3d::DrawAdmission` in boot. On with the native renderer
//! (`impl.render.native`).
//!
//! The callback at `0x00044040` binds textures, marks duplicates and issues
//! one array draw per visible record. That call resolves once for ordered
//! native admission and the original D3D fallback.
//!
//! The stderr tally reports resolved draws per present and guest pool
//! occupancy, with the process totals labelled separately. `render.stats`
//! counts committed draws after all attachment copies have recorded.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use engine::datum::{DatumArray, SIGNATURE, layout};
use engine::memory::GuestMemory;
use engine::points;
use native_render::decal::Mode;

use hooks::{BuildError, Builder, Feature, Manifest};
use timeline::native::{Metric, Stage};

pub const ID: &str = "native-decals";

const CURRENT_MODE: u32 = 0x004c_8790;

pub fn on(switches: &hooks::Switches) -> bool {
    hooks::switches::native_render_on(switches)
}

thread_local! {
    /// The guest callback currently emitting on this render thread.
    static CURRENT: Cell<Option<(Mode, i16)>> = const { Cell::new(None) };
    static LAST_PRESENT: Cell<Option<u64>> = const { Cell::new(None) };
    /// The render camera's matrix block when the callback began.
    static VIEW: Cell<Option<native_render::decal_material::View>> = const { Cell::new(None) };
}

/// The view the callback emitting on this thread draws in, as Rust gives
/// its matrix constants.
pub fn view() -> Option<native_render::decal_material::View> {
    VIEW.with(Cell::get)
}

struct Ro<'a>(&'a dyn GuestMemory);

impl scene_extract::GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

static EXAMINED: AtomicU64 = AtomicU64::new(0);
static CANDIDATES: AtomicU64 = AtomicU64::new(0);
static REFUSALS: Mutex<BTreeMap<Cow<'static, str>, u64>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy)]
struct PresentSample {
    present: u64,
    examined: u64,
}

impl PresentSample {
    fn since(self, earlier: Self) -> Option<(u64, u64)> {
        let presents = self.present.checked_sub(earlier.present)?;
        let draws = self.examined.checked_sub(earlier.examined)?;
        (presents != 0).then_some((presents, draws))
    }
}

static LAST_LOG: Mutex<Option<(Instant, PresentSample)>> = Mutex::new(None);
const LOG_EVERY: Duration = Duration::from_secs(10);
const DECAL_DATUM_ARRAY: u32 = 0x004e_a950;

/// Whether the scene's decal pass draws the view's decals: the guest's
/// decal callback `0x00044040` is then cut, and nothing else of the decal
/// path (the sorted-list replays `0x00041cc0` and `0x000423c0` draw
/// object and camouflage layers, not decals) is the decals'. A host sets
/// it when it reserves the pass in slots 2, 5 and 7
/// (`native_render::decal_scene`); it stays false until one does.
static SCENE_DRAWS: AtomicBool = AtomicBool::new(false);

pub fn scene_draws() -> bool {
    SCENE_DRAWS.load(Ordering::Relaxed)
}

pub fn set_scene_draws(on: bool) {
    SCENE_DRAWS.store(on, Ordering::Relaxed);
}

/// The proof of the Rust decal material (`native_render::decal_material`)
/// against the guest's own: one resolved draw in [`SAMPLE`] is compared,
/// and the material is proven after [`CLEAN`] clean samples in a row. Any
/// difference un-proves it and counts each differing field; the guest's
/// state is what draws until it is proven again.
pub const SAMPLE: u64 = 64;
pub const CLEAN: u32 = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proof {
    seen: u64,
    pub sampled: u64,
    pub clean: u64,
    run: u32,
    pub mismatched: u64,
    pub fields: BTreeMap<String, u64>,
}

impl Proof {
    /// Whether this draw is to be compared (the first, then 1 in [`SAMPLE`]).
    pub fn sample(&mut self) -> bool {
        self.seen += 1;
        (self.seen - 1) % SAMPLE == 0
    }

    /// Records a sample's differing fields.
    pub fn record(&mut self, fields: &[String]) {
        self.sampled += 1;
        if fields.is_empty() {
            self.clean += 1;
            self.run = (self.run + 1).min(CLEAN);
            return;
        }
        self.run = 0;
        self.mismatched += 1;
        for f in fields {
            *self.fields.entry(f.clone()).or_default() += 1;
        }
    }

    pub fn proven(&self) -> bool {
        self.run >= CLEAN
    }
}

static PROOF: Mutex<Proof> = Mutex::new(Proof {
    seen: 0,
    sampled: 0,
    clean: 0,
    run: 0,
    mismatched: 0,
    fields: BTreeMap::new(),
});
static OFFSET_BITS: AtomicU64 = AtomicU64::new(0);
/// The slot-3 programs and the datum they were read for: read again when
/// the rasterizer's datum changes (a new map).
static PROGRAMS: Mutex<Option<(u32, Vec<Vec<u8>>)>> = Mutex::new(None);

/// Whether `code` is one of the engine slot's programs read from the tags;
/// `None` when they did not read.
pub fn program_in_tags(code: &[u8]) -> Option<bool> {
    let programs = PROGRAMS.lock().unwrap_or_else(|e| e.into_inner());
    programs.as_ref().map(|(_, p)| native_render::decal_material::program_matches(p, code))
}

fn read_programs(mem: &dyn GuestMemory) {
    use native_render::decal_material::{RASTERIZER, vertex_programs};
    let word = |a: u32| mem.u32(a).ok();
    let datum = word(RASTERIZER)
        .and_then(|r| word(r.checked_add(0x5c)?))
        .and_then(|t| word(t.checked_add(3 * 8 + 4)?));
    let mut programs = PROGRAMS.lock().unwrap_or_else(|e| e.into_inner());
    if datum.is_some() && programs.as_ref().map(|p| p.0) == datum {
        return;
    }
    *programs = vertex_programs(&word, &|a, n| mem.bytes(a, n).ok());
}

/// The polygon offset constant the wrapper loaded (`[0x0045e48c]`), read at
/// the callback's entry.
pub fn offset_bits() -> u32 {
    OFFSET_BITS.load(Ordering::Relaxed) as u32
}

/// Whether the draw in `mode` is to be compared with the Rust material.
pub fn proof_sample() -> bool {
    PROOF.lock().unwrap_or_else(|e| e.into_inner()).sample()
}

/// Records a compared draw's differing fields.
pub fn proof_record(fields: &[String]) {
    PROOF.lock().unwrap_or_else(|e| e.into_inner()).record(fields);
}

/// Whether the Rust material is proven.
pub fn material_proven() -> bool {
    PROOF.lock().unwrap_or_else(|e| e.into_inner()).proven()
}

/// [`proof_counters`] with static names (each distinct name is leaked once).
pub fn proof_rows() -> Vec<(&'static str, u64)> {
    static NAMES: Mutex<BTreeMap<String, &'static str>> = Mutex::new(BTreeMap::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    proof_counters()
        .into_iter()
        .map(|(n, v)| (*names.entry(n.clone()).or_insert_with(|| Box::leak(n.into_boxed_str())), v))
        .collect()
}

/// `render.scene` rows: samples, clean samples, whether proven, and each
/// differing field.
pub fn proof_counters() -> Vec<(String, u64)> {
    let p = PROOF.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = vec![
        ("decal_material_sampled".to_owned(), p.sampled),
        ("decal_material_clean".to_owned(), p.clean),
        ("decal_material_mismatched".to_owned(), p.mismatched),
        ("decal_material_proven".to_owned(), u64::from(p.proven())),
    ];
    out.extend(p.fields.iter().map(|(f, n)| (format!("decal_material_mismatch_{f}"), *n)));
    out
}

/// `(examined draws, native candidates)` and the running refusal tally.
pub fn counts() -> ((u64, u64), BTreeMap<Cow<'static, str>, u64>) {
    (
        (
            EXAMINED.load(Ordering::Relaxed),
            CANDIDATES.load(Ordering::Relaxed),
        ),
        REFUSALS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    )
}

fn pool_counts(mem: &dyn GuestMemory) -> Option<(i32, i32)> {
    let header = mem.u32(DECAL_DATUM_ARRAY).ok()?;
    if header == 0 || mem.u32(header.checked_add(layout::SIGNATURE)?).ok()? != SIGNATURE {
        return None;
    }
    let array = DatumArray::at(header);
    let live = array.header_field(mem, layout::ACTIVE_COUNT).ok()?;
    let capacity = array.header_field(mem, layout::MAXIMUM_COUNT).ok()?;
    (0..=capacity).contains(&live).then_some((live, capacity))
}

fn log_completed_presents(mem: &dyn GuestMemory, present: u64) {
    let mut last = LAST_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let sample = PresentSample {
        present,
        examined: EXAMINED.load(Ordering::Relaxed),
    };
    let Some((time, earlier)) = *last else {
        *last = Some((now, sample));
        return;
    };
    if now.duration_since(time) < LOG_EVERY {
        return;
    }
    let Some((presents, draws)) = sample.since(earlier) else {
        return;
    };
    *last = Some((now, sample));
    drop(last);
    let per_present = draws as f64 / presents as f64;
    let pool = pool_counts(mem)
        .map_or_else(|| "unavailable".into(), |(live, max)| format!("{live}/{max}"));
    let ((examined, candidates), refusals) = counts();
    eprintln!(
        "native-decals: {draws} resolved draws in {presents} presents ({per_present:.1}/present), guest pool {pool}. Lifetime: {candidates} native candidates of {examined} resolved draws, refusals {refusals:?}"
    );
}

/// The stage metric for an admission rejected before the backend sees it.
fn refused_metric(why: &str) -> Metric {
    match why.split_once(':').map_or(why, |(reason, _)| reason) {
        "geometry" | "vertex streams" | "vertex declaration" | "vertex bytes" => {
            Metric::RefusedInputs
        }
        "texture kind" | "texture view" | "texture is the target" | "texture group" => {
            Metric::RefusedResources
        }
        _ => Metric::RefusedValidation,
    }
}

pub fn note_examined() {
    EXAMINED.fetch_add(1, Ordering::Relaxed);
}

pub fn note_candidate() {
    CANDIDATES.fetch_add(1, Ordering::Relaxed);
}

/// The backend accounts for candidates it receives. Account for early
/// refusals here so each draw contributes one stage examination.
pub fn note_admission_refusal(why: impl Into<Cow<'static, str>>) {
    let why = why.into();
    Stage::Decals.count(Metric::Examined, 1);
    Stage::Decals.count(refused_metric(&why), 1);
    note_refusal(why);
}

/// Adds an admission or recording failure to the same tally.
pub fn note_refusal(why: impl Into<Cow<'static, str>>) {
    let mut refusals = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
    *refusals.entry(why.into()).or_default() += 1;
}

/// The callback emitting on this thread, if any.
pub fn current() -> Option<(Mode, i16)> {
    CURRENT.with(Cell::get)
}

fn begin(mem: &dyn GuestMemory, list: i16) {
    read_programs(mem);
    if let Ok(bits) = mem.u32(native_render::decal_material::OFFSET_CONSTANT_ADDRESS) {
        OFFSET_BITS.store(u64::from(bits), Ordering::Relaxed);
    }
    let mode = mem
        .u16(CURRENT_MODE)
        .ok()
        .and_then(|v| Mode::from_guest(v as i16));
    CURRENT.with(|c| c.set(mode.map(|m| (m, list))));
    let view = scene_extract::extract::camera(&Ro(mem))
        .and_then(|c| native_render::decal_material::View::from_camera(&c));
    VIEW.with(|v| v.set(view));
    let present = timeline::frames();
    if LAST_PRESENT.with(|last| last.replace(Some(present)) != Some(present)) {
        log_completed_presents(mem, present);
    }
}

fn end() {
    CURRENT.with(|c| c.set(None));
}

pub struct NativeDecals;

impl Feature for NativeDecals {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Dynamic decals drawn by the native GPU renderer")
            .doc("docs/graphics/native-decals-research.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::DECAL_RENDER, "native_decals_begin", |o| {
                let Some(list) = o.arg_u32("list").map(|v| v as u16 as i16) else {
                    return;
                };
                let mem = o.guest();
                begin(&*mem, list);
            })
            .context_only();
        b.raw()
            .observe_before(&points::scene_effects::DECAL_RENDER_END, "native_decals_end", |_| end())
            .context_only();
        // The weather plates' host draws in the same pass (slot 9).
        crate::native_weather::build(b);
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_present_window_reports_only_draws_in_that_window() {
        let first = PresentSample {
            present: 1_000,
            examined: 30_000,
        };
        let middle = PresentSample {
            present: 1_100,
            examined: 33_000,
        };
        let last = PresentSample {
            present: 1_200,
            examined: 36_000,
        };
        assert_eq!(middle.since(first), Some((100, 3_000)));
        assert_eq!(last.since(middle), Some((100, 3_000)));
        assert_eq!(last.since(last), None);
    }

    #[test]
    fn decal_pool_reports_only_valid_live_and_capacity_counts() {
        use engine::memory::FlatMemory;
        let mut mem = FlatMemory::new(DECAL_DATUM_ARRAY, 0x100);
        let header = DECAL_DATUM_ARRAY + 0x40;
        assert_eq!(pool_counts(&mem), None);
        mem.set_u32(DECAL_DATUM_ARRAY, header).unwrap();
        mem.set_u32(header + layout::SIGNATURE, SIGNATURE).unwrap();
        mem.set_i32(header + layout::MAXIMUM_COUNT, 10).unwrap();
        mem.set_i32(header + layout::ACTIVE_COUNT, 4).unwrap();
        assert_eq!(pool_counts(&mem), Some((4, 10)));
        mem.set_i32(header + layout::ACTIVE_COUNT, 11).unwrap();
        assert_eq!(pool_counts(&mem), None);
    }

    #[test]
    fn the_material_is_proven_after_eight_clean_samples_and_a_difference_resets_it() {
        let mut p = Proof::default();
        assert!(p.sample());
        assert!((1..SAMPLE).all(|_| !p.sample()));
        assert!(p.sample());
        for _ in 0..CLEAN - 1 {
            p.record(&[]);
        }
        assert!(!p.proven());
        p.record(&[]);
        assert!(p.proven());
        p.record(&["combiner_w53".to_owned(), "render_0x4a".to_owned()]);
        assert!(!p.proven());
        assert_eq!((p.sampled, p.clean, p.mismatched), (CLEAN as u64 + 1, CLEAN as u64, 1));
        assert_eq!(p.fields["combiner_w53"], 1);
    }

    #[test]
    fn refusal_reasons_map_to_stage_metrics() {
        assert!(matches!(refused_metric("geometry"), Metric::RefusedInputs));
        assert!(matches!(refused_metric("vertex streams"), Metric::RefusedInputs));
        assert!(matches!(refused_metric("vertex declaration: synthetic layout"), Metric::RefusedInputs));
        assert!(matches!(refused_metric("vertex bytes"), Metric::RefusedInputs));
        assert!(matches!(refused_metric("texture kind"), Metric::RefusedResources));
        assert!(matches!(refused_metric("texture is the target"), Metric::RefusedResources));
        assert!(matches!(refused_metric("blend state"), Metric::RefusedValidation));
        assert!(matches!(refused_metric("vertex program"), Metric::RefusedValidation));
    }
}
