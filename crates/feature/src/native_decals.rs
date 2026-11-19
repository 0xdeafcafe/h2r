//! `native-decals`: the seven dynamic decal modes drawn by the native GPU
//! renderer. Off by default; `H2_FEATURES=impl.render.native_decals=on`.
//!
//! Only `0x00044040` is hooked, after its `0x00043a40` state wrapper and
//! before its first record. The sorted handlers `0x00041cc0` and `0x000423c0`
//! are object/layer replays, not decals, and are deliberately left alone.
//! A complete ordered [`Snapshot`] must resolve and queue before the hook
//! answers `ret 8`; every refusal leaves the entire callback on D3D and makes
//! none of the guest's duplicate-marker writes.
//!
//! `H2_NATIVE_DECALS_COMPARE=1`, `=mode`, or `=mode:list` prepares and logs
//! matching native candidates without queueing or suppressing D3D. Refusals
//! are tallied by their stable reason string.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use engine::memory::GuestMemory;
use engine::points;
use native_render::decal::{self, Memory, Snapshot};

use crate::{BuildError, Builder, Feature, Flow, Manifest};

pub const ID: &str = "native-decals";
pub const SWITCH: &str = "impl.render.native_decals";

pub fn on(switches: &crate::Switches) -> bool {
    switches.feature(SWITCH, false)
}

/// Boot's device, asset and GPU side.
pub trait Host: Send + Sync {
    /// Resolves the complete snapshot. With `draw`, also requests its ordered
    /// native pass. Without it, this is compare-only and D3D still draws.
    fn admit(
        &self,
        mem: &dyn GuestMemory,
        snapshot: Snapshot,
        draw: bool,
    ) -> Result<(), &'static str>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);
static CALLBACKS: AtomicU64 = AtomicU64::new(0);
static ADMITTED: AtomicU64 = AtomicU64::new(0);
static COMPARED: AtomicU64 = AtomicU64::new(0);
static REFUSALS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

/// `(callbacks, admitted, compared)` and the running refusal tally.
pub fn counts() -> ((u64, u64, u64), BTreeMap<&'static str, u64>) {
    (
        (
            CALLBACKS.load(Ordering::Relaxed),
            ADMITTED.load(Ordering::Relaxed),
            COMPARED.load(Ordering::Relaxed),
        ),
        REFUSALS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    )
}

fn refuse(why: &'static str) {
    *REFUSALS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(why)
        .or_default() += 1;
}

/// Adds an asynchronous admission/encode failure to the same tally. The
/// handoff can use this when a late backend refusal becomes observable.
pub fn note_refusal(why: &'static str) {
    refuse(why);
}

struct Ro<'a>(&'a dyn GuestMemory);

impl Ro<'_> {
    fn bytes(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.0.bytes(addr, len).ok()
    }
}

impl Memory for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }

    fn read_u16(&self, addr: u32) -> Option<u16> {
        self.0.u16(addr).ok()
    }

    fn read_bytes(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.bytes(addr, len)
    }

    fn write_u16(&mut self, _addr: u32, _value: u16) {
        unreachable!("read-only snapshot adapter")
    }
}

struct Rw<'a>(&'a mut dyn GuestMemory);

impl Memory for Rw<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }

    fn read_bytes(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.0.bytes(addr, len).ok()
    }

    fn write_u16(&mut self, addr: u32, value: u16) {
        // Every address was read during preparation. As with the other
        // context-only native hooks, a store to that same snapshot is
        // expected to succeed; there is no safe D3D replay after admission.
        let _ = self.0.set_u16(addr, value);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Compare {
    all: bool,
    mode: Option<i16>,
    list: Option<i16>,
}

impl Compare {
    fn matches(self, snapshot: &Snapshot) -> bool {
        self.all
            || (self.mode == Some(snapshot.mode.index())
                && self.list.is_none_or(|l| l == snapshot.list))
    }
}

fn number(s: &str) -> Option<i16> {
    let v = s.strip_prefix("0x").map_or_else(
        || s.parse::<i16>().ok(),
        |h| i16::from_str_radix(h, 16).ok(),
    )?;
    Some(v)
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
        eprintln!("native-decals: compare {c:?}; matching callbacks stay on D3D");
        Some(c)
    })
}

/// The callback's `before`: `true` only after complete native admission and
/// reproduction of the guest's marker writes.
pub fn on_callback(mem: &mut dyn GuestMemory, list: i16) -> bool {
    CALLBACKS.fetch_add(1, Ordering::Relaxed);
    let snapshot = match decal::read_snapshot(&Ro(mem), list) {
        Ok(s) => s,
        Err(why) => {
            refuse(why);
            return false;
        }
    };
    // An empty callback has no draws or marker writes to replace.
    if snapshot.draws.is_empty() {
        return false;
    }
    let Some(host) = HOST.lock().unwrap_or_else(|e| e.into_inner()).clone() else {
        return false;
    };
    if compare().is_some_and(|c| c.matches(&snapshot)) {
        COMPARED.fetch_add(1, Ordering::Relaxed);
        let result = host.admit(mem, snapshot.clone(), false);
        eprintln!(
            "native-decals: compare mode {} list {}: {} draws, {} quads, records {:?}: {}",
            snapshot.mode.index(),
            snapshot.list,
            snapshot.draws.len(),
            snapshot.quad_count(),
            snapshot.draws.iter().map(|d| d.record).collect::<Vec<_>>(),
            result.as_ref().map_or_else(|why| *why, |_| "prepared")
        );
        return false;
    }
    match host.admit(mem, snapshot.clone(), true) {
        Ok(()) => {
            snapshot.commit(&mut Rw(mem));
            let admitted = ADMITTED.fetch_add(1, Ordering::Relaxed) + 1;
            if admitted % 1800 == 1 {
                let ((callbacks, admitted, compared), refusals) = counts();
                eprintln!(
                    "native-decals: {admitted} of {callbacks} callbacks native, {compared} compared; refusals {refusals:?}"
                );
            }
            true
        }
        Err(why) => {
            refuse(why);
            false
        }
    }
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
            .before(&points::DECAL_RENDER, "native_decals_skip", |c| {
                let Some(list) = c.arg_u32("list").map(|v| v as u16 as i16) else {
                    return Ok(Flow::Continue);
                };
                let rt = Arc::clone(c.ctx.runtime());
                let skip = {
                    let mut mem = rt.memory();
                    on_callback(&mut *mem, list)
                };
                if skip {
                    c.ret(8).map(Flow::Return)
                } else {
                    Ok(Flow::Continue)
                }
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_compare_selector() {
        assert_eq!(number("6"), Some(6));
        assert_eq!(number("0x10"), Some(16));
        assert!(
            Compare {
                all: true,
                ..Default::default()
            }
            .matches(&Snapshot {
                mode: decal::Mode::Opaque,
                list: 4,
                draws: vec![native_render::decal::Draw {
                    record: 1,
                    marker: 0,
                    bitmap: 0,
                    selector: 0,
                    image: None,
                    texture_header: None,
                    default_color: [0; 4],
                    vertices: Vec::new(),
                }],
                frame: 0,
            })
        );
        assert!(
            Compare {
                all: false,
                mode: Some(6),
                list: None
            }
            .matches(&Snapshot {
                mode: decal::Mode::Opaque,
                list: 4,
                draws: Vec::new(),
                frame: 0,
            })
        );
        assert!(
            !Compare {
                all: false,
                mode: Some(6),
                list: Some(3)
            }
            .matches(&Snapshot {
                mode: decal::Mode::Opaque,
                list: 4,
                draws: Vec::new(),
                frame: 0,
            })
        );
    }
}
