//! The sorted list's order made in Rust, checked against the guest's sort
//! (native-only plan, `nor-7a`; docs/graphics/native-transparents.md, "The
//! sort in Rust").
//!
//! | Guest | Cut |
//! |---|---|
//! | `0x0001e370`'s index fill and `TRANSPARENT_SORT` (`0x0013df80`, comparator `0x0001e700`), `TRANSPARENT_ORDER_BEGIN..TRANSPARENT_ORDER_READY` | the live rows' order from `native_scene_lists::transparents`, written to `[0x004b628c]`, then a jump to `TRANSPARENT_ORDER_READY` |
//!
//! The list has two callers: slot 9's sorted transparents (`0x0002c19a`,
//! returning to `0x0002c19f`) and the first person's sorted pass
//! `0x00040870` in slot 10 (returning to `0x00040875`). [`Pass`] tells
//! them apart by the return address, which sits [`RETURN_OFFSET`] above
//! `esp` at both sites. A call from anywhere else runs the guest body.
//!
//! The arm: one call in [`CHECK_EVERY`] of each pass runs the guest's fill
//! and sort, and at `TRANSPARENT_ORDER_READY` the order it wrote is
//! compared with the one Rust made. A difference sends every later call of
//! both passes to the guest sort for the session, counted
//! `scene_kept_transparent_sort_<pass>_mismatch`. A call the cut does not
//! make runs the guest body, counted `scene_kept_transparent_sort_<pass>_<cause>`.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use guest_layout::memory::GuestMemory;
use frame_scene::ViewSlot;

/// One call of each pass in this many runs the guest sort and is compared.
pub const CHECK_EVERY: u64 = 64;

/// Where slot 9's call `0x0002c19a` returns.
pub const WORLD_RETURN: u32 = 0x0002_c19f;
/// Where `0x00040870`'s call returns (slot 10).
pub const FIRST_PERSON_RETURN: u32 = 0x0004_0875;
/// The return address's place above `esp` at `TRANSPARENT_ORDER_BEGIN`
/// and `TRANSPARENT_ORDER_READY`: three saved registers and 8 bytes of
/// locals (`0x0001e370..0x0001e375`); the sort pops its own arguments.
pub const RETURN_OFFSET: u32 = 0x14;

/// Which call of the sorted list this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// Slot 9: the world's sorted transparents and layer 15.
    World,
    /// Slot 10: the first person's sorted pass `0x00040870`.
    FirstPerson,
    /// A caller the call scan does not name.
    Other,
}

impl Pass {
    pub fn of_return(ret: u32) -> Self {
        match ret {
            WORLD_RETURN => Self::World,
            FIRST_PERSON_RETURN => Self::FirstPerson,
            _ => Self::Other,
        }
    }

    /// The pass of the call whose `esp` at either order site is `esp`.
    pub fn at(mem: &dyn GuestMemory, esp: u32) -> Self {
        esp.checked_add(RETURN_OFFSET)
            .and_then(|at| mem.u32(at).ok())
            .map_or(Self::Other, Self::of_return)
    }

    /// The view slot the pass's entries draw in.
    pub fn slot(self) -> Option<ViewSlot> {
        match self {
            Self::World => Some(ViewSlot::Transparents),
            Self::FirstPerson => Some(ViewSlot::FirstPerson),
            Self::Other => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::World => "world",
            Self::FirstPerson => "first_person",
            Self::Other => "other",
        }
    }
}

/// What one call does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Publish Rust's order and jump past the guest sort.
    Cut,
    /// Run the guest's fill and sort.
    Guest,
}

/// One pass's counts.
pub struct Arm {
    pass: Pass,
    calls: AtomicU64,
    cut: AtomicU64,
    checked: AtomicU64,
    kept: Mutex<BTreeMap<&'static str, u64>>,
}

impl Arm {
    pub const fn new(pass: Pass) -> Self {
        Self {
            pass,
            calls: AtomicU64::new(0),
            cut: AtomicU64::new(0),
            checked: AtomicU64::new(0),
            kept: Mutex::new(BTreeMap::new()),
        }
    }

    fn keep(&self, cause: &'static str) {
        *self
            .kept
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(cause)
            .or_default() += 1;
    }

    fn counters(&self) -> Vec<(String, u64)> {
        let name = self.pass.name();
        let mut out = vec![
            (
                format!("transparent_sort_{name}_calls"),
                self.calls.load(Ordering::Relaxed),
            ),
            (format!("transparent_sort_{name}_cut"), self.cut.load(Ordering::Relaxed)),
            (
                format!("transparent_sort_{name}_checked"),
                self.checked.load(Ordering::Relaxed),
            ),
        ];
        out.extend(
            self.kept
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .map(|(c, n)| (format!("scene_kept_transparent_sort_{name}_{c}"), *n)),
        );
        out
    }
}

/// A checked call: Rust's order, compared at `TRANSPARENT_ORDER_READY`.
struct Pending {
    pass: Pass,
    order: Vec<u16>,
}

/// The arm's state: its three passes, the call being checked, and whether
/// a difference has sent the session to the guest sort.
pub struct Sort {
    world: Arm,
    first_person: Arm,
    other: Arm,
    pending: Mutex<Option<Pending>>,
    fallback: AtomicBool,
}

impl Default for Sort {
    fn default() -> Self {
        Self::new()
    }
}

impl Sort {
    pub const fn new() -> Self {
        Self {
            world: Arm::new(Pass::World),
            first_person: Arm::new(Pass::FirstPerson),
            other: Arm::new(Pass::Other),
            pending: Mutex::new(None),
            fallback: AtomicBool::new(false),
        }
    }

    fn arm(&self, pass: Pass) -> &Arm {
        match pass {
            Pass::World => &self.world,
            Pass::FirstPerson => &self.first_person,
            Pass::Other => &self.other,
        }
    }

