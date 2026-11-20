use super::*;
use crate::brain::Me;
use crate::nav::Poly;

/// A floor 12 by 4 at z 0.
fn floor() -> NavMesh {
    NavMesh::new(
        vec![Poly::new(vec![
            [0.0, 0.0, 0.0],
            [12.0, 0.0, 0.0],
            [12.0, 4.0, 0.0],
            [0.0, 4.0, 0.0],
        ])],
        vec![],
    )
}

/// One triangle in the plane x = 6, far bigger than the floor: a wall
/// with no edge anywhere near a sight line over the floor.
fn thin_wall() -> Walls {
    Walls::new(vec![[[6.0, -50.0, -50.0], [6.0, 50.0, -50.0], [6.0, 0.0, 50.0]]])
}

/// A wall 0.2 thick across the floor at x 5.9 to 6.1, each face a quad
/// (two triangles, the diagonal from its low corner at y -1).
fn thick_wall() -> Walls {
    let face = |x: f32| [[x, -1.0, -2.0], [x, 5.0, -2.0], [x, 5.0, 4.0], [x, -1.0, 4.0]];
    let (a, b) = (face(5.9), face(6.1));
    Walls::from_polygons([&a[..], &b[..]])
}

/// A deterministic stream of numbers in `0..1`.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Every pair of points the grid calls blocked is blocked by the walk:
/// random points over the grid's space, and points on its cells' faces
/// and corners. Returns how many pairs it said were blocked.
fn never_is_certain(vis: &Vis, walls: &Walls) -> usize {
    assert!(walls.vis().is_none(), "check against the bare walk");
    let top: V3 = std::array::from_fn(|k| vis.origin[k] + vis.size[k] * vis.dims[k] as f32);
    let mut r = Lcg(7);
    let mut point = |r: &mut Lcg| -> V3 {
        let snap = r.next() < 0.3;
        std::array::from_fn(|k| {
            let v = vis.origin[k] + (top[k] - vis.origin[k]) * r.next();
            if snap {
                // On a cell boundary.
                vis.origin[k] + ((v - vis.origin[k]) / vis.size[k]).round() * vis.size[k]
            } else {
                v
            }
        })
    };
    let mut blocked = 0;
    for _ in 0..40_000 {
        let (a, b) = (point(&mut r), point(&mut r));
        if vis.never(a, b) {
            blocked += 1;
            assert!(!walls.clear(a, b), "{a:?} to {b:?} is clear, the grid said never");
            assert!(!walls.clear(b, a), "{b:?} to {a:?} is clear, the grid said never");
        }
    }
    blocked
}

#[test]
fn pair_bits_number_every_pair_once() {
    for n in [0, 1, 2, 7, 64, 65] {
        let mut all: Vec<usize> = (0..n)
            .flat_map(|i| (i + 1..n).map(move |j| pair_bit(n, i, j)))
            .collect();
        all.sort_unstable();
        assert_eq!(all, (0..pairs(n)).collect::<Vec<_>>(), "{n} cells");
    }
}

#[test]
fn a_wall_between_cells_means_never_and_open_space_maybe() {
    let walls = thin_wall();
    let vis = build(&floor(), &walls);
    assert!(vis.cells() > 0);
    let n = vis.cells();
    let boxes: Vec<Box3> = vis.voxels.iter().map(|&v| vis.voxel_box(v)).collect();
    let (mut across, mut same) = (0, 0);
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (boxes[i], boxes[j]);
            let bit = pair_bit(n, i, j);
            let never = (vis.bits[bit / 64] >> (bit % 64)) & 1 == 1;
            let left = |b: &Box3| b.hi[0] < 5.9;
            let right = |b: &Box3| b.lo[0] > 6.1;
            if (left(&a) && right(&b)) || (right(&a) && left(&b)) {
                assert!(never, "cells {a:?} and {b:?} are on either side of the wall");
                across += 1;
            } else if (left(&a) && left(&b)) || (right(&a) && right(&b)) {
                assert!(!never, "cells {a:?} and {b:?} see each other");
                same += 1;
            }
        }
    }
    assert!(across > 0 && same > 0);
    // Through the cells' own points.
    assert!(vis.never([1.5, 2.5, 0.6], [10.5, 2.5, 0.4]));
    assert!(!vis.never([1.5, 2.5, 0.6], [4.5, 1.5, 0.4]));
    // Outside the grid, or not a number: no answer, the ray is cast.
    assert!(!vis.never([1.5, 2.5, 0.6], [10.5, 2.5, 30.0]));
    assert!(!vis.never([f32::NAN, 2.5, 0.6], [10.5, 2.5, 0.4]));
    assert_eq!(vis.cell([1.5, 2.5, f32::INFINITY]), None);
    assert!(never_is_certain(&vis, &walls) > 0);
}

