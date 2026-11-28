//! The flood on invented portal graphs (no game data).
use super::*;
use crate::synth::{CONSTS, Image, PortalSpec, Portals, Rng, random_portals, square};

const BASE: u32 = 0x10_000;
const MARKS: u32 = BASE + 0xc000;
const EPOCH: u32 = 7;
const SINGLE: u16 = 0x007f;
const DOUBLE: u16 = 0x027f;

/// A wall `x = at` between clusters `a` and `b`: a square of half-size 1
/// around the x axis, wound so the inside passes the polygon test.
fn wall(p: &mut Portals, a: i16, b: i16, at: f32) -> i16 {
    p.planes.push([1.0, 0.0, 0.0, at]);
    let mut vertices = square(0, at, (0.0, 0.0), 1.0);
    vertices.reverse();
    p.link(
        a,
        b,
        PortalSpec {
            plane: p.planes.len() as u32 - 1,
            sphere: [at, 0.0, 0.0, 1.5],
            vertices,
            ..PortalSpec::default()
        },
    )
}

/// Clusters `0..n` in a row, walls at `x = 1, 2, ...`.
fn row(n: usize) -> Portals {
    let mut p = Portals {
        clusters: vec![Vec::new(); n],
        ..Portals::default()
    };
    for k in 1..n {
        wall(&mut p, k as i16 - 1, k as i16, k as f32);
    }
    p
}

fn image(p: &Portals) -> (Image, TopologyKey) {
    let mut img = Image::new(BASE, 0x10_000);
    let (end, bsp) = p.write(BASE, |a, d| img.put(a, d));
    assert!(end < MARKS);
    (img, TopologyKey { structure: BASE, bsp })
}

fn decode(p: &Portals) -> (Image, Topology) {
    let (img, key) = image(p);
    let t = Topology::decode(&img, key, &CONSTS.projection).unwrap();
    (img, t)
}

fn query(cluster: u16, centre: [f32; 3], radius: f32) -> FloodQuery {
    FloodQuery {
        cluster,
        centre: centre.map(f32::to_bits),
        radius: radius.to_bits(),
        capacity: 16,
        control: DOUBLE,
        marks: MARKS,
        epoch: EPOCH,
    }
}

fn flood(p: &Portals, q: &FloodQuery) -> Result<Flood, Bail> {
    let (img, t) = decode(p);
    t.cluster_flood(&img, q)
}

#[test]
fn the_dominant_axis_prefers_z_then_y_on_ties() {
    let axis = |n: [f32; 3]| dominant_axis(n.map(f32::to_bits));
    assert_eq!(axis([1.0, 1.0, 1.0]), 2);
    assert_eq!(axis([1.0, 1.0, 0.5]), 1);
    assert_eq!(axis([2.0, -1.0, 1.0]), 0);
    assert_eq!(axis([-3.0, 1.0, 2.0]), 0);
    assert_eq!(axis([0.0, -1.0, 1.0]), 2);
    assert_eq!(axis([0.0, -1.0, 0.0]), 1);
    assert_eq!(axis([-0.0, 0.0, -0.0]), 2);
}

#[test]
fn a_sphere_crosses_the_walls_it_reaches_and_no_further() {
    let p = row(4);
    for control in [SINGLE, DOUBLE] {
        let q = FloodQuery {
            control,
            ..query(0, [0.5, 0.0, 0.0], 0.75)
        };
        let f = flood(&p, &q).unwrap();
        assert_eq!(f.clusters, [0, 1], "{control:#x}");
        assert_eq!((f.written, f.count(), f.deepest), (2, 2, Some(1)));
        assert_eq!(f.portal_count, 1);
        // From the middle, both neighbours.
        let f = flood(&p, &FloodQuery { control, ..query(1, [1.5, 0.0, 0.0], 0.75) }).unwrap();
        assert_eq!(f.clusters, [1, 0, 2]);
        assert_eq!(f.deepest, Some(1));
        assert_eq!(f.portal_count, 2);
    }
    // A big sphere floods the whole row, depth first.
    let f = flood(&p, &query(0, [0.5, 0.0, 0.0], 5.0)).unwrap();
    assert_eq!(f.clusters, [0, 1, 2, 3]);
    assert_eq!(f.deepest, Some(2), "the last cluster's only wall leads back");
}