    /// At `TRANSPARENT_ORDER_BEGIN`, with the order Rust made for the
    /// live rows.
    pub fn begin(&self, pass: Pass, order: Vec<u16>) -> Decision {
        let arm = self.arm(pass);
        let call = arm.calls.fetch_add(1, Ordering::Relaxed);
        if pass == Pass::Other {
            arm.keep("caller");
            return Decision::Guest;
        }
        if self.fallback.load(Ordering::Relaxed) {
            arm.keep("fallback");
            return Decision::Guest;
        }
        if call % CHECK_EVERY == 0 {
            *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(Pending { pass, order });
            return Decision::Guest;
        }
        arm.cut.fetch_add(1, Ordering::Relaxed);
        Decision::Cut
    }

    /// Whether a checked call waits for the guest's order.
    pub fn pending(&self) -> bool {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// At `TRANSPARENT_ORDER_READY`, with the order the guest's sort left
    /// (`None`: it did not read). A difference falls back for the session.
    pub fn ready(&self, guest: Option<Vec<u16>>) {
        let Some(Pending { pass, order }) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).take() else {
            return;
        };
        let arm = self.arm(pass);
        let Some(guest) = guest else {
            arm.keep("unread");
            return;
        };
        arm.checked.fetch_add(1, Ordering::Relaxed);
        if guest == order {
            return;
        }
        arm.keep("mismatch");
        if !self.fallback.swap(true, Ordering::Relaxed) {
            let at = order
                .iter()
                .zip(&guest)
                .position(|(a, b)| a != b)
                .unwrap_or(order.len().min(guest.len()));
            eprintln!(
                "native-transparents: the {} pass's sort differs from the guest's at ordinal {at} ({} rows, guest {}); the guest sort runs from here",
                pass.name(),
                order.len(),
                guest.len(),
            );
        }
    }

    pub fn fallen_back(&self) -> bool {
        self.fallback.load(Ordering::Relaxed)
    }

    pub fn counters(&self) -> Vec<(String, u64)> {
        [&self.world, &self.first_person, &self.other]
            .into_iter()
            .flat_map(Arm::counters)
            .collect()
    }
}

/// The game's arm.
pub static SORT: Sort = Sort::new();

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    SORT.counters()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    fn count(sort: &Sort, name: &str) -> u64 {
        sort.counters()
            .into_iter()
            .find(|(k, _)| k == name)
            .map_or(0, |(_, n)| n)
    }

    #[test]
    fn the_first_person_sorted_pass_draws_in_slot_10() {
        let esp = 0x1_0000;
        let mut m = FlatMemory::new(esp, 0x40);
        m.set_u32(esp + RETURN_OFFSET, FIRST_PERSON_RETURN).unwrap();
        let pass = Pass::at(&m, esp);
        assert_eq!(pass, Pass::FirstPerson);
        assert_eq!(pass.slot().and_then(ViewSlot::number), Some(10));
        m.set_u32(esp + RETURN_OFFSET, WORLD_RETURN).unwrap();
        assert_eq!(Pass::at(&m, esp).slot().and_then(ViewSlot::number), Some(9));
        m.set_u32(esp + RETURN_OFFSET, 0x0004_1234).unwrap();
        assert_eq!(Pass::at(&m, esp).slot(), None);
    }

    #[test]
    fn one_call_in_64_runs_the_guest_sort_and_is_compared() {
        let sort = Sort::new();
        let order = vec![3, 1, 2];
        assert_eq!(sort.begin(Pass::World, order.clone()), Decision::Guest);
        assert!(sort.pending());
        sort.ready(Some(order.clone()));
        assert!(!sort.pending());
        for _ in 1..CHECK_EVERY {
            assert_eq!(sort.begin(Pass::World, order.clone()), Decision::Cut);
            sort.ready(Some(vec![9, 9, 9]));
        }
        assert_eq!(sort.begin(Pass::World, order.clone()), Decision::Guest);
        // The first person's pass counts its own calls.
        assert_eq!(sort.begin(Pass::FirstPerson, order.clone()), Decision::Guest);
        assert_eq!(count(&sort, "transparent_sort_world_checked"), 1);
        assert_eq!(count(&sort, "transparent_sort_world_cut"), CHECK_EVERY - 1);
        assert!(!sort.fallen_back());
    }

    #[test]
    fn a_different_order_falls_back_for_the_session() {
        let sort = Sort::new();
        assert_eq!(sort.begin(Pass::FirstPerson, vec![0, 1]), Decision::Guest);
        sort.ready(Some(vec![1, 0]));
        assert!(sort.fallen_back());
        assert_eq!(count(&sort, "scene_kept_transparent_sort_first_person_mismatch"), 1);
        assert_eq!(sort.begin(Pass::World, vec![0, 1]), Decision::Guest);
        assert_eq!(sort.begin(Pass::World, vec![0, 1]), Decision::Guest);
        assert!(!sort.pending());
        assert_eq!(count(&sort, "scene_kept_transparent_sort_world_fallback"), 2);
    }

    #[test]
    fn an_unnamed_caller_and_an_unread_order_are_kept_by_name() {
        let sort = Sort::new();
        assert_eq!(sort.begin(Pass::Other, vec![0]), Decision::Guest);
        assert!(!sort.pending());
        assert_eq!(count(&sort, "scene_kept_transparent_sort_other_caller"), 1);
        assert_eq!(sort.begin(Pass::World, vec![0]), Decision::Guest);
        sort.ready(None);
        assert_eq!(count(&sort, "scene_kept_transparent_sort_world_unread"), 1);
        assert_eq!(count(&sort, "transparent_sort_world_checked"), 0);
        assert!(!sort.fallen_back());
    }
}
