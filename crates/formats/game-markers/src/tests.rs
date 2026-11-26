use crate::area::{self, Area, AreaError, Shape};
use crate::digits;
use crate::form::{self, Form, ICON_HALF, ICON_LIFT, PLATE, Part, WALL_HIGH, WALL_LOW};
use crate::icon::{self, ICON_FILL_ALPHA, Icon};
use crate::kind::{Item, Kind, Solid, SpawnUse, Volume};
use crate::mesh::{Mesh, Place, Vec3};
use crate::palette::{self, Palette};
use crate::{FILL_ALPHA, LIFT, LINE_ALPHA, Marker};

const EPS: f32 = 1e-4;

fn near(a: Vec3, b: Vec3) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= EPS)
}

/// The edit form in white, without the plate.
fn white(kind: &Kind) -> Mesh {
    kind.mesh_in(Form::Edit, [1.0; 3])
}

#[test]
fn every_kind_has_its_lines_faces_and_bounds() {
    // The edit form's lines and faces, the least corner and the
    // greatest, then the play form's lines and faces, in the order of
    // `Kind::samples`. The sums are in `Kind::build`'s comments: k = 32
    // points for a radius, a post every fourth, so a wall is 72 lines
    // and 64 faces.
    let hill = ([-1.0, -1.0, -0.5], [1.0, 1.0, 1.5]);
    let plate = ([-0.5, -0.5, LIFT], [0.5, 0.5, 1.0]);
    let cube = ([-1.0; 3], [1.0; 3]);
    let pad = |r: f32| ([-r, -r, LIFT], [r, r, 1.0]);
    #[rustfmt::skip]
    let expected: Vec<(&str, usize, usize, (Vec3, Vec3), usize, usize)> = vec![
        ("spawn", 39, 16, ([-0.35, -0.35, 0.0], [0.70, 0.35, 1.0]), 0, 0),
        ("spawn", 55, 16, ([-0.35, -0.35, 0.0], [0.70, 0.35, 1.0]), 0, 0),
        ("flag_stand", 22, 17, pad(0.3), 17, 0),
        ("flag_return", 105, 64, plate, 104, 64),
        ("bomb_spawn", 29, 24, pad(0.3), 16, 0),
        ("bomb_goal", 139, 64, plate, 138, 64),
        ("ball_spawn", 53, 16, pad(0.3), 16, 0),
        ("hill", 81, 64, hill, 72, 64),
        ("territory", 73, 64, hill, 73, 64),
        ("teleporter_sender", 68, 32, ([-0.4, -0.5, LIFT], [0.4, 0.5, 1.12]), 32, 0),
        ("teleporter_receiver", 71, 32, ([-0.4, -0.5, LIFT], [0.8, 0.5, 1.12]), 35, 0),
        ("weapon_spawn", 26, 16, pad(0.3), 0, 0),
        ("vehicle_spawn", 26, 16, ([-1.0, -1.0, LIFT], [1.4, 1.0, 1.0]), 0, 0),
        ("equipment_spawn", 29, 24, pad(0.3), 0, 0),
        ("trigger_volume", 12, 12, cube, 0, 0),
        ("kill_volume", 12, 12, cube, 0, 0),
        ("soft_volume", 12, 12, cube, 12, 12),
        ("bsp_switch", 12, 12, cube, 0, 0),
        ("trigger_volume", 96, 64, ([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]), 0, 0),
        ("trigger_volume", 96, 0, cube, 0, 0),
        ("ff_drop", 23, 16, ([-0.5, -0.5, LIFT], [0.9, 0.5, 1.3]), 0, 0),
        ("ff_base", 22, 3, pad(0.6), 0, 0),
        ("ff_cover", 24, 2, ([-0.25, -0.3, LIFT], [0.6, 0.3, 1.0]), 0, 0),
        ("siege_site", 73, 64, hill, 73, 64),
        ("siege_goal", 139, 64, plate, 138, 64),
        ("siege_line", 8, 2, ([0.0, -1.0, LIFT], [0.5, 1.0, 1.0]), 0, 0),
    ];
    let samples = Kind::samples();
    assert_eq!(samples.len(), expected.len());
    for (kind, (name, lines, tris, (min, max), play_lines, play_tris)) in
        samples.iter().zip(expected)
    {
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
        let play = kind.mesh_in(Form::Play, [1.0; 3]);
        assert_eq!(play.lines.len(), play_lines, "{name}: play lines");
        assert_eq!(play.tris.len(), play_tris, "{name}: play faces");
    }
}

