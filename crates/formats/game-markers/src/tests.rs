use crate::area::{self, Area, AreaError, Shape};
use crate::kind::{Item, Kind, Solid, Volume};
use crate::mesh::{Mesh, Place, Vec3};
use crate::palette::{self, Palette};
use crate::{FILL_ALPHA, LIFT, LINE_ALPHA, Marker};

const EPS: f32 = 1e-4;

fn near(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= EPS)
}

fn white(kind: &Kind) -> Mesh {
    kind.mesh([1.0; 3])
}

#[test]
fn every_kind_has_its_lines_faces_and_bounds() {
    // Lines, faces, the least corner and the greatest, in the order of
    // `Kind::samples`. The sums are in `Kind::mesh`'s comments.
    let hill = ([-1.0, -1.0, -0.5], [1.0, 1.0, 1.5]);
    let cube = ([-1.0; 3], [1.0; 3]);
    let expected: Vec<(&str, usize, usize, Vec3, Vec3)> = vec![
        ("spawn", 38, 16, [-0.35, -0.35, 0.0], [0.70, 0.35, 0.70]),
        ("flag_stand", 20, 17, [-0.3, -0.3, LIFT], [0.3, 0.3, 0.9]),
        ("flag_return", 35, 16, [-0.3, -0.3, LIFT], [0.3, 0.3, 0.6]),
        ("bomb_spawn", 28, 24, [-0.3, -0.3, LIFT], [0.3, 0.3, 0.45]),
        ("bomb_goal", 34, 16, [-0.4, -0.4, LIFT], [0.4, 0.4, LIFT]),
        ("ball_spawn", 52, 16, [-0.3, -0.3, LIFT], [0.3, 0.3, 0.42]),
        ("hill", 105, 64, hill.0, hill.1),
        ("territory", 109, 72, hill.0, hill.1),
        (
            "teleporter_sender",
            51,
            32,
            [-0.4, -0.4, LIFT],
            [0.4, 0.4, 1.0],
        ),
        (
            "teleporter_receiver",
            54,
            32,
            [-0.4, -0.4, LIFT],
            [0.8, 0.4, 1.0],
        ),
        ("weapon_spawn", 25, 16, [-0.3, -0.3, LIFT], [0.3, 0.3, 0.42]),
        (
            "vehicle_spawn",
            25,
            16,
            [-1.0, -1.0, LIFT],
            [1.4, 1.0, 0.75],
        ),
        (
            "equipment_spawn",
            28,
            24,
            [-0.3, -0.3, LIFT],
            [0.3, 0.3, 0.47],
        ),
        ("trigger_volume", 12, 12, cube.0, cube.1),
        ("kill_volume", 12, 12, cube.0, cube.1),
        ("soft_volume", 12, 12, cube.0, cube.1),
        ("bsp_switch", 12, 12, cube.0, cube.1),
        ("trigger_volume", 96, 64, [-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]),
        ("trigger_volume", 96, 0, cube.0, cube.1),
        ("ff_drop", 22, 16, [-0.5, -0.5, LIFT], [0.9, 0.5, 1.2]),
        ("ff_base", 21, 3, [-0.6, -0.6, LIFT], [0.6, 0.6, 0.65]),
        ("ff_cover", 23, 2, [-0.25, -0.3, LIFT], [0.6, 0.3, 0.5]),
        ("siege_site", 109, 72, hill.0, hill.1),
        ("siege_goal", 45, 8, [-0.5, -0.5, LIFT], [0.5, 0.5, 0.8]),
        ("siege_line", 7, 2, [0.0, -1.0, LIFT], [0.5, 1.0, 0.5]),
    ];
    let samples = Kind::samples();
    assert_eq!(samples.len(), expected.len());
    for (kind, (name, lines, tris, min, max)) in samples.iter().zip(expected) {
        assert_eq!(kind.name(), name);
        let mesh = white(kind);
        assert_eq!(mesh.lines.len(), lines, "{name}: lines");
        assert_eq!(mesh.tris.len(), tris, "{name}: faces");
        assert_eq!(
            mesh.vertex_count(),
            lines * 2 + tris * 3,
            "{name}: vertices"
        );
        let (low, high) = mesh.bounds().unwrap();
        assert!(near(low, min), "{name}: least corner {low:?}, not {min:?}");
        assert!(
            near(high, max),
            "{name}: greatest corner {high:?}, not {max:?}"
        );
        assert!(
            mesh.points().all(|p| p.iter().all(|c| c.is_finite())),
            "{name}"
        );
    }
}