#[test]
fn the_grid_is_the_same_on_any_threads_and_at_background_priority() {
    let walls = thin_wall();
    let reference = build(&floor(), &walls);
    for count in [0, 1, 3, 7] {
        for background in [false, true] {
            let vis = build_with(&floor(), &walls, Threads { count, background });
            assert!(vis == reference, "{count} threads, background {background}");
        }
    }
    assert!(Threads::background().background && Threads::background().count >= 1);
    assert!(!Threads::foreground().background && Threads::foreground().count >= 1);
}

#[test]
fn a_wall_of_several_triangles_is_proved_where_one_covers_and_never_wrong() {
    let walls = thick_wall();
    let vis = build(&floor(), &walls);
    // Low across the wall: far from either face's diagonal.
    assert!(vis.never([0.5, 2.5, -0.05], [11.5, 2.5, -0.05]));
    assert!(never_is_certain(&vis, &walls) > 0);
}

#[test]
fn open_space_has_no_never() {
    let walls = Walls::default();
    let vis = build(&floor(), &walls);
    assert!(vis.cells() > 0);
    assert_eq!(vis.never_pairs(), 0);
}

#[test]
fn no_walkable_space_gives_an_empty_grid() {
    let vis = build(&NavMesh::default(), &thin_wall());
    assert_eq!(vis.cells(), 0);
    assert!(!vis.never([1.0, 1.0, 1.0], [10.0, 1.0, 1.0]));
    let stamp = Stamp::now(3);
    assert_eq!(Vis::from_bytes(&vis.to_bytes(stamp)), Ok((vis, stamp)));
}

#[test]
fn the_file_round_trips_and_refuses_damage() {
    let walls = thin_wall();
    let vis = build(&floor(), &walls);
    assert!(vis.never_pairs() > 0);
    let stamp = Stamp::now(0x0123_4567_89ab_cdef);
    let b = vis.to_bytes(stamp);
    let (back, s) = Vis::from_bytes(&b).unwrap();
    assert_eq!(s, stamp);
    assert_eq!(back, vis);
    assert_eq!(back.walls, walls_hash(&walls));
    assert_eq!(Vis::from_bytes(&b[..b.len() - 1]), Err(Status::Damaged));
    let mut long = b.clone();
    long.extend_from_slice(&[0; 8]);
    assert_eq!(Vis::from_bytes(&long), Err(Status::Damaged));
    assert_eq!(Vis::from_bytes(&b[..HEADER_LEN]), Err(Status::Damaged));
    assert_eq!(Vis::from_bytes(&[]), Err(Status::Damaged));
    let mut old = b.clone();
    old[4..8].copy_from_slice(&(VERSION + 1).to_le_bytes());
    assert_eq!(Vis::from_bytes(&old), Err(Status::OldVersion(VERSION + 1)));
    let mut magic = b.clone();
    magic[0] = b'X';
    assert_eq!(Vis::from_bytes(&magic), Err(Status::Damaged));
    // A bit past the last pair.
    let used = pairs(vis.cells());
    if used % 64 != 0 {
        let mut stray = b.clone();
        let last = stray.len() - 1;
        stray[last] |= 0x80;
        assert_eq!(Vis::from_bytes(&stray), Err(Status::Damaged));
    }
}