#[test]
fn every_piece_of_a_play_form_is_a_piece_of_its_edit_form() {
    for kind in Kind::samples() {
        let name = kind.name();
        let (edit, play) = (white(&kind), kind.mesh_in(Form::Play, [1.0; 3]));
        // The same lines and faces at the same places: a piece, not a
        // copy near it.
        assert!(
            play.lines.iter().all(|l| edit.lines.contains(l)),
            "{name}: lines"
        );
        assert!(
            play.tris.iter().all(|t| edit.tris.contains(t)),
            "{name}: faces"
        );
        let bits = |p: Vec3| p.map(f32::to_bits);
        let points: Vec<_> = edit.points().map(bits).collect();
        assert!(
            play.points().all(|p| points.contains(&bits(p))),
            "{name}: vertices"
        );
        assert!(
            kind.parts(Form::Play).within(kind.parts(Form::Edit)),
            "{name}: parts"
        );
        // The edit-only pieces and the play form make the edit form.
        let rest = kind.edit_only([1.0; 3]);
        assert_eq!(
            rest.lines.len() + play.lines.len(),
            edit.lines.len(),
            "{name}"
        );
        assert_eq!(rest.tris.len() + play.tris.len(), edit.tris.len(), "{name}");
    }
}

#[test]
fn a_kind_with_no_play_form_gives_an_empty_mesh() {
    let mut none = Vec::new();
    for kind in Kind::samples() {
        let parts = kind.parts(Form::Play);
        let mesh = kind.mesh_in(Form::Play, [1.0; 3]);
        assert_eq!(parts.is_empty(), mesh.is_empty(), "{}", kind.name());
        if parts.is_empty() {
            none.push(kind.name());
        }
        // Every kind has an edit form with a label; none has a label in
        // play.
        assert!(kind.parts(Form::Edit).has(Part::Label), "{}", kind.name());
        assert!(!parts.has(Part::Label), "{}", kind.name());
    }
    assert_eq!(
        none,
        [
            "spawn",
            "spawn",
            "weapon_spawn",
            "vehicle_spawn",
            "equipment_spawn",
            "trigger_volume",
            "kill_volume",
            "bsp_switch",
            "trigger_volume",
            "trigger_volume",
            "ff_drop",
            "ff_base",
            "ff_cover",
            "siege_line",
        ]
    );
    // The parts each form shows, by the forms page's table.
    let parts = |kind: Kind, form| kind.parts(form).iter().collect::<Vec<_>>();
    assert_eq!(
        parts(Kind::Spawn(SpawnUse::Respawn), Form::Edit),
        [
            Part::Pad,
            Part::Body,
            Part::Post,
            Part::Plate,
            Part::Facing,
            Part::Label
        ]
    );
    assert_eq!(parts(Kind::FlagStand, Form::Play), [Part::Pad, Part::Body]);
    assert_eq!(
        parts(Kind::FlagReturn(Area::plate()), Form::Play),
        [Part::Body]
    );
    assert_eq!(
        parts(Kind::Hill(Area::default()), Form::Play),
        [Part::Boundary]
    );
    assert_eq!(
        parts(Kind::Territory(Area::default()), Form::Play),
        [Part::Post, Part::Plate, Part::Boundary]
    );
    assert_eq!(
        parts(Kind::TeleporterReceiver, Form::Play),
        [Part::Body, Part::Facing]
    );
    let cube = Solid::Box { half: [1.0; 3] };
    assert!(!parts(Kind::Volume(Volume::Kill, cube), Form::Edit).contains(&Part::Plate));
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
        let marker = Marker::new(Kind::Spawn(SpawnUse::Respawn), team);
        assert_eq!(marker.colour(&palette), colour);
        let mesh = marker.mesh_in(Form::Edit, &palette);
        let [r, g, b] = colour;
        assert!(mesh.lines.iter().all(|l| l.colour == [r, g, b, LINE_ALPHA]));
        assert!(
            mesh.tris
                .iter()
                .all(|t| t.colours == [[r, g, b, FILL_ALPHA]; 3])
        );
        // The plate is a skin at the same alpha; the icon's fills are
        // stronger, and in the same colour.
        let plate = marker.icon(&palette, None).unwrap();
        assert!(
            plate
                .lines
                .iter()
                .all(|l| l.colour == [r, g, b, LINE_ALPHA])
        );
        assert!(plate.tris.iter().all(|t| t.colours[0][..3] == [r, g, b]));
        // The whole marker is the edit form and the plate.
        let whole = marker.mesh(&palette);
        assert_eq!(whole.lines.len(), mesh.lines.len() + plate.lines.len());
        assert_eq!(whole.tris.len(), mesh.tris.len() + plate.tris.len());
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
    assert_eq!(colour(Kind::Goal(Area::plate()), Some(1)), palette::SIEGE);
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
    let mesh = white(&Kind::Spawn(SpawnUse::Respawn));
    let placed = mesh.placed(&Place::facing(
        [10.0, 20.0, 3.0],
        std::f32::consts::FRAC_PI_2,
    ));
    assert_eq!(placed.lines.len(), mesh.lines.len());
    assert_eq!(placed.tris.len(), mesh.tris.len());
    // The arrow that ran along x runs along y; the post stands a unit.
    let (low, high) = placed.bounds().unwrap();
    assert!(near(low, [9.65, 19.65, 3.0]), "{low:?}");
    assert!(near(high, [10.35, 20.70, 4.0]), "{high:?}");
    // No turn: the mesh moved.
    let moved = mesh.placed(&Place::facing([1.0, 2.0, 3.0], 0.0));
    let (low, high) = moved.bounds().unwrap();
    assert!(near(low, [0.65, 1.65, 3.0]) && near(high, [1.70, 2.35, 4.0]));
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
            .all(|t| t.colours == [[1.0, 1.0, 1.0, FILL_ALPHA * 0.5]; 3])
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
    assert!(Kind::Spawn(SpawnUse::Respawn).faces() && !Kind::Goal(Area::plate()).faces());
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
        // The foot, the top and a post at each corner, two faces an edge,
        // the pip's eight lines and the post to the plate.
        assert_eq!((mesh.lines.len(), mesh.tris.len()), (4 * 3 + 9, 4 * 2));
        let (low, high) = mesh.bounds().unwrap();
        assert!(near(low, [-2.0, -2.0, -0.5]) && near(high, [2.0, 2.0, 1.5]));
    }
    // A wider radius is a wider wall with the same lines.
    let mesh = white(&Kind::Hill(Area::radius(3.0)));
    assert_eq!((mesh.lines.len(), mesh.tris.len()), (81, 64));
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
    // For the default hill, k the outline's points: the foot and the top,
    // k lines each, and a post every fourth point; two faces an edge. No
    // pip, no post and no plate.
    let area = Area::default();
    let k = area.outline().len();
    let wall = Kind::wall(&area, [1.0; 3]);
    assert_eq!(wall.lines.len(), 2 * k + k / 4);
    assert_eq!(wall.tris.len(), 2 * k);
    // The same box as the hill's edit form: the pip and the post stay
    // inside the wall.
    assert_eq!(wall.bounds(), white(&Kind::Hill(area.clone())).bounds());
    // It is the hill's play form.
    assert_eq!(wall, Kind::Hill(area).mesh_in(Form::Play, [1.0; 3]));
}