#[test]
fn a_marker_is_its_teams_colour_with_solid_lines_and_a_thin_skin() {
    let palette = Palette::default();
    for (team, colour) in [
        (Some(0), palette::TEAMS[0]),
        (Some(1), palette::TEAMS[1]),
        (Some(7), palette::TEAMS[7]),
        (None, palette::NEUTRAL),
        // A team past the eighth is no team.
        (Some(8), palette::NEUTRAL),
    ] {
        let marker = Marker::new(Kind::Spawn, team);
        assert_eq!(marker.colour(&palette), colour);
        let mesh = marker.mesh(&palette);
        let [r, g, b] = colour;
        assert!(mesh.lines.iter().all(|l| l.colour == [r, g, b, LINE_ALPHA]));
        assert!(mesh.tris.iter().all(|t| t.colour == [r, g, b, FILL_ALPHA]));
    }
}

#[test]
fn a_kinds_own_colour_holds_or_gives_way_to_a_team() {
    let palette = Palette::default();
    let colour = |kind: Kind, team| Marker::new(kind, team).colour(&palette);
    // Fixed: a team changes nothing.
    assert_eq!(colour(Kind::TeleporterSender, Some(0)), palette::SENDER);
    assert_eq!(colour(Kind::TeleporterReceiver, None), palette::RECEIVER);
    assert_eq!(colour(Kind::Drop, Some(1)), palette::DROP);
    assert_eq!(colour(Kind::Cover, None), palette::COVER);
    assert_eq!(colour(Kind::Goal, Some(1)), palette::SIEGE);
    assert_eq!(colour(Kind::Site(Area::default()), None), palette::SIEGE);
    let cube = Solid::Box { half: [1.0; 3] };
    assert_eq!(
        colour(Kind::Volume(Volume::Kill, cube), Some(0)),
        palette::KILL
    );
    assert_eq!(
        colour(Kind::Volume(Volume::Soft, cube), None),
        palette::SOFT
    );
    // Its own colour without a team, the team's with one.
    assert_eq!(colour(Kind::Item(Item::Weapon), None), palette::WEAPON);
    assert_eq!(colour(Kind::Item(Item::Weapon), Some(1)), palette::TEAMS[1]);
    assert_eq!(colour(Kind::Line, None), palette::SIEGE);
    assert_eq!(colour(Kind::Line, Some(0)), palette::TEAMS[0]);
    // A team's or neutral.
    assert_eq!(colour(Kind::Base, Some(1)), palette::TEAMS[1]);
    assert_eq!(colour(Kind::Hill(Area::default()), None), palette::NEUTRAL);
    // The game's own team colours replace the stand-ins.
    let mut map = Palette::default();
    map.teams[0] = [0.5, 0.1, 0.1];
    assert_eq!(
        Marker::new(Kind::FlagStand, Some(0)).colour(&map),
        [0.5, 0.1, 0.1]
    );
}

