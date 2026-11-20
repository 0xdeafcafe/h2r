//! `native-decals`: scope dynamic decal callbacks for the ordered
//! `d3d::DrawAdmission` in boot. On by default (since 2026-11-20);
//! `H2_FEATURES=impl.render.native_decals=off` turns it off for the
//! comparison against the D3D path.
//!
//! The observers do not suppress guest code. `0x00044040` still binds every
//! texture, writes every duplicate marker and emits every draw. The admission
//! replaces only a fully resolved D3D draw packet, after two-copy recording
//! succeeds. Sorted handlers `0x00041cc0` and `0x000423c0` are object/layer
//! replays, not decals, and are deliberately left alone.
//!
//! `H2_NATIVE_DECALS_COMPARE=1`, `=mode`, or `=mode:list` prepares and logs
//! matching candidates without replacing D3D. Refusals are tallied by reason.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;
use native_render::decal::Mode;

use crate::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "native-decals";
pub const SWITCH: &str = "impl.render.native_decals";
const CURRENT_MODE: u32 = 0x004c_8790;

pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, true)
}

thread_local! {
    /// The guest callback currently emitting on this render thread.
    static CURRENT: Cell<Option<(Mode, i16)>> = const { Cell::new(None) };
}

static EXAMINED: AtomicU64 = AtomicU64::new(0);
static ADMITTED: AtomicU64 = AtomicU64::new(0);
static COMPARED: AtomicU64 = AtomicU64::new(0);
static REFUSALS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

/// `(examined draws, admitted, compared)` and the running refusal tally.
pub fn counts() -> ((u64, u64, u64), BTreeMap<&'static str, u64>) {
    (
        (
            EXAMINED.load(Ordering::Relaxed),
            ADMITTED.load(Ordering::Relaxed),
            COMPARED.load(Ordering::Relaxed),
        ),
        REFUSALS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    )
}

pub fn note_examined() {
    EXAMINED.fetch_add(1, Ordering::Relaxed);
}

pub fn note_admitted() {
    let admitted = ADMITTED.fetch_add(1, Ordering::Relaxed) + 1;
    if admitted % 1800 == 1 {
        let ((examined, admitted, compared), refusals) = counts();
        eprintln!(
            "native-decals: {admitted} of {examined} resolved draws native, {compared} compared; refusals {refusals:?}"
        );
    }
}

pub fn note_compared() {
    COMPARED.fetch_add(1, Ordering::Relaxed);
}

/// Adds an admission or recording failure to the same tally.
pub fn note_refusal(why: &'static str) {
    let total = {
        let mut refusals = REFUSALS.lock().unwrap_or_else(|e| e.into_inner());
        *refusals.entry(why).or_default() += 1;
        refusals.values().sum::<u64>()
    };
    if total % 1800 == 1 {
        let ((examined, admitted, compared), refusals) = counts();
        eprintln!(
            "native-decals: {admitted} of {examined} resolved draws native, {compared} compared; refusals {refusals:?}"
        );
    }
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
}

fn end() {
    CURRENT.with(|c| c.set(None));
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Compare {
    all: bool,
    mode: Option<i16>,
    list: Option<i16>,
}

impl Compare {
    fn matches(self, mode: Mode, list: i16) -> bool {
        self.all || (self.mode == Some(mode.index()) && self.list.is_none_or(|l| l == list))
    }
}

fn number(s: &str) -> Option<i16> {
    s.strip_prefix("0x").map_or_else(
        || s.parse::<i16>().ok(),
        |h| i16::from_str_radix(h, 16).ok(),
    )
}

fn compare() -> Option<Compare> {
    static COMPARE: OnceLock<Option<Compare>> = OnceLock::new();
    *COMPARE.get_or_init(|| {
        let value = std::env::var("H2_NATIVE_DECALS_COMPARE").ok()?;
        let c = match value.as_str() {
            "1" | "*" => Compare {
                all: true,
                ..Default::default()
            },
            value => {
                let mut parts = value.split(':');
                let mode = parts.next().and_then(number)?;
                let list = match parts.next() {
                    Some(v) => Some(number(v)?),
                    None => None,
                };
                if parts.next().is_some() {
                    return None;
                }
                Compare {
                    all: false,
                    mode: Some(mode),
                    list,
                }
            }
        };
        eprintln!("native-decals: compare {c:?}; matching resolved draws stay on D3D");
        Some(c)
    })
}

/// Whether this callback is compare-only (no native replacement).
pub fn comparing(mode: Mode, list: i16) -> bool {
    compare().is_some_and(|c| c.matches(mode, list))
}

pub struct NativeDecals;

impl Feature for NativeDecals {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Dynamic decals drawn by the native GPU renderer")
            .doc("docs/graphics/native-decals-research.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !on(b.switches()) && std::env::var_os("H2_NATIVE_DECALS_COMPARE").is_none() {
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
            .observe_after(&points::DECAL_RENDER, "native_decals_end", |_| end())
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_matches_the_compare_selector() {
        assert_eq!(number("6"), Some(6));
        assert_eq!(number("0x10"), Some(16));
        assert!(
            Compare {
                all: true,
                ..Default::default()
            }
            .matches(Mode::Opaque, 4)
        );
        assert!(
            Compare {
                all: false,
                mode: Some(6),
                list: None
            }
            .matches(Mode::Opaque, 4)
        );
        assert!(
            !Compare {
                all: false,
                mode: Some(6),
                list: Some(3)
            }
            .matches(Mode::Opaque, 4)
        );
        assert!(
            !Compare {
                all: false,
                mode: Some(5),
                list: None
            }
            .matches(Mode::Opaque, 4)
        );
    }
}
