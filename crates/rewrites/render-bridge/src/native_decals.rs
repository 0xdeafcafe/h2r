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
use std::sync::atomic::{AtomicU64, Ordering};
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
    let mode = mem
        .u16(CURRENT_MODE)
        .ok()
        .and_then(|v| Mode::from_guest(v as i16));
    CURRENT.with(|c| c.set(mode.map(|m| (m, list))));
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