#[test]
fn a_placed_marker_turns_with_its_facing() {
    let mesh = white(&Kind::Spawn);
    let placed = mesh.placed(&Place::facing(
        [10.0, 20.0, 3.0],
        std::f32::consts::FRAC_PI_2,
    ));
    assert_eq!(placed.lines.len(), mesh.lines.len());
    assert_eq!(placed.tris.len(), mesh.tris.len());
    // The arrow that ran along x runs along y.
    let (low, high) = placed.bounds().unwrap();
    assert!(near(low, [9.65, 19.65, 3.0]), "{low:?}");
    assert!(near(high, [10.35, 20.70, 3.70]), "{high:?}");
    // No turn: the mesh moved.
    let moved = mesh.placed(&Place::facing([1.0, 2.0, 3.0], 0.0));
    let (low, high) = moved.bounds().unwrap();
    assert!(near(low, [0.65, 1.65, 3.0]) && near(high, [1.70, 2.35, 3.70]));
}

#[test]
fn a_faded_mesh_keeps_its_colour_and_loses_alpha() {
    let mesh = white(&Kind::FlagStand).faded(0.5);
    assert!(
        mesh.lines
            .iter()
            .all(|l| l.colour == [1.0, 1.0, 1.0, LINE_ALPHA * 0.5])
    );
    assert!(
        mesh.tris
            .iter()
            .all(|t| t.colour == [1.0, 1.0, 1.0, FILL_ALPHA * 0.5])
    );
    assert!(Mesh::default().bounds().is_none());
    assert!(Mesh::default().is_empty());
}

#[test]
fn a_label_names_its_marker() {
    for text in [
        "ctf_flag_spawn",
        "ctf_flag_return",
        "assault_bomb_spawn",
        "assault_bomb_return",
        "oddball_spawn",
        "teleporter_sender",
        "teleporter_receiver",
        "territory",
    ] {
        assert!(Kind::of_label(text).is_some(), "{text}");
    }
    for text in [
        "ff_drop",
        "ff_base",
        "ff_cover",
        "siege_site",
        "siege_goal",
        "siege_line",
    ] {
        assert_eq!(Kind::of_label(text).unwrap().name(), text);
    }
    assert_eq!(
        Kind::of_label("territory").unwrap().area(),
        Some(&Area::default())
    );
    assert_eq!(Kind::of_label("hill_corner"), None);
    assert_eq!(Kind::of_label("checkpoint"), None);
    assert!(Kind::Spawn.faces() && !Kind::Goal.faces());
}

fn square(half: f32) -> Vec<[f32; 2]> {
    vec![[-half, -half], [half, -half], [half, half], [-half, half]]
}

#[test]
fn a_hill_on_a_path_has_a_wall_an_edge() {
    let mut clockwise = square(2.0);
    clockwise.reverse();
    for points in [square(2.0), clockwise] {
        let area = Area::path(points);
        assert_eq!(area.validate(), Ok(()));
        let mesh = white(&Kind::Hill(area));
        // Three lines and two faces a point, and the pip's nine lines.
        assert_eq!((mesh.lines.len(), mesh.tris.len()), (4 * 3 + 9, 4 * 2));
        let (low, high) = mesh.bounds().unwrap();
        assert!(near(low, [-2.0, -2.0, -0.5]) && near(high, [2.0, 2.0, 1.5]));
    }
    // A wider radius is a wider wall with the same lines.
    let mesh = white(&Kind::Hill(Area::radius(3.0)));
    assert_eq!((mesh.lines.len(), mesh.tris.len()), (105, 64));
    let (low, high) = mesh.bounds().unwrap();
    assert!(near(low, [-3.0, -3.0, -0.5]) && near(high, [3.0, 3.0, 1.5]));
}

#[test]
fn the_default_area_is_the_cylinder_round_one_flag() {
    let area = Area::default();
    assert_eq!(area.shape, Shape::Radius(1.0));
    assert_eq!((area.below, area.above), (0.5, 1.5));
    assert_eq!(area.validate(), Ok(()));
    assert_eq!(area.outline().len(), area::SEGMENTS);
}