#[test]
fn the_walls_alpha_falls_from_its_foot_to_its_top() {
    assert!(WALL_LOW > WALL_HIGH && WALL_HIGH > 0.0);
    let area = Area::default();
    let (low, high) = (-area.below, area.above);
    let wall = Kind::wall(&area, [0.2, 0.4, 0.6]);
    for t in &wall.tris {
        for (p, c) in t.points.iter().zip(t.colours) {
            assert_eq!(c[..3], [0.2, 0.4, 0.6]);
            let want = if (p[2] - low).abs() < EPS {
                WALL_LOW
            } else {
                assert!(
                    (p[2] - high).abs() < EPS,
                    "{p:?}: a corner off the foot and the top"
                );
                WALL_HIGH
            };
            assert_eq!(c[3], want, "{p:?}");
        }
    }
    let k = area.outline().len();
    // The foot at full strength, the top at a third, the posts full.
    let foot = &wall.lines[..k];
    assert!(
        foot.iter()
            .all(|l| l.colour[3] == LINE_ALPHA && l.a[2] == low && l.b[2] == low)
    );
    let top = &wall.lines[k..2 * k];
    assert!(
        top.iter()
            .all(|l| l.colour[3] == LINE_ALPHA / 3.0 && l.a[2] == high)
    );
    let posts = &wall.lines[2 * k..];
    assert_eq!(posts.len(), k / 4);
    assert!(
        posts
            .iter()
            .all(|l| l.a[2] == low && l.b[2] == high && l.colour[3] == LINE_ALPHA)
    );
    // A path has a post at every corner.
    let path = Kind::wall(&Area::path(square(1.0)), [1.0; 3]);
    assert_eq!((path.lines.len(), path.tris.len()), (12, 8));
}

