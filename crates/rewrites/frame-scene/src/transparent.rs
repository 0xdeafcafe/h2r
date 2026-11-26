//! Sorted layer-15 work for the display copy of a game view.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DisplayTransparentQueue {
    /// Shared slots and display geometry in transparent comparator order.
    pub slots: Vec<DisplayTransparentSlot>,
    /// Resident section or material data needed to enumerate these clusters.
    pub unresolved_clusters: Vec<u16>,
    /// Equal keys or a plane cycle can depend on the wide gather's insertion order.
    pub order_exact: bool,
}

impl DisplayTransparentQueue {
    /// Whether the queue's order can be drawn: the merged order does not
    /// depend on the wide gather's insertion order and every cluster
    /// resolved. A queue that is not drawable stays with the guest.
    pub fn drawable(&self) -> bool {
        self.order_exact && self.unresolved_clusters.is_empty()
    }

    /// Display geometry in queue order, as the guest's loop would meet it.
    pub fn geometry(&self) -> impl Iterator<Item = &DisplayTransparentPart> {
        self.slots.iter().filter_map(|slot| match slot {
            DisplayTransparentSlot::Geometry(part) => Some(part),
            DisplayTransparentSlot::Reference(_) => None,
        })
    }

    /// The display geometry `keep` selects, grouped by the reference
    /// ordinal it follows in the sorted dispatch (`after: None` is before
    /// the first). A run is drawn on the display copy once the guest's
    /// handler for `after` has returned, which is where the comparator
    /// puts it. `None` when the queue is not [`drawable`](Self::drawable)
    /// or its reference ordinals are not `0, 1, 2, ...` in order (the
    /// guest's loop would not meet them as the plan assumes).
    pub fn runs(&self, keep: impl Fn(&DisplayTransparentPart) -> bool) -> Option<Vec<DisplayRun<'_>>> {
        if !self.drawable() {
            return None;
        }
        let mut runs: Vec<DisplayRun<'_>> = Vec::new();
        let mut next = 0u16;
        let mut after = None;
        for slot in &self.slots {
            match slot {
                DisplayTransparentSlot::Reference(ordinal) => {
                    if *ordinal != next {
                        return None;
                    }
                    next = next.checked_add(1)?;
                    after = Some(*ordinal);
                }
                DisplayTransparentSlot::Geometry(part) if keep(part) => {
                    match runs.last_mut() {
                        Some(run) if run.after == after => run.parts.push(part),
                        _ => runs.push(DisplayRun { after, parts: vec![part] }),
                    }
                }
                DisplayTransparentSlot::Geometry(_) => {}
            }
        }
        Some(runs)
    }
}

/// Display geometry that sorts between two reference ordinals.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayRun<'a> {
    /// The reference ordinal the run follows; `None` for the head.
    pub after: Option<u16>,
    /// The parts, in queue order.
    pub parts: Vec<&'a DisplayTransparentPart>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum DisplayTransparentSlot {
    /// Ordinal in this view's sorted reference dispatch, including null handlers.
    Reference(u16),
    /// A cluster surface which has no reference render record or handler.
    Geometry(DisplayTransparentPart),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DisplayTransparentPart {
    pub cluster: u16,
    pub part: u8,
    pub shader: u32,
    /// LOD selected by the scene material walk for its layer mask.
    pub lod: u8,
    pub first_index: u16,
    pub index_count: u16,
    pub triangle_list: bool,
    /// Visibility of these subparts must be resolved for the display lens
    /// before submitting the index range.
    pub first_subpart: u16,
    pub subpart_count: u16,
    pub position: [f32; 3],
    pub depth: f32,
    pub class: i32,
    pub plane: Option<([f32; 3], f32)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(cluster: u16, shader: u32) -> DisplayTransparentPart {
        DisplayTransparentPart {
            cluster,
            part: 0,
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

    fn queue(slots: Vec<DisplayTransparentSlot>) -> DisplayTransparentQueue {
        DisplayTransparentQueue { slots, unresolved_clusters: Vec::new(), order_exact: true }
    }

    #[test]
    fn display_geometry_runs_follow_the_reference_ordinal_they_sort_after() {
        use DisplayTransparentSlot::{Geometry, Reference};
        let q = queue(vec![
            Geometry(part(1, 10)),
            Reference(0),
            Geometry(part(2, 11)),
            Geometry(part(3, 12)),
            Reference(1),
            Reference(2),
            Geometry(part(4, 13)),
        ]);
        let runs = q.runs(|_| true).unwrap();
        let shape: Vec<_> = runs
            .iter()
            .map(|r| (r.after, r.parts.iter().map(|p| p.cluster).collect::<Vec<_>>()))
            .collect();
        assert_eq!(shape, [(None, vec![1]), (Some(0), vec![2, 3]), (Some(2), vec![4])]);
        // A selection keeps queue order and drops empty runs.
        let runs = q.runs(|p| p.shader == 12 || p.shader == 13).unwrap();
        let shape: Vec<_> = runs.iter().map(|r| (r.after, r.parts.len())).collect();
        assert_eq!(shape, [(Some(0), 1), (Some(2), 1)]);
        assert_eq!(q.geometry().map(|p| p.cluster).collect::<Vec<_>>(), [1, 2, 3, 4]);
    }

    #[test]
    fn a_queue_the_guest_must_order_has_no_runs() {
        use DisplayTransparentSlot::{Geometry, Reference};
        let mut q = queue(vec![Reference(0), Geometry(part(1, 10))]);
        assert!(q.drawable() && q.runs(|_| true).is_some());
        q.order_exact = false;
        assert!(!q.drawable() && q.runs(|_| true).is_none());
        q.order_exact = true;
        q.unresolved_clusters.push(7);
        assert!(!q.drawable() && q.runs(|_| true).is_none());
        // Reference ordinals out of order or with a gap are not the loop's.
        let q = queue(vec![Reference(1), Reference(0), Geometry(part(1, 10))]);
        assert!(q.runs(|_| true).is_none());
        let q = queue(vec![Reference(0), Reference(2)]);
        assert!(q.runs(|_| true).is_none());
        // No geometry: no runs, and still drawable.
        let q = queue(vec![Reference(0), Reference(1)]);
        assert_eq!(q.runs(|_| true), Some(Vec::new()));
    }
}