#[test]
fn a_full_output_still_counts_every_cluster() {
    let p = row(4);
    let f = flood(&p, &FloodQuery { capacity: 2, ..query(0, [0.5, 0.0, 0.0], 5.0) }).unwrap();
    assert_eq!((f.written, f.count()), (2, 4));
    let f = flood(&p, &FloodQuery { capacity: 0, ..query(0, [0.5, 0.0, 0.0], 5.0) }).unwrap();
    assert_eq!((f.written, f.count()), (0, 4));
    assert_eq!(
        flood(&p, &FloodQuery { capacity: 0x8000, ..query(0, [0.5, 0.0, 0.0], 5.0) }),
        Err(Bail::Data),
        "a capacity whose low half is negative"
    );
}

#[test]
fn marked_clusters_are_not_entered_and_run_no_test() {
    let p = row(3);
    let (mut img, t) = decode(&p);
    img.word(MARKS + 4, EPOCH);
    let f = t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 5.0)).unwrap();
    assert_eq!(f.clusters, [0]);
    assert_eq!(f.deepest, None);
    // A stale mark is no mark. The first cluster is entered marked or not.
    img.word(MARKS + 4, EPOCH - 1);
    img.word(MARKS, EPOCH);
    let f = t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 5.0)).unwrap();
    assert_eq!(f.clusters, [0, 1, 2]);
}

#[test]
fn the_bounding_sphere_and_the_polygon_each_reject() {
    let p = row(2);
    // Beside the portal's bounding sphere.
    let f = flood(&p, &query(0, [0.5, 5.0, 0.0], 0.75)).unwrap();
    assert_eq!(f.clusters, [0]);
    assert_eq!(f.deepest, Some(0));
    // Inside the bounding sphere, but the circle on the wall misses the square.
    let mut q = p.clone();
    q.portals[0].sphere[3] = 10.0;
    let f = flood(&q, &query(0, [0.5, 3.0, 0.0], 0.75)).unwrap();
    assert_eq!(f.clusters, [0]);
    // Grazing a corner: the circle reaches the square.
    let f = flood(&q, &query(0, [0.5, 1.25, 0.0], 0.75)).unwrap();
    assert_eq!(f.clusters, [0, 1]);
    // The wrong winding rejects points inside.
    q.portals[0].vertices.reverse();
    let f = flood(&q, &query(0, [0.5, 0.0, 0.0], 0.75)).unwrap();
    assert_eq!(f.clusters, [0]);
}

#[test]
fn a_portal_with_no_sides_passes_and_bad_counts_decline() {
    let mut p = row(2);
    p.portals[0].count = Some(0);
    assert_eq!(flood(&p, &query(0, [0.5, 0.0, 0.0], 0.75)).unwrap().clusters, [0, 1]);
    for count in [0x1_0003, 0xffff_0003, MAX_SIDES + 1] {
        p.portals[0].count = Some(count);
        assert_eq!(
            flood(&p, &query(0, [0.5, 0.0, 0.0], 0.75)),
            Err(Bail::Data),
            "{count:#x}"
        );
        // Not reached: the far cluster is marked.
        let (mut img, t) = decode(&p);
        img.word(MARKS + 4, EPOCH);
        assert!(t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 0.75)).is_ok());
    }
    // Negative counts copy nothing and the low half says no sides.
    p.portals[0].count = Some(0xffff_ffff);
    assert_eq!(flood(&p, &query(0, [0.5, 0.0, 0.0], 0.75)).unwrap().clusters, [0, 1]);
}

#[test]
fn a_portal_listed_by_a_third_cluster_leads_to_its_first_cluster() {
    let mut p = row(3);
    // The wall between 1 and 2 is also listed by 0: from 0 its far side
    // is its first cluster, 1.
    p.clusters[0].insert(0, 1);
    let f = flood(&p, &query(0, [1.5, 0.0, 0.0], 0.75)).unwrap();
    assert_eq!(f.clusters, [0, 1, 2]);
}