#[test]
fn three_kinds_gain_an_area_with_the_plate_for_a_default() {
    let plate = Area::plate();
    assert_eq!(plate.shape, Shape::Radius(form::PLATE_RADIUS));
    assert_eq!((plate.below, plate.above), (0.1, 1.0));
    assert_eq!(plate.validate(), Ok(()));
    for label in ["ctf_flag_return", "assault_bomb_return", "siege_goal"] {
        let kind = Kind::of_label(label).unwrap();
        assert_eq!(kind.area(), Some(&plate), "{label}");
        // An area set on the flag is the one it draws.
        let wide = kind.clone().with_area(Area::radius(2.0));
        assert_eq!(wide.area(), Some(&Area::radius(2.0)), "{label}");
        assert_eq!(wide.name(), kind.name());
        let (low, high) = white(&wide).bounds().unwrap();
        assert!(
            near(low, [-2.0, -2.0, LIFT]) && near(high, [2.0, 2.0, 1.0]),
            "{label}"
        );
    }
    // A kind with no area is itself.
    assert_eq!(
        Kind::FlagStand.with_area(Area::radius(2.0)),
        Kind::FlagStand
    );
    let mut hill = Kind::Hill(Area::default());
    *hill.area_mut().unwrap() = Area::radius(3.0);
    assert_eq!(hill, Kind::Hill(Area::radius(3.0)));
    let mut spawn = Kind::BombSpawn;
    assert!(spawn.area_mut().is_none());
}

#[test]
fn a_capture_plate_is_its_areas_ring_an_inner_ring_and_a_low_rim() {
    let area = Area::radius(2.0);
    let play = Kind::FlagReturn(area.clone()).mesh_in(Form::Play, [1.0; 3]);
    let k = area.outline().len();
    let radius = |l: &crate::mesh::Line| l.a[0].hypot(l.a[1]);
    // A ring's lines on the floor, not a cross's through the middle.
    let on_floor = |l: &&crate::mesh::Line| {
        let through = (l.a[0] + l.b[0]).abs() < EPS && (l.a[1] + l.b[1]).abs() < EPS;
        l.a[2] == LIFT && l.b[2] == LIFT && !through
    };
    let rings: Vec<f32> = play.lines.iter().filter(on_floor).map(radius).collect();
    assert_eq!(rings.len(), 2 * k);
    assert_eq!(rings.iter().filter(|r| (**r - 2.0).abs() < EPS).count(), k);
    assert_eq!(rings.iter().filter(|r| (**r - 1.0).abs() < EPS).count(), k);
    let (_, high) = play.bounds().unwrap();
    assert!((high[2] - (LIFT + form::RIM)).abs() < EPS);
    // A plant zone: its ring, the target rings at 0.6 and 0.3 of it and a
    // cross out to the outer target ring.
    let play = Kind::BombGoal(area).mesh_in(Form::Play, [1.0; 3]);
    let rings: Vec<f32> = play.lines.iter().filter(on_floor).map(radius).collect();
    for r in [2.0, 1.2, 0.6] {
        assert_eq!(
            rings.iter().filter(|q| (**q - r).abs() < EPS).count(),
            k,
            "{r}"
        );
    }
    assert_eq!(rings.len(), 3 * k);
    let cross: Vec<_> = play
        .lines
        .iter()
        .filter(|l| near(l.a, [-l.b[0], -l.b[1], LIFT]))
        .collect();
    assert_eq!(cross.len(), 2);
    assert!(cross.iter().all(|l| (radius(l) - 1.2).abs() < EPS));
}

