//! The display copy's water parts from the sorted display queue
//! (`frame_scene::transparent::DisplayTransparentQueue::runs`), drawn
//! from inside the guest's sorted dispatch (`0x0001e370`) instead of the
//! layer-6 call's lone part (docs/graphics/native-renderer.md, "Scene
//! list builders", and docs/graphics/retire-d3d.md, "R2's first slice").
//!
//! The queue is built at `TRANSPARENT_ORDER_READY` (after the sort, before
//! the loop), which is where [`begin`] runs and draws the head run
//! (`after: None`). The loop then calls each entry's handler, or skips a
//! null one, and falls to `TRANSPARENT_DISPATCH_NEXT` either way: [`after_entry`]
//! draws the run that sorts after that ordinal. [`end`] drops what is left
//! at the loop's exit (`SCENE_TRANSPARENTS_END`).
//!
//! Only the display copy is drawn to ([`Host::draw_display_parts`]); the
//! guest's handlers still draw everything the reference view shows. A queue
//! the plan refuses (`runs` returns `None`) draws nothing here, so the
//! layer-6 path is the only display water draw, as before.

use std::cell::RefCell;

use guest_layout::memory::GuestMemory;
use frame_scene::transparent::{DisplayTransparentPart, DisplayTransparentQueue};

use super::Host;

/// A display part: its cluster and BSP part index.
pub type PartKey = (u16, u8);

/// The parts of the queue the display pass draws, grouped by the
/// reference ordinal they sort after, in dispatch order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    runs: Vec<(Option<u16>, Vec<PartKey>)>,
    /// The loop's `esp` at the build, which every return of its handlers
    /// restores: a hook at another frame is not this dispatch's.
    esp: u32,
}

impl Plan {
    /// The plan for `queue` over the parts `keep` selects (the host's
    /// water set, less what the layer-6 pass drew), or `None` when the queue
    /// is not drawable. An empty plan is `Some`: nothing to draw, but the
    /// queue was accepted.
    pub fn build(
        queue: &DisplayTransparentQueue,
        esp: u32,
        keep: impl Fn(&DisplayTransparentPart) -> bool,
    ) -> Option<Plan> {
        let runs = queue.runs(keep)?;
        Some(Plan {
            runs: runs
                .iter()
                .map(|run| (run.after, run.parts.iter().map(|p| (p.cluster, p.part)).collect()))
                .collect(),
            esp,
        })
    }