#[test]
fn a_radius_area_is_refused_outside_its_limits() {
    assert_eq!(Area::radius(0.1).validate(), Err(AreaError::Radius(0.1)));
    assert_eq!(
        Area::radius(100.0).validate(),
        Err(AreaError::Radius(100.0))
    );
    assert_eq!(Area::radius(area::MIN_RADIUS).validate(), Ok(()));
    assert_eq!(Area::radius(area::MAX_REACH).validate(), Ok(()));
    assert_eq!(Area::radius(f32::NAN).validate(), Err(AreaError::NotFinite));
    assert_eq!(
        Area::radius(f32::INFINITY).validate(),
        Err(AreaError::NotFinite)
    );
    let heights = |below, above| Area {
        shape: Shape::Radius(1.0),
        below,
        above,
    };
    assert_eq!(heights(0.0, 1.0).validate(), Ok(()));
    for (below, above) in [
        (-1.0, 1.0),
        (0.0, 0.1),
        (0.5, 40.0),
        (40.0, 0.5),
        (1.0, -0.5),
    ] {
        assert_eq!(
            heights(below, above).validate(),
            Err(AreaError::Height { below, above })
        );
    }
    assert_eq!(heights(f32::NAN, 1.0).validate(), Err(AreaError::NotFinite));
}

#[test]
fn a_path_is_refused_by_its_first_fault() {
    let refused = |points: Vec<[f32; 2]>| Area::path(points).validate().unwrap_err();
    assert_eq!(refused(vec![[0.0, 0.0], [1.0, 0.0]]), AreaError::Points(2));
    let many = (0..33).map(|i| [i as f32, (i * i) as f32 * 0.01]).collect();
    assert_eq!(refused(many), AreaError::Points(33));
    assert_eq!(
        refused(vec![[0.0, 0.0], [100.0, 0.0], [0.0, 5.0]]),
        AreaError::Reach(1)
    );
    assert_eq!(
        refused(vec![[0.0, 0.0], [0.05, 0.0], [1.0, 1.0]]),
        AreaError::Edge(0)
    );
    assert_eq!(
        refused(vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]),
        AreaError::Flat
    );
    // A bow tie: the first edge crosses the third.
    assert_eq!(
        refused(vec![[0.0, 0.0], [4.0, 2.0], [4.0, 0.0], [0.0, 1.0]]),
        AreaError::Crosses(0, 2)
    );
    // A spike: the second edge folds back along the first.
    assert_eq!(
        refused(vec![[0.0, 0.0], [2.0, 0.0], [1.0, 0.0], [1.0, 1.0]]),
        AreaError::Crosses(0, 1)
    );
    assert_eq!(
        refused(vec![[0.0, 0.0], [f32::NAN, 0.0], [1.0, 1.0]]),
        AreaError::NotFinite
    );
    assert!(AreaError::Points(2).to_string().contains("3 to 32 points"));
    assert!(AreaError::Crosses(0, 2).to_string().contains("point 1"));
}

/// An L: a square with its far corner cut out.
fn ell() -> Area {
    Area::path(vec![
        [0.0, 0.0],
        [4.0, 0.0],
        [4.0, 2.0],
        [2.0, 2.0],
        [2.0, 4.0],
        [0.0, 4.0],
    ])
}

#[test]
fn a_concave_path_holds_what_is_in_it_and_not_its_notch() {
    let area = ell();
    assert_eq!(area.validate(), Ok(()));
    let centre = [10.0, 20.0, 5.0];
    let at = |x: f32, y: f32, z: f32| area.contains(centre, [10.0 + x, 20.0 + y, 5.0 + z]);
    assert!(at(1.0, 1.0, 0.0));
    assert!(at(3.0, 1.0, 0.0));
    assert!(at(1.0, 3.0, 0.0));
    // The notch, and outside altogether.
    assert!(!at(3.0, 3.0, 0.0));
    assert!(!at(5.0, 1.0, 0.0));
    assert!(!at(-0.5, 1.0, 0.0));
    // An edge and a corner are in.
    assert!(at(4.0, 1.0, 0.0));
    assert!(at(2.0, 2.0, 0.0));
    // The heights, edges included.
    assert!(at(1.0, 1.0, 1.5) && at(1.0, 1.0, -0.5));
    assert!(!at(1.0, 1.0, 1.75) && !at(1.0, 1.0, -0.75));
    let middle = area.middle(centre);
    assert!(near(middle, [10.0 + 2.0, 20.0 + 2.0, 5.5]), "{middle:?}");
}