#[test]
fn a_plate_stands_on_its_post_over_the_body() {
    for kind in Kind::samples() {
        let Some(at) = kind.plate_at() else {
            assert!(matches!(kind, Kind::Volume(..)), "{}", kind.name());
            assert!(kind.icon().is_none() && kind.plate(None, [1.0; 3]).is_none());
            continue;
        };
        assert_eq!((at[0], at[1]), (0.0, 0.0), "{}", kind.name());
        assert!(at[2] >= form::POST + PLATE - EPS, "{}", kind.name());
        // The post runs from the floor to the plate's foot.
        let foot = [0.0, 0.0, at[2] - PLATE];
        assert!(
            white(&kind)
                .lines
                .iter()
                .any(|l| l.a == [0.0, 0.0, LIFT] && near(l.b, foot)),
            "{}: no post",
            kind.name()
        );
    }
    // The plate clears a body taller than the post.
    assert!(near(
        Kind::Drop.plate_at().unwrap(),
        [0.0, 0.0, 1.2 + form::PLATE_GAP + PLATE]
    ));
    assert!(near(
        Kind::FlagStand.plate_at().unwrap(),
        [0.0, 0.0, form::POST + PLATE]
    ));
    // `Kind::mesh` is the edit form with the plate over it.
    let kind = Kind::BombSpawn;
    let plate = kind.plate(None, [1.0; 3]).unwrap();
    let whole = kind.mesh([1.0; 3]);
    assert_eq!(
        whole.lines.len(),
        white(&kind).lines.len() + plate.lines.len()
    );
    let (_, high) = whole.bounds().unwrap();
    assert!(
        (high[2] - (form::POST + 2.0 * PLATE)).abs() < EPS,
        "{high:?}"
    );
}

#[test]
fn each_icon_lies_in_its_unit_square_and_no_two_are_alike() {
    let mut names: Vec<&str> = Icon::ALL.iter().map(|i| i.name()).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), Icon::ALL.len());
    let meshes: Vec<Mesh> = Icon::ALL.iter().map(|i| i.mesh([1.0; 3])).collect();
    for (icon, mesh) in Icon::ALL.iter().zip(&meshes) {
        let strokes = icon.strokes();
        assert!(!strokes.lines.is_empty(), "{icon:?}");
        for [u, v] in strokes.points() {
            assert!(
                (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v),
                "{icon:?}: {u}, {v}"
            );
        }
        // In the plate's space: in front of the plate, inside its rim.
        for p in mesh.points() {
            assert_eq!(p[0], ICON_LIFT, "{icon:?}");
            assert!(
                p[1].abs() <= ICON_HALF + EPS && p[2].abs() <= ICON_HALF + EPS,
                "{icon:?}"
            );
            assert!(p[1].hypot(p[2]) < PLATE, "{icon:?}");
        }
        assert!(mesh.lines.iter().all(|l| l.colour[3] == LINE_ALPHA));
        assert!(mesh.tris.iter().all(|t| t.colours[0][3] == ICON_FILL_ALPHA));
    }
    for i in 0..meshes.len() {
        for j in i + 1..meshes.len() {
            assert_ne!(
                meshes[i],
                meshes[j],
                "{:?} and {:?}",
                Icon::ALL[i],
                Icon::ALL[j]
            );
        }
    }
    // Every kind but a volume has one of them.
    for kind in Kind::samples() {
        assert_eq!(
            kind.icon().is_some_and(|i| Icon::ALL.contains(&i)),
            !matches!(kind, Kind::Volume(..)),
            "{}",
            kind.name()
        );
    }
    assert_eq!(Kind::Spawn(SpawnUse::Start).icon(), Some(Icon::Start));
    assert_eq!(Kind::Spawn(SpawnUse::Respawn).icon(), Some(Icon::Respawn));
}