#[test]
fn clusters_outside_the_decoded_graph_decline() {
    let mut p = row(2);
    p.portals[0].clusters[1] = 9;
    assert_eq!(flood(&p, &query(0, [0.5, 0.0, 0.0], 5.0)), Err(Bail::Data));
    p.portals[0].clusters[1] = -2;
    assert_eq!(flood(&p, &query(0, [0.5, 0.0, 0.0], 5.0)), Err(Bail::Data));
    let p = row(2);
    assert_eq!(flood(&p, &query(2, [0.5, 0.0, 0.0], 5.0)), Err(Bail::Data));
    assert_eq!(flood(&p, &query(0xffff, [0.5, 0.0, 0.0], 5.0)), Err(Bail::Data));
}

#[test]
fn operands_off_the_fast_path_and_extended_precision_decline() {
    let p = row(2);
    let tiny = f32::from_bits(1);
    assert_eq!(flood(&p, &query(0, [0.5, tiny, 0.0], 0.75)), Err(Bail::Float));
    assert_eq!(flood(&p, &query(0, [f32::NAN, 0.0, 0.0], 0.75)), Err(Bail::Float));
    let q = FloodQuery {
        control: 0x037f,
        ..query(0, [0.5, 0.0, 0.0], 0.75)
    };
    assert_eq!(flood(&p, &q), Err(Bail::Float));
    // No portal test runs: the operands never reach an operation.
    let lone = Portals {
        clusters: vec![Vec::new()],
        ..Portals::default()
    };
    assert_eq!(flood(&lone, &q).unwrap().clusters, [0]);
}

#[test]
fn the_decode_names_its_sources_and_shares_portals() {
    let p = row(3);
    let (img, t) = decode(&p);
    assert_eq!(t.clusters(), 3);
    assert_eq!(t.portals.len(), 2, "each wall decoded once");
    let s = t.sources();
    let covers = |a: u32, n: u32| s.iter().any(|&(at, len)| at <= a && a + n <= at + len);
    assert!(covers(BASE + layout::COLLISION_BSP, 4));
    assert!(covers(BASE + layout::PORTALS, 4));
    assert!(covers(BASE + layout::CLUSTER_COUNT, 8));
    let wall: Vec<&Portal> = t.portals_of(1).unwrap().collect();
    assert_eq!(wall.len(), 2);
    assert_eq!(wall[0].clusters, [0, 1]);
    let projection = wall[1].test.as_ref().unwrap().projection.as_ref().unwrap();
    // x normal, positive: row 1 of the made-up table, z then y.
    assert_eq!(projection.axes, [2, 1]);
    assert_eq!(t.ring(projection).len(), 4);
    assert!(t.footprint() > 0);
    // The decode reads only as many clusters as the block says.
    let mut img = img;
    img.word(BASE + layout::CLUSTER_COUNT, 2);
    let t = Topology::decode(&img, t.key(), &CONSTS.projection).unwrap();
    assert_eq!(t.clusters(), 2);
    assert_eq!(
        t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 5.0)),
        Err(Bail::Data),
        "the third cluster is past the block"
    );
}

#[test]
fn an_unreadable_portal_list_declines_the_cluster_not_the_decode() {
    let p = row(3);
    let (mut img, key) = image(&p);
    let clusters = u32::from_le_bytes(
        img.bytes[(BASE + layout::CLUSTERS - BASE) as usize..][..4]
            .try_into()
            .unwrap(),
    );
    let list = clusters + 2 * layout::CLUSTER_STRIDE + layout::CLUSTER_PORTAL_COUNT + 4;
    img.word(list, 0xdead_0000);
    let t = Topology::decode(&img, key, &CONSTS.projection).unwrap();
    assert_eq!(t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 0.75)).unwrap().clusters, [0, 1]);
    assert_eq!(t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 5.0)), Err(Bail::Fault));
}