#[test]
fn file_names_sit_beside_the_bots_file() {
    assert_eq!(file_name("lockout", 0), "lockout.0.vis");
    assert_eq!(file_name("../x y", 1), "___x_y.1.vis");
}

#[test]
fn perception_asks_the_grid_first_and_walks_without_one() {
    let walled = thin_wall();
    let vis = Arc::new(build(&floor(), &walled));
    let me = Me {
        position: [1.5, 2.5, 0.0],
        ..Default::default()
    };
    let target = [10.5, 2.5, 0.0];
    let sees = |walls: &Walls| {
        let map = MapData {
            walls: walls.clone(),
            ..Default::default()
        };
        crate::perceive::in_view(me, target, crate::mind::CHEST, Some(&map))
    };
    // Open space with no grid: seen.
    let open = Walls::default();
    assert!(sees(&open));
    // The same open space carrying the walled map's grid: not seen, so
    // the grid answered and no ray was walked. (Never done for real:
    // `load` attaches a grid only to the walls it was built from.)
    assert!(open.set_vis(Arc::clone(&vis)));
    assert!(!sees(&open));
    // The wall, with and without its grid: the same answer.
    let walled_vis = walled.clone();
    assert!(walled_vis.set_vis(vis));
    assert!(!sees(&walled));
    assert!(!sees(&walled_vis));
    // Beside the bot, on its side: the grid has no answer, the ray says.
    let near = [4.5, 2.5, 0.0];
    let map = MapData {
        walls: walled_vis,
        ..Default::default()
    };
    assert!(crate::perceive::in_view(me, near, crate::mind::CHEST, Some(&map)));
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("bots-vis-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn data() -> MapData {
    MapData {
        nav: floor(),
        walls: thin_wall(),
        ..Default::default()
    }
}

#[test]
fn a_missing_or_stale_grid_is_not_used_and_says_why() {
    let dir = tmp("load");
    let stamp = Stamp::now(5);
    let walls = thin_wall();
    let line = load(&dir, "lockout", 0, stamp, &walls);
    assert!(line.starts_with("bots: lockout: visibility grid not used (lockout.0.vis: missing)"), "{line}");
    assert!(line.ends_with("casting every sight ray"), "{line}");
    assert!(walls.vis().is_none());

    let vis = build(&floor(), &walls);
    let path = dir.join(file_name("lockout", 0));
    // Built for another `.bots` build.
    std::fs::write(&path, vis.to_bytes(Stamp::now(6))).unwrap();
    let line = load(&dir, "lockout", 0, stamp, &walls);
    assert!(line.contains("built for another nav build"), "{line}");
    assert!(walls.vis().is_none());
    // Damaged.
    std::fs::write(&path, b"H2VS").unwrap();
    let line = load(&dir, "lockout", 0, stamp, &walls);
    assert!(line.contains("damaged"), "{line}");
    // Another version.
    let mut old = vis.to_bytes(stamp);
    old[4..8].copy_from_slice(&0u32.to_le_bytes());
    std::fs::write(&path, &old).unwrap();
    let line = load(&dir, "lockout", 0, stamp, &walls);
    assert!(line.contains(&format!("version 0, now {VERSION}")), "{line}");
    assert!(walls.vis().is_none());
    // From other walls.
    std::fs::write(&path, vis.to_bytes(stamp)).unwrap();
    let other = thick_wall();
    let line = load(&dir, "lockout", 0, stamp, &other);
    assert!(line.contains("built from other walls"), "{line}");
    assert!(other.vis().is_none());
    // Its own: attached.
    let line = load(&dir, "lockout", 0, stamp, &walls);
    assert!(line.starts_with("bots: lockout: visibility grid: "), "{line}");
    assert_eq!(walls.vis().map(|v| &**v), Some(&vis));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_fresh_nav_file_without_a_grid_gets_one_built() {
    let dir = tmp("rebuild");
    crate::autonav::write_map(&dir, "lockout", 9, &[data()]).unwrap();
    assert_eq!(status(&dir, "lockout"), Status::Missing);
    let d = rebuild(&dir, "lockout").unwrap();
    assert!(d.walls.vis().is_some_and(|v| v.never_pairs() > 0));
    assert_eq!(status(&dir, "lockout"), Status::Fresh);
    // The nav build changes (another map file): the grid is stale.
    crate::autonav::write_map(&dir, "lockout", 10, &[data()]).unwrap();
    assert_eq!(status(&dir, "lockout"), Status::OtherBuild);
    // The worker's own build writes both, fresh together.
    let with = data();
    with.walls.set_vis(Arc::new(build(&with.nav, &with.walls)));
    crate::autonav::write_map(&dir, "lockout", 11, &[with]).unwrap();
    assert_eq!(status(&dir, "lockout"), Status::Fresh);
    let walls = thin_wall();
    let line = load(&dir, "lockout", 0, Stamp::now(11), &walls);
    assert!(walls.vis().is_some(), "{line}");
    std::fs::write(dir.join(file_name("lockout", 0)), b"junk").unwrap();
    assert_eq!(status(&dir, "lockout"), Status::Damaged);
    for s in [Status::Missing, Status::Damaged, Status::OldVersion(0), Status::OtherBuild] {
        assert!(s.stale(), "{s:?}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_stopped_grid_build_keeps_its_progress_and_carries_on_to_the_same_grid() {
    let dir = tmp("resume");
    crate::autonav::write_map(&dir, "lockout", 9, &[data()]).unwrap();
    let full = build(&floor(), &thin_wall());
    assert!(full.cells() > 2);
    // Stopped after its first chunk: no grid yet, its progress saved.
    let stopped = rebuild_resumable(&dir, "lockout", Threads::foreground(), &|| true).unwrap();
    assert!(stopped.is_none());
    assert_eq!(status(&dir, "lockout"), Status::Missing);
    let part = dir.join(part_name("lockout", 0));
    let fresh = layout(&floor(), &thin_wall());
    let (_, row) = read_part(&part, Stamp::now(9), &fresh).expect("progress saved");
    assert!(row > 0 && row + 1 < full.cells(), "row {row} of {}", full.cells());
    // Progress for another nav build is not used.
    assert!(read_part(&part, Stamp::now(10), &fresh).is_none());
    // Carried on: the grid a build in one go makes, written, progress gone.
    let d = rebuild_resumable(&dir, "lockout", Threads::background(), &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(d.walls.vis().map(|v| &**v), Some(&full));
    assert_eq!(status(&dir, "lockout"), Status::Fresh);
    assert!(!part.exists());
    // Current: loaded, not built again (no stop is ever asked).
    let again = rebuild_resumable(&dir, "lockout", Threads::foreground(), &|| panic!("built again"))
        .unwrap()
        .unwrap();
    assert_eq!(again.walls.vis().map(|v| &**v), Some(&full));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn light_index_budget_preserves_dense_cell_and_pair_answers() {
    // Just large enough to exceed the light index budget. Include both
    // ends of the lattice and a pair with a set visibility bit.
    let columns = (LIGHT_INDEX_BYTES / size_of::<u16>() + 1) as u32;
    let make = |budget| {
        let mut grid = Vis::with_index_budget(
            [0.0; 3], [1.0; 3], [columns, 1, 1], vec![0, 10, columns - 1], 0, budget,
        ).unwrap();
        grid.bits[0] = 1;
        grid
    };
    let light = make(LIGHT_INDEX_BYTES);
    let dense = make(usize::MAX);
    assert_eq!(light.index.capacity(), 0);
    assert_eq!(dense.index.len(), columns as usize);
    for x in -1..=columns as i32 {
        let p = [x as f32 + 0.5, 0.5, 0.5];
        assert_eq!(light.cell(p), dense.cell(p));
        assert_eq!(light.never([0.5; 3], p), dense.never([0.5; 3], p));
    }
    assert_eq!(light.to_bytes(Stamp::now(7)), dense.to_bytes(Stamp::now(7)));
    assert!(Vis::with_index_budget(
        [0.0; 3], [1.0; 3], [columns, 1, 1], vec![columns], 0, 0,
    ).is_none());
}
