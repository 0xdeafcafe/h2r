//! The stage's own tests: no disc. The player's size is a stand-in; the
//! built map reads the real one from the base's biped.

use crate::Error;
use crate::layout::{Layout, Player};
use crate::mesh::{Mesh, Surface};
use crate::texture;

const PLAYER: Player = Player {
    height: 0.7,
    radius: 0.2,
};

fn layout() -> Layout {
    Layout::builtin(PLAYER).expect("the shipped layout parses")
}

/// A one-bay layout with `references` as its reference list.
fn small(references: &str) -> String {
    format!(
        "floor: {{ size: 16, label_every: 5, texels: 256 }}
bays:
  - {{ name: only, origin: [0, 0, 0], light: flat }}
spawn: {{ at: [0, 0], facing: 0 }}
subject: {{ at: [3, 0], radius: 1 }}
pads: {{ first: [-4, -6], step: [2, 0], count: 3, radius: 0.5 }}
references:
{references}
cameras:
  fov: 70
  aim: 0.3
  eyes: [{{ name: front, at: [2.5, 0, 0.5] }}]
  orbit: {{ count: 4, radius: 2, height: 0.6 }}
"
    )
}

#[test]
fn the_layout_parses() {
    let l = layout();
    assert_eq!(l.version, crate::VERSION);
    let names: Vec<_> = l.bays.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["flat", "day", "dark"]);
    let flat = &l.bays[0];
    assert_eq!(flat.pads.len(), 8);
    assert_eq!(flat.cameras.len(), 5 + 8);
    // Sizes named after the player take the player's.
    let pillar = flat.references.iter().find(|r| r.name == "pillar").unwrap();
    assert!((pillar.bounds[5] - PLAYER.height).abs() < 1e-6);
    // Every bay has the same furniture, moved to its origin.
    let day = &l.bays[1];
    assert_eq!(day.subject.point[0] - flat.subject.point[0], 80.0);
    assert_eq!(l.cameras("ground").count(), 3);
    // The layout is the report's `layout`.
    let yaml = serde_yaml_ng::to_string(&l).unwrap();
    assert!(yaml.contains("subject"), "{yaml}");
}

#[test]
fn a_small_layout_parses() {
    let text = small("  - { name: cube, at: [6, 3], box: { size: [1, 1, 1] } }");
    let l = Layout::parse(&text, PLAYER).unwrap();
    assert_eq!(l.bays[0].references.len(), 1);
}

#[test]
fn overlaps_are_refused() {
    // A cube on the subject pad.
    let text = small("  - { name: cube, at: [3, 0.5], box: { size: [1, 1, 1] } }");
    match Layout::parse(&text, PLAYER) {
        Err(Error::Overlap { bay, a, b }) => {
            assert_eq!(bay, "only");
            assert_eq!(a, "reference cube");
            assert_eq!(b, "the subject pad");
        }
        other => panic!("{other:?}"),
    }
    // Two references on one spot.
    let text = small(
        "  - { name: a, at: [6, 3], box: { size: [1, 1, 1] } }
  - { name: b, at: [6.5, 3], box: { size: [1, 1, 1] } }",
    );
    assert!(matches!(
        Layout::parse(&text, PLAYER),
        Err(Error::Overlap { .. })
    ));
    // Touching is not overlapping.
    let text = small(
        "  - { name: a, at: [6, 3], box: { size: [1, 1, 1] } }
  - { name: b, at: [7, 3], box: { size: [1, 1, 1] } }",
    );
    Layout::parse(&text, PLAYER).unwrap();
    // Off the floor.
    let text = small("  - { name: a, at: [7.8, 3], box: { size: [1, 1, 1] } }");
    assert!(matches!(
        Layout::parse(&text, PLAYER),
        Err(Error::Layout(_))
    ));
    // A camera inside a reference.
    let text = small("  - { name: a, at: [5.5, 0], box: { size: [1, 1, 2] } }");
    assert!(matches!(
        Layout::parse(&text, PLAYER),
        Err(Error::Layout(_))
    ));
    // A shape that is not one shape.
    let text = small("  - { name: a, at: [6, 3] }");
    assert!(matches!(
        Layout::parse(&text, PLAYER),
        Err(Error::Layout(_))
    ));
    // A size that is not a length.
    let text = small("  - { name: a, at: [6, 3], box: { size: [1, 1, tall] } }");
    assert!(matches!(
        Layout::parse(&text, PLAYER),
        Err(Error::Layout(_))
    ));
    // An unknown key.
    let text = small("  - { name: a, at: [6, 3], colour: red, box: { size: [1, 1, 1] } }");
    assert!(matches!(Layout::parse(&text, PLAYER), Err(Error::Yaml(_))));
}