#[test]
fn a_plate_holds_its_icon_and_a_hills_number_inside_its_rim() {
    let plate = icon::plate(Icon::Crown, None, [1.0; 3]);
    let crown = Icon::Crown.mesh([1.0; 3]);
    assert_eq!(plate.lines.len(), icon::PLATE_SEGMENTS + crown.lines.len());
    assert_eq!(plate.tris.len(), icon::PLATE_SEGMENTS + crown.tris.len());
    // Hill 12: a one of two segments and a two of five.
    let hill = Kind::Hill(Area::default());
    let numbered = hill.plate(Some(12), [1.0; 3]).unwrap();
    assert_eq!(numbered.lines.len(), plate.lines.len() + 7);
    assert_eq!(numbered.tris.len(), plate.tris.len());
    for p in numbered.points() {
        assert!(
            p[0] >= 0.0 && p[0] <= ICON_LIFT && p[1].hypot(p[2]) <= PLATE + EPS,
            "{p:?}"
        );
    }
    assert_ne!(numbered, hill.plate(Some(13), [1.0; 3]).unwrap());
    // A kind with no number ignores one.
    assert_eq!(
        Kind::FlagStand.plate(Some(3), [1.0; 3]),
        Kind::FlagStand.plate(None, [1.0; 3])
    );
    // A plate scaled for distance keeps its lines and grows round its
    // centre.
    let far = plate.scaled(form::icon_scale(16.0));
    assert_eq!(far.lines.len(), plate.lines.len());
    let (_, high) = far.bounds().unwrap();
    assert!((high[2] - 2.0 * PLATE).abs() < EPS);
}

#[test]
fn a_plate_keeps_its_size_near_and_grows_with_distance_past_it() {
    assert_eq!(form::icon_scale(0.0), 1.0);
    assert_eq!(form::icon_scale(form::ICON_NEAR), 1.0);
    assert_eq!(form::icon_scale(2.0 * form::ICON_NEAR), 2.0);
    assert_eq!(form::icon_scale(60.0), 7.5);
    assert_eq!(form::icon_scale(f32::NAN), 1.0);
    assert_eq!(form::icon_scale(f32::INFINITY), 1.0);
    for f in Form::ALL {
        assert_eq!(Form::of_name(f.name()), Some(f));
    }
    assert_eq!(Form::of_name("both"), None);
    assert_eq!(Form::default(), Form::Edit);
    assert_eq!(SpawnUse::default(), SpawnUse::Respawn);
    assert_eq!(SpawnUse::Start.name(), "start");
}

#[test]
fn stroke_digits_light_their_segments_and_no_two_numbers_match() {
    let counts: Vec<usize> = (0..10).map(|d| digits::digit(d).len()).collect();
    assert_eq!(counts, [6, 2, 5, 5, 4, 5, 6, 3, 7, 6]);
    assert_eq!(digits::digits_of(0), [0]);
    assert_eq!(digits::digits_of(207), [2, 0, 7]);
    // Ten: a one and a nought, 0.2 apart, inside the width.
    let ten = digits::number(10, [1.0, 2.0], 0.5);
    assert_eq!(ten.len(), 8);
    let width = digits::width(10) * 0.5;
    assert!((width - (2.0 * digits::WIDTH + digits::GAP) * 0.5).abs() < EPS);
    for [u, v] in ten.iter().flatten() {
        assert!(*u >= 1.0 - EPS && *u <= 1.0 + width + EPS, "{u}");
        assert!(*v >= 2.0 - EPS && *v <= 2.5 + EPS, "{v}");
    }
    let drawn: Vec<_> = (0..100)
        .map(|n| {
            let mut lines: Vec<[u32; 4]> = digits::number(n, [0.0; 2], 1.0)
                .iter()
                .map(|[a, b]| [a[0], a[1], b[0], b[1]].map(f32::to_bits))
                .collect();
            lines.sort();
            lines
        })
        .collect();
    for i in 0..drawn.len() {
        for j in i + 1..drawn.len() {
            assert_ne!(drawn[i], drawn[j], "{i} and {j}");
        }
    }
}
