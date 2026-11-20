//! Pathfinding-data codec tests on invented bytes.
use crate::geometry::TagView;
use crate::pathfinding::*;

const BASE: u32 = 0x8000_0000;

/// An sbsp payload: a 0x240-byte header whose +0xc4 block holds one
/// pathfinding element, followed by its child tables.
struct Payload {
    bytes: Vec<u8>,
}
impl Payload {
    fn new() -> Self {
        Self {
            bytes: vec![0; 0x240 + PathfindingData::SIZE],
        }
    }
    fn put(&mut self, at: usize, v: u32) {
        self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    /// Append a table and point element field `field` at it.
    fn table(&mut self, field: usize, count: u32, data: &[u8]) {
        let at = self.bytes.len();
        self.bytes.extend_from_slice(data);
        self.put(0x240 + field, count);
        self.put(0x240 + field + 4, BASE + at as u32);
    }
    fn finish(mut self) -> Vec<u8> {
        self.put(0xc4, 1);
        self.put(0xc8, BASE + 0x240);
        self.bytes
    }
}
fn u16s(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn u32s(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Two triangles sharing the edge (1,2): sector 0 is (0,1,2), sector 1 is
/// (1,3,2). Surface 0 is split between them by a 2D BSP on x + y = 1.
/// Plus one orphan link.
fn sample() -> Vec<u8> {
    let mut p = Payload::new();
    // sectors: flags, hint, first link
    let mut sectors = u16s(&[0x19, 0xffff]);
    sectors.extend(u32s(&[0]));
    sectors.extend(u16s(&[0x19, 0xffff]));
    sectors.extend(u32s(&[3]));
    p.table(0x00, 2, &sectors);
    // links: v1 v2 flags hint fwd rev left right
    let links = u16s(&[
        0, 1, 1, 0xffff, 1, 1, 0, 0xffff, // 0: sector 0 boundary
        1, 2, 1, 0xffff, 2, 3, 0, 1, // 1: shared, left 0 / right 1
        2, 0, 1, 0xffff, 0, 2, 0, 0xffff, // 2: sector 0 boundary
        1, 3, 1, 0xffff, 4, 3, 1, 0xffff, // 3: sector 1 boundary
        3, 2, 1, 0xffff, 1, 4, 1, 0xffff, // 4: sector 1 boundary
        0, 3, 3, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, // 5: orphan
    ]);
    p.table(0x08, 6, &links);
    // refs: surface 0 -> node 0, surface 1 -> sector 1, surface 2 -> none
    p.table(0x10, 3, &u32s(&[0x8000_0000, 1, u32::MAX]));
    // node 0: x + y - 1 >= 0 -> sector 1, else sector 0
    let mut node = f32s(&[1.0, 1.0, 1.0]);
    node.extend(u32s(&[1, 0]));
    p.table(0x18, 1, &node);
    p.table(
        0x28,
        4,
        &f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]),
    );
    p.table(0x40, 2, &u32s(&[0xffff, 3]));
    p.put(0x240 + 0x48, 0xabcd_ef01);
    p.table(0x6c, 1, &[0; PathfindingData::USER_HINTS_SIZE]);
    p.finish()
}

fn decode(bytes: &[u8]) -> PathfindingData {
    PathfindingData::from_sbsp(&TagView::new(bytes, BASE))
        .unwrap()
        .unwrap()
}

#[test]
fn decodes_every_interpreted_table() {
    let pf = decode(&sample());
    assert_eq!(pf.sectors.len(), 2);
    assert_eq!(pf.sectors[1].first_link, 3);
    assert_eq!(pf.links.len(), 6);
    assert_eq!(pf.links[1].left_sector, 0);
    assert_eq!(pf.links[1].right_sector, 1);
    assert_eq!(pf.links[1].reverse_link, 3);
    assert_eq!(pf.refs[0].target(), RefTarget::Node(0));
    assert_eq!(pf.refs[1].target(), RefTarget::Sector(1));
    assert_eq!(pf.refs[2].target(), RefTarget::None);
    assert_eq!(pf.bsp2d_nodes[0].positive.target(), RefTarget::Sector(1));
    assert_eq!(pf.vertices[3], [1.0, 1.0, 0.0]);
    assert_eq!(pf.instance_refs, vec![0xffff, 3]);
    assert_eq!(pf.checksum, 0xabcd_ef01);
    assert_eq!(pf.user_hints.len(), 1);
    assert_eq!(pf.orphan_links(), 1);
}

#[test]
fn rings_follow_the_side_rule() {
    let pf = decode(&sample());
    assert_eq!(pf.sector_ring(0), Some(vec![0, 1, 2]));
    assert_eq!(pf.sector_ring(1), Some(vec![3, 4, 1]));
    assert_eq!(pf.sector_polygon(0), Some(vec![0, 1, 2]));
    // Sector 1 walks the shared link backwards: there it starts at vertex2.
    assert_eq!(pf.sector_polygon(1), Some(vec![1, 3, 2]));
    assert!(
        pf.validate(Some(3)).is_empty(),
        "{:?}",
        pf.validate(Some(3))
    );
}

#[test]
fn surface_lookup_walks_the_2d_bsp() {
    let pf = decode(&sample());
    assert_eq!(pf.sector_for_surface(0, 0.2, 0.2), Some(0));
    assert_eq!(pf.sector_for_surface(0, 0.8, 0.8), Some(1));
    assert_eq!(pf.sector_for_surface(1, 0.0, 0.0), Some(1));
    assert_eq!(pf.sector_for_surface(2, 0.5, 0.5), None);
    assert_eq!(pf.sector_for_surface(9, 0.5, 0.5), None);
}

#[test]
fn validation_reports_broken_indices() {
    let mut pf = decode(&sample());
    assert_eq!(pf.validate(Some(4)).len(), 1, "ref count against surfaces");
    pf.links[0].vertex2 = 99;
    pf.links[3].right_sector = 7;
    pf.refs[1] = Ref::sector(5);
    pf.bsp2d_nodes[0].negative = Ref::node(0);
    pf.sectors[1].first_link = 5;
    let v = pf.validate(Some(3));
    for want in [
        "link 0: vertex out of range",
        "link 3: sector out of range",
        "ref 1: target out of range",
        "2d node 0: child 0 does not precede it",
        "sector 1: link ring does not close",
    ] {
        assert!(v.iter().any(|m| m == want), "missing {want:?} in {v:?}");
    }
}

#[test]
fn a_cyclic_ring_or_2d_bsp_terminates() {
    let mut pf = decode(&sample());
    pf.bsp2d_nodes[0].negative = Ref::node(0);
    assert_eq!(pf.sector_for_surface(0, 0.0, 0.0), None);
    // 0 -> 1 -> 1 -> ... never returns to the first link.
    pf.links[1].forward_link = 1;
    assert_eq!(pf.sector_ring(0), None);
}

#[test]
fn unknown_blocks_are_refused_and_absent_data_is_none() {
    let mut b = sample();
    b[0x240 + 0x20] = 1;
    assert!(PathfindingData::from_sbsp(&TagView::new(&b, BASE)).is_err());
    let mut b = sample();
    b[0x240 + 0x54] = 1;
    assert!(PathfindingData::from_sbsp(&TagView::new(&b, BASE)).is_err());
    let mut b = sample();
    b[0xc4] = 0;
    assert_eq!(
        PathfindingData::from_sbsp(&TagView::new(&b, BASE)).unwrap(),
        None
    );
    let mut b = sample();
    b[0xc4] = 2;
    assert!(PathfindingData::from_sbsp(&TagView::new(&b, BASE)).is_err());
}
