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
