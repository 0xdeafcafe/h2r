//! Layer 3's objects before the layer draw (docs/graphics/retire-d3d-remainders.md,
//! "R1's remainder"): the Rust build of the layer's list, sorted as the
//! guest sorts it, drawn with the layer's entry state, so that a layer 3
//! whose other items are all native returns without the guest body.
//!
//! At the scene's and the alternate view's layer 3 ([`at_layer`]):
//!
//! - **The list.** Each item still holding bit 3 (after the world's and
//!   the decorators' clears) is built as `0x0004cbf0` builds it
//!   (`native_world::scene_build`), the accepted slots sorted as
//!   `0x0013da70` sorts them (`scene_extract::layer_sort::geometry`), and
//!   read as the walk reads the guest's (`object_walk::read_slots`). An
//!   item of another build, or one the build does not make, refuses.
//! - **The entry state.** The host's fields ([`Host::entry_state`]
//!   `predict`): the device before the call with what the layer begin
//!   leaves at the first geometry handler (its `ZENABLE`, the cache
//!   `0x00016b10` clears). An open shader in the cache refuses: its
//!   restore is a D3D write.
//! - **The gate.** One frame in [`SAMPLE`] (`0x004ba034`) is compared: the
//!   guest's body runs and draws as before, and at the walk's first
//!   geometry handler ([`compare`]) the guest's entries are compared with
//!   the Rust list, and, where no entry drew before the handler, the
//!   host's fields with the prediction, each mismatch counted by field.
//!   After [`CLEAN`] clean sampled frames in a row the gate answers; at
//!   any difference it stops, until another clean run.
//! - **The draw.** While answering, on an unsampled frame whose list holds
//!   no entry of another kind (every structure item and decorator native),
//!   the leading runs of the list draw before the call
//!   ([`Host::draw_before`]) and their items' bit 3 is cleared until the
//!   next layer draw. The first object the runs do not take ends them:
//!   it and every object after it keep their guest handlers' draws (their
//!   own cause, else `order`), as the walk keeps every object after a
//!   guest draw. The walk does not run in that call's body.
//!
//! Counted in `render.scene` as `layer_plan_3_objects_before_<outcome>`.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Mutex;

use guest_layout::memory::GuestMemory;
use frame_scene::ViewKind;
use native_render::camera::GuestRead;
use native_render::object_walk::{self, Entry};
use timeline::native::ObjectWalk;

use super::{Host, keep, kept_site};
use crate::native_scene_lists::LodDetail;

/// One frame in this many is compared.
pub const SAMPLE: u32 = 64;
/// Clean sampled frames in a row before the gate answers.
pub const CLEAN: u32 = 16;
/// The render frame counter.
pub const FRAME: u32 = 0x004b_a034;
/// Sort slots a layer build makes at most (`0x0003bc30`'s frame).
const MAX_SLOTS: usize = 2560;
/// Layer 3.
const LAYER: u32 = native_render::skip::LIGHTMAP_LAYER;

/// One field of the entry state: its name and value (`None`: it does not read).
pub type Field = (String, Option<u32>);

/// `layer_plan_3_objects_before_<outcome>`.
pub fn count(outcome: &str, n: u64) {
    crate::native_layers::count(LAYER, &format!("objects_before_{outcome}"), n);
}