    /// The runs that sort at or before `after`, each once, in queue order.
    /// The loop meets every ordinal, so this is the one run at `after`.
    fn take(&mut self, after: Option<u16>) -> Vec<Vec<PartKey>> {
        let due = self.runs.iter().take_while(|(a, _)| *a <= after).count();
        self.runs.drain(..due).map(|(_, parts)| parts).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

thread_local! {
    /// The view's plan while its sorted dispatch runs (the guest thread).
    static PLAN: RefCell<Option<Plan>> = const { RefCell::new(None) };
}

/// `render.stats`: `layer_plan_15_display_queue_<outcome>`.
fn count(outcome: &str, n: u64) {
    crate::native_layers::count(15, &format!("display_queue_{outcome}"), n);
}

/// Draws one run on the host; counts it.
fn draw(mem: &mut dyn GuestMemory, host: &dyn Host, parts: &[PartKey]) {
    let drawn = host.draw_display_parts(mem, parts);
    if drawn == 0 {
        count("runs_refused", 1);
        return;
    }
    count("runs_drawn", 1);
    count("parts_drawn", drawn as u64);
}

/// At the sorted order's end, before the first entry: plans `queue` and
/// draws the head run. `esp` is the dispatch's stack pointer.
/// `already` are the parts the layer-6 pass drew on the display copy.
pub fn begin(mem: &mut dyn GuestMemory, host: &dyn Host, queue: &DisplayTransparentQueue, esp: u32, already: &[PartKey]) {
    PLAN.with(|p| p.borrow_mut().take());
    let plan = {
        let ro: &dyn GuestMemory = &*mem;
        Plan::build(queue, esp, |part| {
            !already.contains(&(part.cluster, part.part)) && host.draws_display_part(ro, part)
        })
    };
    let Some(plan) = plan else {
        // The guest orders what the plan cannot: the layer-6 draw stands.
        count("old_path", 1);
        return;
    };
    count("runs_planned", plan.runs.len() as u64);
    if plan.is_empty() {
        return;
    }
    PLAN.with(|p| *p.borrow_mut() = Some(plan));
    after(mem, host, esp, None);
}

/// After the dispatch loop's entry `ordinal` has returned (or was
/// skipped for its null handler): draws the run that sorts after it.
pub fn after_entry(mem: &mut dyn GuestMemory, host: &dyn Host, esp: u32, ordinal: u32) {
    let Ok(ordinal) = u16::try_from(ordinal) else { return };
    after(mem, host, esp, Some(ordinal));
}

fn after(mem: &mut dyn GuestMemory, host: &dyn Host, esp: u32, ordinal: Option<u16>) {
    let due = PLAN.with(|p| {
        let mut p = p.borrow_mut();
        let plan = p.as_mut()?;
        if plan.esp != esp {
            count("hook_foreign_frame", 1);
            return None;
        }
        let due = plan.take(ordinal);
        if plan.is_empty() {
            *p = None;
        }
        Some(due)
    });
    for parts in due.into_iter().flatten() {
        draw(mem, host, &parts);
    }
}

/// The loop's exit: what was not reached is dropped.
pub fn end() {
    if let Some(plan) = PLAN.with(|p| p.borrow_mut().take()) {
        count("runs_undrawn", plan.runs.len() as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use frame_scene::transparent::DisplayTransparentSlot::{Geometry, Reference};
    use std::sync::Mutex;

    fn part(cluster: u16, part: u8, shader: u32) -> DisplayTransparentPart {
        DisplayTransparentPart {
            cluster,
            part,
            shader,
            lod: 0,
            first_index: 0,
            index_count: 3,
            triangle_list: true,
            first_subpart: 0,
            subpart_count: 1,
            position: [0.; 3],
            depth: 1.,
            class: 1,
            plane: None,
        }
    }

    fn queue(slots: Vec<frame_scene::transparent::DisplayTransparentSlot>) -> DisplayTransparentQueue {
        DisplayTransparentQueue { slots, unresolved_clusters: Vec::new(), order_exact: true }
    }

    /// Draws everything of shader 50 and up; records each call.
    #[derive(Default)]
    struct Recorder {
        drawn: Mutex<Vec<Vec<PartKey>>>,
        refuse: bool,
    }

    impl Host for Recorder {
        fn ask(&self, _: &mut dyn GuestMemory, _: &[u16], _: &mut crate::native_layers::Left) -> Option<native_render::skip::Cleared> {
            None
        }
        fn ask_run(
            &self, _: &mut dyn GuestMemory, _: &crate::late_sorted::RunAt<'_>, left: &mut crate::native_layers::Left,
        ) -> crate::late_sorted::Run {
            left.all("device");
            crate::late_sorted::Run::Refused
        }
        fn draws_display_part(&self, _: &dyn GuestMemory, part: &DisplayTransparentPart) -> bool {
            part.shader >= 50
        }
        fn draw_display_parts(&self, _: &mut dyn GuestMemory, parts: &[PartKey]) -> usize {
            if self.refuse {
                return 0;
            }
            self.drawn.lock().unwrap().push(parts.to_vec());
            parts.len()
        }
    }

    const ESP: u32 = 0x0060_0000;

    fn memory() -> FlatMemory {
        FlatMemory::new(0x0040_0000, 0x1000)
    }

    #[test]
    fn runs_draw_in_order_between_the_entries_the_guest_dispatches() {
        // Ordinals 0..4; ordinal 1 has no handler (the loop skips the call
        // but still reaches the site).
        let q = queue(vec![
            Geometry(part(1, 0, 50)),
            Reference(0),
            Geometry(part(2, 1, 51)),
            Geometry(part(2, 2, 52)),
            Reference(1),
            Reference(2),
            Geometry(part(3, 0, 10)), // not water: left to the guest
            Geometry(part(4, 0, 53)),
            Reference(3),
        ]);
        let host = Recorder::default();
        let mut mem = memory();
        let mut seen = Vec::new();
        begin(&mut mem, &host, &q, ESP, &[]);
        seen.push(host.drawn.lock().unwrap().len());
        for ordinal in 0..4 {
            after_entry(&mut mem, &host, ESP, ordinal);
            seen.push(host.drawn.lock().unwrap().len());
        }
        end();
        // The head run before the first entry; after 0 the pair; after 1
        // (no handler) nothing; after 2 the last water part; after 3 nothing.
        assert_eq!(seen, [1, 2, 2, 3, 3]);
        assert_eq!(
            *host.drawn.lock().unwrap(),
            [vec![(1, 0)], vec![(2, 1), (2, 2)], vec![(4, 0)]]
        );
        PLAN.with(|p| assert!(p.borrow().is_none()));
    }

    #[test]
    fn a_refused_queue_draws_nothing_and_leaves_the_old_path() {
        for mutate in [0, 1, 2] {
            let mut q = queue(vec![Reference(0), Geometry(part(1, 0, 50)), Reference(1)]);
            match mutate {
                0 => q.order_exact = false,
                1 => q.unresolved_clusters.push(9),
                // Ordinals the loop would not meet as `0, 1, ...`.
                _ => q.slots[2] = Reference(5),
            }
            let host = Recorder::default();
            let mut mem = memory();
            begin(&mut mem, &host, &q, ESP, &[]);
            for ordinal in 0..2 {
                after_entry(&mut mem, &host, ESP, ordinal);
            }
            end();
            assert!(host.drawn.lock().unwrap().is_empty(), "case {mutate}");
            PLAN.with(|p| assert!(p.borrow().is_none()));
        }
    }

    #[test]
    fn parts_the_layer_6_pass_drew_and_another_frame_are_not_drawn_again() {
        let q = queue(vec![Geometry(part(1, 0, 50)), Reference(0), Geometry(part(2, 0, 51))]);
        let host = Recorder::default();
        let mut mem = memory();
        begin(&mut mem, &host, &q, ESP, &[(1, 0)]);
        // A hook from another stack frame is not this dispatch's.
        after_entry(&mut mem, &host, ESP - 16, 0);
        assert!(host.drawn.lock().unwrap().is_empty());
        after_entry(&mut mem, &host, ESP, 0);
        assert_eq!(*host.drawn.lock().unwrap(), [vec![(2, 0)]]);
        end();
    }

    #[test]
    fn a_host_that_refuses_a_run_is_counted_and_the_loop_goes_on() {
        let q = queue(vec![Geometry(part(1, 0, 50)), Reference(0), Geometry(part(2, 0, 51))]);
        let host = Recorder { refuse: true, ..Default::default() };
        let mut mem = memory();
        begin(&mut mem, &host, &q, ESP, &[]);
        after_entry(&mut mem, &host, ESP, 0);
        end();
        assert!(host.drawn.lock().unwrap().is_empty());
        let counters = crate::native_layers::counters();
        let refused = counters
            .iter()
            .find(|(k, _)| k == "layer_plan_15_display_queue_runs_refused")
            .map_or(0, |(_, v)| *v);
        assert!(refused >= 2, "{counters:?}");
    }

    #[test]
    fn the_plan_takes_a_late_run_in_order() {
        let q = queue(vec![Geometry(part(1, 0, 50)), Reference(0), Geometry(part(2, 0, 51)), Reference(1)]);
        let mut plan = Plan::build(&q, ESP, |_| true).unwrap();
        assert_eq!(plan.take(Some(1)), [vec![(1, 0)], vec![(2, 0)]]);
        assert!(plan.is_empty());
    }
}