#[test]
fn a_radius_area_holds_a_cylinder() {
    let area = Area::radius(2.0);
    let centre = [10.0, 10.0, 5.0];
    assert!(area.contains(centre, [11.9, 10.0, 5.0]));
    assert!(area.contains(centre, [12.0, 10.0, 5.0]));
    assert!(!area.contains(centre, [12.1, 10.0, 5.0]));
    assert!(!area.contains(centre, [11.5, 11.5, 5.0]));
    assert!(area.contains(centre, [10.0, 10.0, 6.5]));
    assert!(!area.contains(centre, [10.0, 10.0, 6.75]));
    assert!(!area.contains(centre, [10.0, 10.0, 4.25]));
    assert_eq!(area.middle(centre), [10.0, 10.0, 5.5]);
}

#[test]
fn an_outline_runs_counter_clockwise_whichever_way_it_was_dropped() {
    let floor = |points: &[[f32; 2]]| -> f32 {
        (0..points.len())
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                a[0] * b[1] - b[0] * a[1]
            })
            .sum()
    };
    let mut clockwise = square(1.0);
    clockwise.reverse();
    assert!(floor(&clockwise) < 0.0);
    assert!(floor(&Area::path(clockwise).outline()) > 0.0);
    assert_eq!(Area::path(square(1.0)).outline(), square(1.0));
    assert!(floor(&Area::radius(1.0).outline()) > 0.0);
}

#[test]
fn a_hills_corners_make_the_area_the_host_makes_of_them() {
    // Four corners and a flag inside them, the far corner a step up.
    let corners = [
        [0.0, 0.0, 1.0],
        [4.0, 0.0, 1.0],
        [4.0, 4.0, 2.0],
        [0.0, 4.0, 1.0],
        [2.0, 2.0, 1.5],
    ];
    let (centre, area) = Area::of_corners(&corners).unwrap();
    assert_eq!(centre, [2.0, 2.0, 1.0]);
    assert_eq!(area.shape, Shape::Path(square(2.0)));
    // Half a unit under the lowest corner, one and a half over the highest.
    assert_eq!((area.below, area.above), (0.5, 2.5));
    assert_eq!(area.validate(), Ok(()));
    assert!(area.contains(centre, [3.9, 3.9, 3.5]));
    assert!(area.contains(centre, [0.1, 0.1, 0.5]));
    assert!(!area.contains(centre, [4.1, 2.0, 1.0]));
    assert!(!area.contains(centre, [2.0, 2.0, 3.6]));
    // Two corners, or corners in a line, make no area.
    assert!(Area::of_corners(&corners[..2]).is_none());
    assert!(Area::of_corners(&[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]).is_none());
    assert_eq!(area::hull(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]).len(), 2);
}

#[test]
fn a_wall_is_the_boundary_alone() {
    // 105 lines for the default hill: its wall is 3k of them, k the
    // outline's points, with no ring and no post of the pip.
    let area = Area::default();
    let k = area.outline().len();
    let wall = Kind::wall(&area, [1.0; 3]);
    assert_eq!(wall.lines.len(), 3 * k);
    assert_eq!(wall.tris.len(), 2 * k);
    // The same box as the hill's mesh: the pip stays inside the wall.
    assert_eq!(wall.bounds(), Kind::Hill(area).mesh([1.0; 3]).bounds());
}