/// Under the guest's state the cases the checked path declines run, as
/// the guest's instructions do.
#[test]
fn the_guest_flood_runs_what_the_checked_path_declines() {
    let p = row(2);
    let (img, t) = decode(&p);
    let guest = |q: &FloodQuery| t.cluster_flood_guest(&img, q, 0x1f80);
    // On the checked path both agree.
    let plain = query(0, [0.5, 0.0, 0.0], 0.75);
    assert_eq!(guest(&plain).unwrap().0, t.cluster_flood(&img, &plain).unwrap());
    // A subnormal centre: DE on the x87 load, and the same flood.
    let tiny = guest(&query(0, [0.5, f32::from_bits(1), 0.0], 0.75)).unwrap();
    assert_eq!(tiny.0.clusters, t.cluster_flood(&img, &plain).unwrap().clusters);
    assert_ne!(tiny.1.x87 & 0x02, 0);
    // A NaN centre: the distance is unordered, `jbe` is taken, no portal
    // passes, and `fcomip` raises IE.
    let nan = guest(&query(0, [f32::NAN, 0.0, 0.0], 0.75)).unwrap();
    assert_eq!(nan.0.clusters, [0]);
    assert_ne!(nan.1.x87 & 0x01, 0);
    // Extended precision runs on the model.
    let ext = FloodQuery {
        control: 0x037f,
        ..plain
    };
    assert_eq!(guest(&ext).unwrap().0.clusters, t.cluster_flood(&img, &plain).unwrap().clusters);
    // A subnormal normal component decodes, and its load raises DE.
    let mut p = row(3);
    p.planes[1] = [1.0, f32::from_bits(3), 0.0, 2.0];
    let (img, t) = decode(&p);
    let r = t.cluster_flood_guest(&img, &query(0, [0.5, 0.0, 0.0], 0.75), 0x1f80).unwrap();
    assert_ne!(r.1.x87 & 0x02, 0);
}

#[test]
fn a_projection_is_needed_only_past_the_plane_and_sphere_tests() {
    // Wall 2's normal has a subnormal component. The distance plane and the
    // projection read the same plane here, and the plane test loads every
    // component of it, so the flood bails before any sphere is turned away.
    let mut p = row(3);
    p.planes[1] = [1.0, f32::from_bits(3), 0.0, 2.0];
    let (img, t) = decode(&p);
    assert_eq!(t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 0.75)), Err(Bail::Float));
    assert_eq!(t.cluster_flood(&img, &query(1, [1.5, 0.0, 0.0], 0.75)), Err(Bail::Float));
    // Likewise a vertex count the guest cannot use.
    let mut p = row(3);
    p.portals[1].count = Some(MAX_SIDES + 1);
    let (img, t) = decode(&p);
    assert_eq!(
        t.cluster_flood(&img, &query(0, [0.5, 0.0, 0.0], 0.75)).unwrap().clusters,
        [0, 1]
    );
    assert_eq!(t.cluster_flood(&img, &query(1, [1.5, 0.0, 0.0], 0.75)), Err(Bail::Data));
}

#[test]
fn random_graphs_flood_the_same_in_single_and_double_precision() {
    // The made-up graphs use eighths, where the two precisions agree.
    for n in 0..200u64 {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let clusters = 2 + rng.below(9);
        let p = random_portals(&mut rng, clusters);
        let (img, t) = decode(&p);
        let cluster = rng.below(t.clusters() as u32) as u16;
        let centre = [rng.eighths(8), rng.eighths(4), rng.eighths(4)];
        let radius = [0.5f32, 1.0, 2.5, 6.0][rng.below(4) as usize];
        let q = query(cluster, centre, radius);
        let single = t.cluster_flood(&img, &FloodQuery { control: SINGLE, ..q });
        let double = t.cluster_flood(&img, &q);
        if let (Ok(a), Ok(b)) = (&single, &double) {
            assert_eq!(a, b, "case {n}");
            assert_eq!(a.clusters[0], cluster);
            let mut seen = a.clusters.clone();
            seen.sort_unstable();
            seen.dedup();
            assert_eq!(seen.len(), a.clusters.len(), "case {n}: a cluster visited twice");
        }
    }
}