#[test]
fn bays_do_not_meet() {
    let text = crate::SOURCE.replace("origin: [80, 0, 0]", "origin: [60, 0, 0]");
    assert!(matches!(
        Layout::parse(&text, PLAYER),
        Err(Error::Overlap { .. })
    ));
}

/// Every camera looks at its subject pad: the pad's centre is in front of
/// it, inside its field of view, and no face is between them; nor between
/// it and any corner of the pad.
#[test]
fn every_camera_sees_its_subject_pad() {
    let l = layout();
    let mesh = Mesh::build(&l);
    for bay in &l.bays {
        let s = bay.subject;
        let lift = 0.02;
        let centre = [s.point[0], s.point[1], s.point[2] + lift];
        let mut points = vec![centre];
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            // Just inside the corner.
            let r = s.radius * 0.95;
            points.push([s.point[0] + dx * r, s.point[1] + dy * r, s.point[2] + lift]);
        }
        for cam in &bay.cameras {
            let what = format!("bay {} camera {}", bay.name, cam.name);
            for (i, p) in points.iter().enumerate() {
                let d = [p[0] - cam.eye[0], p[1] - cam.eye[1], p[2] - cam.eye[2]];
                let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let dir = [d[0] / dist, d[1] / dist, d[2] / dist];
                if let Some((t, face)) = mesh.first_hit(cam.eye, dir) {
                    assert!(
                        t >= dist - 1e-3,
                        "{what}: face {face} ({:?}) is {t} away, before pad point {i} at {dist}",
                        mesh.faces[face].surface
                    );
                }
                if i == 0 {
                    let f = cam.forward();
                    let cos = f[0] * dir[0] + f[1] * dir[1] + f[2] * dir[2];
                    // Inside the narrower, vertical half-angle at 16:9.
                    let half = ((cam.fov.to_radians() / 2.0).tan() * 9.0 / 16.0).atan();
                    assert!(cos >= half.cos(), "{what}: the pad is out of view");
                }
            }
        }
    }
}

#[test]
fn the_collision_compiles() {
    let l = layout();
    let mesh = Mesh::build(&l);
    let compiled = mesh
        .collision()
        .expect("the stage's collision compiles and verifies");
    assert!(compiled.report.dropped.is_empty(), "{:?}", compiled.report);
    assert!(!compiled.bsp.surfaces.is_empty());
    // Both surfaces are used: the floors and everything else.
    for s in Surface::ALL {
        assert!(mesh.faces.iter().any(|f| f.surface == s), "{s:?}");
    }
    // A face's winding agrees with its normal: up on the floors.
    for f in mesh.faces.iter().filter(|f| f.surface == Surface::Grid) {
        assert!(f.normal[2] > 0.99, "{:?}", f.normal);
    }
    // Something falling onto a bay lands on its floor.
    for bay in &l.bays {
        let o = bay.origin;
        let (t, face) = mesh
            .first_hit([o[0] - 10.0, o[1] - 10.0, o[2] + 5.0], [0.0, 0.0, -1.0])
            .expect("the floor");
        assert!((t - 5.0).abs() < 1e-4 && mesh.faces[face].surface == Surface::Grid);
    }
}

#[test]
fn the_grids_labels_read_back() {
    let l = layout();
    let img = texture::grid(&l);
    assert_eq!((img.width, img.height), (l.floor.texels, l.floor.texels));
    let labels = texture::labels(&l);
    for want in ["0", "5", "-5", "30", "-30", "1", "8"] {
        assert!(labels.iter().any(|x| x.text == want), "no label {want}");
    }
    for label in &labels {
        let got = texture::read_label(&img, label.column, label.row, label.text.chars().count());
        assert_eq!(got.as_deref(), Some(label.text.as_str()), "{label:?}");
    }
    // A cell with no ink is no glyph.
    assert_eq!(texture::read_label(&img, 0, 0, 1), None);
}

#[test]
fn the_textures_are_the_right_size() {
    let r = texture::ruler();
    assert_eq!(
        (r.width, r.height),
        (texture::RULER_TEXELS, texture::RULER_TEXELS)
    );
    let m = texture::lightmap();
    assert!(m.pixels.chunks(4).all(|p| p == [0x80, 0x80, 0x80, 0xff]));
}

#[test]
fn the_build_is_the_same_twice() {
    let a = layout();
    let b = layout();
    assert_eq!(a, b);
    let (ma, mb) = (Mesh::build(&a), Mesh::build(&b));
    assert_eq!(ma.to_bytes(), mb.to_bytes());
    assert_eq!(texture::grid(&a).pixels, texture::grid(&b).pixels);
    assert_eq!(texture::ruler().pixels, texture::ruler().pixels);
    assert_eq!(ma.collision().unwrap().bsp, mb.collision().unwrap().bsp);
}