/// The Rust build of a layer 3's list: its entries in the guest's
/// dispatch order, and the list item each entry draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub entries: Vec<Entry>,
    pub items: Vec<u32>,
}

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// Builds `list`'s layer 3 as the guest's build and sort would: `Err`
/// names why it cannot (`list`, `count`, `foreign`, `build`, `entries`).
pub fn build(mem: &dyn GuestMemory, list: u32) -> Result<Built, &'static str> {
    use native_render::skip::{GEOMETRY_BUILD, ITEM_SIZE};
    let word = |at: u32| mem.u32(at).map_err(|_| "list");
    let end = word(list.wrapping_add(4))?;
    let first = word(list.wrapping_add(0xc))?;
    let table = word(list.wrapping_add(0x14))?;
    if list == 0 || first > end || end > u32::from(u16::MAX) {
        return Err("list");
    }
    let bit = 1u32 << LAYER;
    let mut slots: Vec<[u8; 24]> = Vec::new();
    for i in first..end {
        let item = table.wrapping_add(i * ITEM_SIZE);
        if word(item + 0x10)? & bit == 0 {
            continue;
        }
        if word(item)? != GEOMETRY_BUILD {
            return Err("foreign");
        }
        if slots.len() >= MAX_SLOTS {
            return Err("count");
        }
        let shader = word(item + 8)?;
        let packed = word(item + 0xc)?;
        let selector = mem.u8(item + 0x14).map_err(|_| "list")?;
        let lod = mem.u8(item + 0x15).map_err(|_| "list")?;
        let mut slot = [0u8; 24];
        slot[..2].copy_from_slice(&(i as u16).to_le_bytes());
        slot[2..4].copy_from_slice(&(shader as u16).to_le_bytes());
        let built = crate::native_world::scene_build::build(mem, shader, lod, LAYER, selector, packed, slot, true)
            .ok_or("build")?;
        if built.accepted {
            slots.push(built.bytes);
        }
    }
    scene_extract::layer_sort::geometry(&mut slots, LAYER);
    let items = slots
        .iter()
        .map(|s| table.wrapping_add(u32::from(u16::from_le_bytes([s[0], s[1]])) * ITEM_SIZE))
        .collect();
    let (entries, _) = object_walk::read_slots(&Ro(mem), list, &slots).ok_or("entries")?;
    Ok(Built { entries, items })
}

/// The gate: the frame being compared, its verdict so far, and the run of
/// clean sampled frames.
#[derive(Debug, Default)]
pub struct Gate {
    frame: Option<u32>,
    sampled: bool,
    clean: bool,
    compared: u32,
    streak: u32,
    answering: bool,
}

impl Gate {
    /// The frame a layer 3 is in; a new frame ends the last one's
    /// verdict. Whether this frame is compared.
    pub fn frame(&mut self, frame: u32) -> bool {
        if self.frame != Some(frame) {
            self.finish();
            self.frame = Some(frame);
            self.sampled = frame % SAMPLE == 0;
            self.clean = true;
            self.compared = 0;
        }
        self.sampled
    }

    fn finish(&mut self) {
        if self.frame.is_none() || !self.sampled {
            return;
        }
        if !self.clean {
            count("frames_unclean", 1);
            return;
        }
        if self.compared == 0 {
            count("frames_empty", 1);
            return;
        }
        count("frames_clean", 1);
        self.streak += 1;
        if self.streak >= CLEAN && !self.answering {
            self.answering = true;
            count("opened", 1);
        }
    }

    /// One comparison: the fields that differed (none: it matched).
    pub fn compared(&mut self, differed: &[String]) {
        self.compared += 1;
        if differed.is_empty() {
            count("matched", 1);
            return;
        }
        for field in differed {
            count(&format!("mismatched_{field}"), 1);
        }
        self.clean = false;
        self.streak = 0;
        if self.answering {
            self.answering = false;
            count("fallback", 1);
        }
    }

    pub fn answering(&self) -> bool {
        self.answering
    }
}

static GATE: Mutex<Option<Gate>> = Mutex::new(None);

fn with_gate<R>(f: impl FnOnce(&mut Gate) -> R) -> R {
    f(GATE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Gate::default))
}

thread_local! {
    /// A sampled call's Rust list and prediction, for its walk's first handler.
    static PENDING: RefCell<Option<(Built, Vec<Field>)>> = const { RefCell::new(None) };
    /// The items whose bit 3 a draw before the call cleared, each with the
    /// mask it left, until the next layer draw.
    static CLEARED: RefCell<Vec<(u32, u32)>> = const { RefCell::new(Vec::new()) };
}

/// Sets back the bits the last draw before a layer 3 cleared, on the items
/// that still hold the mask it left.
pub fn restore_pending(mem: &mut dyn GuestMemory) {
    let cleared = CLEARED.with(|c| std::mem::take(&mut *c.borrow_mut()));
    for (item, left) in cleared {
        if mem.u32(item + 0x10) == Ok(left) {
            let _ = mem.set_u32(item + 0x10, left | 1 << LAYER);
        }
    }
}

/// Drops what a load leaves behind: the pending bits, a sampled call's
/// list, the gate.
pub fn forget() {
    CLEARED.with(|c| c.borrow_mut().clear());
    PENDING.with(|p| p.borrow_mut().take());
    *GATE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The layer's end: a sampled call's list goes with it.
pub fn end_layer() {
    PENDING.with(|p| p.borrow_mut().take());
}

/// The scene's or the alternate view's layer 3 (`kind`) over `list`, before
/// the call. True when its objects were drawn here, or kept by cause: the
/// walk then does not run in the body.
pub fn at_layer(mem: &mut dyn GuestMemory, host: &dyn Host, kind: ViewKind, list: u32, display_lod: Option<LodDetail>) -> bool {
    let frame = mem.u32(FRAME).unwrap_or(0);
    let (sampled, answering) = with_gate(|g| (g.frame(frame), g.answering()));
    if !sampled && !answering {
        return false;
    }
    use crate::frame_step::layer::{SETUP, SETUP_OPEN};
    if mem.u32(SETUP + SETUP_OPEN) != Ok(0) {
        count("refused_open_shader", 1);
        return false;
    }
    let built = match build(&*mem, list) {
        Ok(b) => b,
        Err(why) => {
            count(&format!("refused_{why}"), 1);
            return false;
        }
    };
    if !built.entries.iter().any(|e| matches!(e, Entry::Object { .. })) {
        return false;
    }
    let Some(state) = host.entry_state(&*mem, true) else {
        count("refused_state", 1);
        return false;
    };
    if sampled {
        PENDING.with(|p| *p.borrow_mut() = Some((built, state)));
        return false;
    }
    if built.entries.iter().any(|e| matches!(e, Entry::Other)) {
        // A kept structure item or another build's: the walk keeps the order.
        count("kept_entries", 1);
        return false;
    }
    if !crate::scene_step::objects(&*mem, kind, &built.entries) {
        count("refused_scene", 1);
        return false;
    }
    draw(mem, host, kind, &built, display_lod);
    count("drawn", 1);
    true
}

/// The leading runs of `built` drawn before the call, the rest kept.
fn draw(mem: &mut dyn GuestMemory, host: &dyn Host, kind: ViewKind, built: &Built, display_lod: Option<LodDetail>) {
    let site = kept_site(kind);
    let decision = object_walk::decide(&built.entries, 0);
    let mut at = 0;
    for run in &decision.runs {
        let leading = built.entries[at..run.slots.start].iter().all(|e| matches!(e, Entry::Excluded));
        if !leading {
            break;
        }
        let items = &built.items[run.slots.clone()];
        let mut consume = |mem: &mut dyn GuestMemory| clear(mem, items);
        let result = crate::scene_step::with_open(|view| {
            host.draw_before(&mut *mem, view, super::SLOT, run.slots.start, &built.entries[run.slots.clone()], display_lod, &mut consume)
        });
        match result {
            Ok(()) => {
                ObjectWalk::Native.count(run.objects);
                at = run.slots.end;
            }
            Err(reason) => {
                keep(site, reason, run.objects);
                at = run.slots.end;
                break;
            }
        }
    }
    // Every object from the first the runs did not take keeps its draw.
    let rest = &built.entries[at..];
    let unsupported: HashSet<u32> = rest
        .iter()
        .filter_map(|e| match e {
            Entry::Object { handle, part: None } => Some(*handle),
            _ => None,
        })
        .collect();
    let mut seen = HashSet::new();
    let (mut bad, mut order) = (0, 0);
    for e in rest {
        if let Entry::Object { handle, .. } = e
            && seen.insert(*handle)
        {
            if unsupported.contains(handle) { bad += 1 } else { order += 1 }
        }
    }
    keep(site, ObjectWalk::Unsupported, bad);
    keep(site, ObjectWalk::Order, order);
}

/// Clears bit 3 on `items`, each remembered with the mask it leaves.
/// False when one does not read or no longer holds the bit.
fn clear(mem: &mut dyn GuestMemory, items: &[u32]) -> bool {
    let bit = 1u32 << LAYER;
    let masks: Option<Vec<u32>> = items.iter().map(|&i| mem.u32(i + 0x10).ok().filter(|m| m & bit != 0)).collect();
    let Some(masks) = masks else { return false };
    for (&item, mask) in items.iter().zip(masks) {
        if mem.set_u32(item + 0x10, mask & !bit).is_err() {
            return false;
        }
        CLEARED.with(|c| c.borrow_mut().push((item, mask & !bit)));
    }
    true
}

/// The fields of `predicted` that `actual` holds otherwise, by name (a
/// field `actual` lacks is `missing_<name>`).
pub fn differing(predicted: &[Field], actual: &[Field]) -> Vec<String> {
    let mut out = Vec::new();
    for (name, value) in predicted {
        match actual.iter().find(|(n, _)| n == name) {
            Some((_, v)) if v == value => {}
            Some(_) => out.push(name.clone()),
            None => out.push(format!("missing_{name}")),
        }
    }
    out
}

/// At a sampled call's first geometry handler: the guest's `entries`
/// (the walk is at `current`) against the Rust list, and, where every
/// entry before `current` is excluded (none drew), the host's fields
/// against the prediction.
pub fn compare(mem: &dyn GuestMemory, host: &dyn Host, entries: &[Entry], current: usize) {
    let Some((built, predicted)) = PENDING.with(|p| p.borrow_mut().take()) else { return };
    let mut differed = Vec::new();
    if built.entries != entries {
        differed.push(if built.entries.len() == entries.len() { "entries".to_owned() } else { "entry_count".to_owned() });
    }
    if entries.get(..current).is_some_and(|before| before.iter().all(|e| matches!(e, Entry::Excluded))) {
        match host.entry_state(mem, false) {
            Some(actual) => differed.extend(differing(&predicted, &actual)),
            None => differed.push("state".to_owned()),
        }
    } else {
        count("state_not_first", 1);
    }
    with_gate(|g| g.compared(&differed));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, v: Option<u32>) -> Field {
        (name.to_owned(), v)
    }

    /// The gate answers after [`CLEAN`] clean sampled frames in a row,
    /// stops at the first difference, and a sampled frame with nothing
    /// compared neither counts nor breaks the run.
    #[test]
    fn the_gate_answers_after_clean_samples_and_stops_at_a_difference() {
        let mut g = Gate::default();
        for k in 0..CLEAN {
            assert!(g.frame(k * SAMPLE));
            g.compared(&[]);
            assert!(!g.frame(k * SAMPLE + 1), "unsampled");
        }
        assert!(g.answering(), "answers after the clean run");
        assert!(g.frame(CLEAN * SAMPLE));
        g.frame(CLEAN * SAMPLE + 1);
        assert!(g.answering(), "an empty sample breaks nothing");
        g.frame((CLEAN + 1) * SAMPLE);
        g.compared(&["CullMode".to_owned()]);
        assert!(!g.answering(), "the guest's at a difference");
        for k in 0..CLEAN - 1 {
            g.frame((CLEAN + 2 + k) * SAMPLE);
            g.compared(&[]);
        }
        g.frame(10_000 * SAMPLE + 1);
        assert!(!g.answering(), "a new clean run is needed");
    }

    /// Each predicted field the guest holds otherwise is named, and one
    /// it lacks is `missing_<name>`.
    #[test]
    fn a_prediction_differs_by_field() {
        let predicted = [field("ZEnable", Some(2)), field("CullMode", Some(0x900)), field("fog_0", None), field("stipple", Some(0))];
        let actual = [field("ZEnable", Some(2)), field("CullMode", Some(0x901)), field("fog_0", None)];
        assert_eq!(differing(&predicted, &actual), ["CullMode", "missing_stipple"]);
        assert!(differing(&actual, &actual).is_empty());
    }

    /// Clearing a run's items takes bit 3 off each and remembers the mask;
    /// the next layer draw sets it back only on an item that still holds it.
    #[test]
    fn a_runs_cleared_bits_go_back_at_the_next_layer_draw() {
        use guest_layout::memory::FlatMemory;
        let mut m = FlatMemory::new(0x0060_0000, 0x100);
        let (a, b) = (0x0060_0000, 0x0060_0018);
        m.set_u32(a + 0x10, 1 << 3 | 1 << 5).unwrap();
        m.set_u32(b + 0x10, 1 << 3).unwrap();
        assert!(clear(&mut m, &[a, b]));
        assert_eq!((m.u32(a + 0x10).unwrap(), m.u32(b + 0x10).unwrap()), (1 << 5, 0));
        m.set_u32(b + 0x10, 1 << 7).unwrap();
        restore_pending(&mut m);
        assert_eq!(m.u32(a + 0x10).unwrap(), 1 << 3 | 1 << 5, "the same item's bit back");
        assert_eq!(m.u32(b + 0x10).unwrap(), 1 << 7, "a rebuilt item left alone");
        m.set_u32(a + 0x10, 0).unwrap();
        assert!(!clear(&mut m, &[a]), "an item without the bit refuses");
    }

    fn counter(name: &str) -> u64 {
        crate::native_layers::counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, v)| v)
    }

    fn part() -> object_walk::Part {
        object_walk::Part {
            key: frame_scene::ItemKey { record: 1, shader: 0xe500_0012, section: 0, part: 0, implementation: 0 },
            mode: 0xe100_0001,
            program: 0x4000,
            vertex_shader: 0xe100_003e,
            geometry_class: 0,
            node_influences: 0,
            triangle_list: true,
            first_index: 0,
            index_count: 3,
            flags: frame_scene::PartFlags::NONE,
            subparts: None,
        }
    }

    /// A host whose fields are `fields` and that draws every run asked of
    /// it, recording each run's first slot.
    struct Fields {
        fields: Vec<Field>,
        drawn: Mutex<Vec<usize>>,
    }

    impl Host for Fields {
        fn entry_state(&self, _: &dyn GuestMemory, _: bool) -> Option<Vec<Field>> {
            Some(self.fields.clone())
        }

        fn draw_before(
            &self, mem: &mut dyn GuestMemory, _: Option<(&frame_scene::FrameScene, &frame_scene::View)>, _: frame_scene::ViewSlot,
            first: usize, _: &[Entry], _: Option<LodDetail>, consume: &mut dyn FnMut(&mut dyn GuestMemory) -> bool,
        ) -> Result<(), ObjectWalk> {
            assert!(consume(mem));
            self.drawn.lock().unwrap().push(first);
            Ok(())
        }
    }

    /// At a sampled call's first geometry handler the guest's entries are
    /// compared with the Rust list, and, where no entry drew before the
    /// handler, the device's fields with the prediction, each mismatch
    /// counted by its field; after a drawn entry the state is not compared.
    #[test]
    fn a_sampled_layer_3_is_compared_at_its_first_handler_by_field() {
        let entries = vec![Entry::Excluded, Entry::Object { handle: 1, part: Some(part()) }];
        let predicted = vec![field("ZEnable", Some(2)), field("CullMode", Some(0x900))];
        let host = Fields { fields: vec![field("ZEnable", Some(2)), field("CullMode", Some(0x901))], drawn: Mutex::new(Vec::new()) };
        let mem = guest_layout::memory::FlatMemory::new(0x0060_0000, 0x100);
        let pend = |e: &[Entry]| PENDING.with(|p| *p.borrow_mut() = Some((Built { entries: e.to_vec(), items: vec![0; e.len()] }, predicted.clone())));
        let names = ["layer_plan_3_objects_before_mismatched_CullMode", "layer_plan_3_objects_before_mismatched_entries", "layer_plan_3_objects_before_state_not_first"];
        let before: Vec<u64> = names.iter().map(|n| counter(n)).collect();
        pend(&entries);
        compare(&mem, &host, &entries, 1);
        let mut other = entries.clone();
        other[0] = Entry::Object { handle: 2, part: None };
        pend(&entries);
        compare(&mem, &host, &other, 1);
        let after: Vec<u64> = names.iter().map(|n| counter(n)).collect();
        assert_eq!(after.iter().zip(&before).map(|(a, b)| a - b).collect::<Vec<_>>(), [1, 1, 1]);
        assert!(PENDING.with(|p| p.borrow().is_none()), "each comparison takes its call's list");
    }

    /// Before the call the leading runs draw and their items' bit 3 is
    /// cleared; from the first object the runs do not take, every object
    /// keeps its guest draw: an unreadable one by its own cause, the rest
    /// as `order`.
    #[test]
    fn the_leading_runs_draw_before_the_call_and_the_rest_keep_their_order() {
        use guest_layout::memory::FlatMemory;
        let base = 0x0060_0000;
        let mut m = FlatMemory::new(base, 0x100);
        let items: Vec<u32> = (0..4).map(|i| base + 0x18 * i).collect();
        for &i in &items {
            m.set_u32(i + 0x10, 1 << 3).unwrap();
        }
        let entries = vec![
            Entry::Object { handle: 1, part: Some(part()) },
            Entry::Excluded,
            Entry::Object { handle: 2, part: None },
            Entry::Object { handle: 3, part: Some(part()) },
        ];
        let host = Fields { fields: Vec::new(), drawn: Mutex::new(Vec::new()) };
        let names = ["scene_kept_alt3_object_unsupported", "scene_kept_alt3_object_order"];
        let before: Vec<u64> = names.iter().map(|n| counter(n)).collect();
        draw(&mut m, &host, ViewKind::Alternate, &Built { entries, items: items.clone() }, None);
        assert_eq!(*host.drawn.lock().unwrap(), [0], "only the leading run");
        assert_eq!(m.u32(items[0] + 0x10).unwrap(), 0, "its bit cleared");
        assert!(items[2..].iter().all(|&i| m.u32(i + 0x10).unwrap() == 1 << 3), "the rest keep theirs");
        let after: Vec<u64> = names.iter().map(|n| counter(n)).collect();
        assert_eq!(after.iter().zip(&before).map(|(a, b)| a - b).collect::<Vec<_>>(), [1, 1]);
        CLEARED.with(|c| c.borrow_mut().clear());
    }

    /// The Rust list refuses an item of another build and builds nothing
    /// from a list with no item holding bit 3.
    #[test]
    fn the_rust_list_refuses_another_build() {
        use guest_layout::memory::FlatMemory;
        let (list, items) = (0x0060_0000, 0x0060_0100);
        let mut m = FlatMemory::new(list, 0x200);
        m.set_u32(list + 4, 1).unwrap();
        m.set_u32(list + 0x14, items).unwrap();
        m.set_u32(items, 0x0004_e000).unwrap();
        assert!(matches!(build(&m, list), Err("entries") | Ok(_)), "no held item builds nothing");
        m.set_u32(items + 0x10, 1 << 3).unwrap();
        assert_eq!(build(&m, list), Err("foreign"));
    }
}
