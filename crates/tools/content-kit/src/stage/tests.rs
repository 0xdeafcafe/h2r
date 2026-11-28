//! The stage's own tests: no disc. The player's size is a stand-in; the
//! built map reads the real one from the base's biped.

use crate::stage::Error;
use crate::stage::layout::{Layout, Player};
use crate::stage::mesh::{Mesh, Surface};
use crate::stage::texture;

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
    assert_eq!(l.version, crate::stage::version());
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
    let text = crate::stage::SOURCE.replace("origin: [80, 0, 0]", "origin: [60, 0, 0]");
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
        (texture::texture_data().ruler.texels, texture::texture_data().ruler.texels)
    );
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

// The map, on an invented base: every tag the stage reads, made here.

mod base {
    use std::collections::HashMap;

    use ::map::{CacheType, FourCC, TagGroup};
    use tags::repo::{Body, LooseTag, Node, RawRes, Region, TagKey, TagSource, Tree, Value};

    use crate::stage::kit::{FIXED, LTMP, SBSP, SCNR, STEM, biped};
    use crate::stage::map::Base;

    pub const SCENARIO: &str = "scenarios\\multi\\test\\test";
    pub const HEIGHT: f32 = 0.62;
    pub const RADIUS: f32 = 0.21;
    /// The bipd's datum on the invented base, held by a script word.
    pub const BIPED_DATUM: u32 = 0xe175_0001;

    fn g(s: &[u8; 4]) -> FourCC {
        FourCC(*s)
    }

    fn put(bytes: &mut [u8], at: usize, v: u32) {
        bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    fn tag(key: TagKey, region: Region, nodes: Vec<Node>) -> LooseTag {
        LooseTag {
            key,
            body: Body::Tree(Tree {
                region,
                nodes,
                raws: Vec::new(),
                losses: Vec::new(),
            }),
        }
    }

    fn node(size: u32, count: u32, fields: Vec<(u32, Value)>) -> Node {
        Node {
            bytes: vec![0; (size * count) as usize],
            count,
            size,
            align: 4,
            fields,
        }
    }

    pub struct Fake {
        pub tags: Vec<LooseTag>,
    }

    impl TagSource for Fake {
        fn keys(&self) -> Vec<TagKey> {
            self.tags.iter().map(|t| t.key.clone()).collect()
        }
        fn read(&mut self, key: &TagKey) -> tags::repo::Result<Option<LooseTag>> {
            Ok(self.tags.iter().find(|t| &t.key == key).cloned())
        }
        fn raw(&mut self, raw: &RawRes) -> tags::repo::Result<Vec<u8>> {
            Err(tags::repo::Error::Invalid(format!("no raw {}", raw.hex())))
        }
        fn groups(&self) -> Vec<TagGroup> {
            let mut out: Vec<TagGroup> = Vec::new();
            for t in &self.tags {
                if !out.iter().any(|x| x.class == t.key.group) {
                    out.push(TagGroup {
                        class: t.key.group,
                        parent: None,
                        grandparent: None,
                    });
                }
            }
            out
        }
    }

    /// The globals, scenario, structure, lightmap and biped keys.
    pub fn keys() -> (TagKey, TagKey, TagKey, TagKey, TagKey) {
        (
            TagKey::new(g(b"matg"), "globals\\globals"),
            TagKey::new(SCNR, SCENARIO),
            TagKey::new(SBSP, SCENARIO),
            TagKey::new(LTMP, format!("{SCENARIO}_lightmap")),
            TagKey::new(g(b"bipd"), biped()),
        )
    }

    /// The invented base and its description.
    pub fn make() -> (Fake, Base) {
        let (globals, scenario, sbsp, ltmp, biped) = keys();
        let mut tags = Vec::new();

        // The globals: two global materials, the floor's second.
        let mut root = node(0x158, 1, vec![(0x150, Value::Block(1))]);
        put(&mut root.bytes, 0x150, 2);
        let list = node(
            0xb4,
            2,
            vec![
                (0, Value::String("hard_metal_thin".into())),
                (0xb4, Value::String(crate::stage::kit::floor_material().into())),
            ],
        );
        tags.push(tag(globals.clone(), Region::Metadata, vec![root, list]));

        // The player.
        let mut root = node(0x280, 1, Vec::new());
        root.bytes[0x268..0x26c].copy_from_slice(&HEIGHT.to_le_bytes());
        root.bytes[0x270..0x274].copy_from_slice(&RADIUS.to_le_bytes());
        tags.push(tag(biped.clone(), Region::Metadata, vec![root]));

        // The stems and the default bitmaps, each a 16 by 16 image.
        for stem in [h1convert::shad::TEX_BUMP.stem, h1convert::shad::ILLUM.stem] {
            tags.push(tag(
                TagKey::new(STEM, stem),
                Region::Metadata,
                vec![node(4, 1, Vec::new())],
            ));
        }
        for f in FIXED {
            let mut root = node(0x4c, 1, vec![(0x44, Value::Block(1))]);
            put(&mut root.bytes, 0x44, 1);
            let mut image = node(0x74, 1, Vec::new());
            image.bytes[4..6].copy_from_slice(&16u16.to_le_bytes());
            image.bytes[6..8].copy_from_slice(&16u16.to_le_bytes());
            tags.push(tag(
                TagKey::new(g(b"bitm"), f.path()),
                Region::Metadata,
                vec![root, image],
            ));
        }

        // The level the stage replaces.
        tags.push(tag(
            sbsp.clone(),
            Region::Bsp,
            vec![node(0x280, 1, Vec::new())],
        ));
        tags.push(tag(
            ltmp.clone(),
            Region::Bsp,
            vec![node(0x200, 1, Vec::new())],
        ));

        // The scenario: a scenery placement and its palette entry, a
        // starting location, the BSP descriptor, a script node holding the
        // biped's datum.
        let mut root = node(
            0x3e0,
            1,
            vec![
                (0x50, Value::Block(1)),
                (0x58, Value::Block(2)),
                (0x100, Value::Block(3)),
                (0x210, Value::Block(4)),
                (0x238, Value::Block(5)),
            ],
        );
        for at in [0x50, 0x58, 0x100, 0x210, 0x238] {
            put(&mut root.bytes, at, 1);
        }
        let scenery = node(0x5c, 1, Vec::new());
        let mut palette = node(
            0x28,
            1,
            vec![(
                4,
                Value::Ref(TagKey::new(g(b"scen"), "scenery\\rock\\rock")),
            )],
        );
        put(&mut palette.bytes, 0, u32::from_le_bytes(*b"scen"));
        let mut start = node(0x34, 1, Vec::new());
        start.bytes[0x2c..0x2e].copy_from_slice(&(-1i16).to_le_bytes());
        let mut desc = node(
            0x44,
            1,
            vec![
                (0, Value::Bsp),
                (0x14, Value::Ref(sbsp.clone())),
                (0x1c, Value::Ref(ltmp.clone())),
            ],
        );
        put(&mut desc.bytes, 0x10, SBSP.as_u32());
        put(&mut desc.bytes, 0x18, LTMP.as_u32());
        let mut script = node(0x14, 1, Vec::new());
        put(&mut script.bytes, 0x0c, BIPED_DATUM);
        tags.push(tag(
            scenario.clone(),
            Region::Metadata,
            vec![root, scenery, palette, start, desc, script],
        ));

        let keys: Vec<TagKey> = tags.iter().map(|t| t.key.clone()).collect();
        let datums = HashMap::from([(BIPED_DATUM, biped)]);
        let base = Base {
            map: "test".into(),
            cache_type: CacheType::Multiplayer,
            scenario,
            globals,
            keys,
            strings: Vec::new(),
            header: vec![0; 0x800],
            low_detail: None,
            index_word_14: 0,
            datums,
        };
        (Fake { tags }, base)
    }
}

fn project() -> crate::stage::map::Project {
    crate::stage::map::Project::builtin().expect("the shipped project parses")
}

#[test]
fn the_project_parses_and_refuses_what_it_cannot_build() {
    let p = project();
    assert_eq!((p.name.as_str(), p.mode.as_str()), ("stage", "fresh"));
    assert_eq!(p.kits, ["stage"]);
    for (from, to) in [
        ("mode = \"fresh\"", "mode = \"onto\""),
        ("kits = [\"stage\"]", "kits = [\"stage\", \"pogo\"]"),
        (
            "kits = [\"stage\"]",
            "kits = [\"stage\"]\nedits = [\"x\", \"x\"]",
        ),
        ("name = \"stage\"", "name = \"a stage\""),
    ] {
        let text = crate::stage::PROJECT.replace(from, to);
        assert_ne!(text, crate::stage::PROJECT, "{from}");
        assert!(
            matches!(crate::stage::map::Project::parse(&text), Err(Error::Project(_))),
            "{to}"
        );
    }
}

#[test]
fn the_facts_come_from_the_base() {
    let (mut src, base) = base::make();
    let facts = crate::stage::kit::Facts::read(&mut src, &base.globals).unwrap();
    let want = Player {
        height: base::HEIGHT,
        radius: base::RADIUS,
    };
    assert_eq!(facts.player, want);
    assert_eq!(facts.material, 1);
    assert_eq!(facts.fixed.len(), crate::stage::kit::FIXED.len());
    // A base without the floor's material is refused by name.
    let (mut src, base) = base::make();
    for t in &mut src.tags {
        if t.key == base.globals
            && let tags::repo::Body::Tree(tree) = &mut t.body
        {
            tree.nodes[1].fields.pop();
        }
    }
    match crate::stage::kit::Facts::read(&mut src, &base.globals) {
        Err(Error::Base(why)) => assert!(why.contains(crate::stage::kit::floor_material()), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_map_is_built_on_an_invented_base() {
    use scenario_edit::BlockKind;
    use scenario_edit::scenario::{Scenario, Value};

    let (mut src, base) = base::make();
    let p = project();
    let built = crate::stage::map::build(&mut src, &base, &p).expect("the stage builds");
    let r = &built.report;
    assert_eq!(r.bsps, 1, "one structure BSP");
    assert_eq!(r.scenario, p.scenario);
    assert_eq!(r.layout, built.layout);
    let want = Player {
        height: base::HEIGHT,
        radius: base::RADIUS,
    };
    assert_eq!(built.layout.player, want);
    let e = &r.scenario_edit;
    assert!(
        e.cleared.iter().any(|(n, c)| n == "scenery" && *c == 1),
        "{:?}",
        e.cleared
    );
    assert_eq!(e.palette_entries, 1);
    assert!(
        e.cleared.iter().any(|(n, _)| n == "skies"),
        "the base's skies are emptied for the stage's own: {:?}",
        e.cleared
    );
    assert!(r.own.iter().any(|k| k.ends_with(".sky ")), "{:?}", r.own);
    assert!(r.lightmap_side.is_power_of_two() && r.lightmap_density > 0.0);
    assert_eq!(e.datum_words, 1, "the script's biped datum");
    assert!(e.renamed >= 2, "the descriptor's two references");
    assert!(r.memory_size <= r.memory_max);

    // Read back as the game and the scenario editor read it.
    let mut m = ::map::Map::from_reader(std::io::Cursor::new(built.bytes.clone())).unwrap();
    assert_eq!(m.header().scenario_path, p.scenario);
    let named = |m: &::map::Map<_>, k: &tags::repo::TagKey| {
        m.tags()
            .iter()
            .any(|t| t.group == k.group && t.name.as_deref() == Some(k.path.as_str()))
    };
    for k in crate::stage::kit::Keys::new(&p.scenario).all() {
        assert!(named(&m, &k), "{k} is in the map");
    }
    let (_, old_scenario, old_sbsp, ..) = base::keys();
    for k in [old_scenario, old_sbsp] {
        assert!(!named(&m, &k), "{k} is not");
    }
    let s = Scenario::read(&mut m).expect("the scenario reads back");
    let mut places = Vec::new();
    for bay in &built.layout.bays {
        places.push(bay.spawn.point);
        places.extend(bay.cameras.iter().map(|c| c.eye));
    }
    assert_eq!(s.block(BlockKind::StartingLocations).len(), places.len());
    for (i, want) in places.iter().enumerate() {
        match s.get(BlockKind::StartingLocations, i, "position").unwrap() {
            Value::Vec3(got) => assert_eq!(&got, want, "starting location {i}"),
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(s.block(BlockKind::Scenery).len(), 0);

    // The same inputs, the same bytes.
    let (mut src, base) = base::make();
    let again = crate::stage::map::build(&mut src, &base, &p).unwrap();
    assert_eq!(again.bytes, built.bytes);
    assert_eq!(again.report.key, built.report.key);
}

#[test]
fn a_base_already_holding_the_stage_is_refused() {
    let (mut src, base) = base::make();
    let mut p = project();
    p.scenario = base::SCENARIO.into();
    assert!(matches!(
        crate::stage::map::build(&mut src, &base, &p),
        Err(Error::Base(_))
    ));
}

#[test]
fn the_package_is_refused_inside_a_git_checkout() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(crate::stage::map::git_checkout(here).is_some());
}

#[test]
fn the_subject_place_is_the_pad_facing_the_spawn() {
    let flat = crate::stage::subject_place("flat").unwrap();
    let day = crate::stage::subject_place("day").unwrap();
    assert_eq!(flat.bay, "flat");
    // The pad is 3 units ahead of the spawn, which faces +x; a thing on
    // it faces back, towards -x.
    assert_eq!(flat.point[..2], [3.0, 0.0]);
    assert!((flat.facing - std::f32::consts::PI).abs() < 1e-6);
    assert_eq!(day.point[0] - flat.point[0], 80.0);
    assert_eq!(layout().subject_place("flat").unwrap(), flat);
    assert!(matches!(
        crate::stage::subject_place("nowhere"),
        Err(Error::Layout(_))
    ));
}

// The lights, the lightmap and the sky.

#[test]
fn the_lights_parse_and_are_refused_when_wrong() {
    let l = layout();
    assert_eq!(l.lights, crate::stage::Lights::default());
    let yaml = serde_yaml_ng::to_string(&l).unwrap();
    assert!(yaml.contains("lights"), "the report carries the lights");
    let sun = l.lights.sun.direction();
    assert!((sun.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
    assert!(sun[2] > 0.0, "the sun is over the horizon");
    let with = |lights: &str| {
        format!(
            "lights: {lights}\n{}",
            small("  - { name: cube, at: [6, 3], box: { size: [1, 1, 1] } }")
        )
    };
    // The same text with good lights parses: the refusals below are the
    // lights'.
    let good = "{ sun: { azimuth: 90, elevation: 30, color: [1, 1, 1], power: 1 }, fill: [0.1, 0.1, 0.1], flat: 1, dark: 0.02 }";
    let l = Layout::parse(&with(good), PLAYER).expect("good lights parse");
    assert_eq!(l.lights.sun.azimuth, 90.0);
    for (what, lights) in [
        (
            "the sun on the horizon",
            "{ sun: { azimuth: 0, elevation: 0, color: [1, 1, 1], power: 1 }, fill: [0, 0, 0], flat: 1, dark: 0 }",
        ),
        (
            "a dark bay as bright as the flat one",
            "{ sun: { azimuth: 0, elevation: 30, color: [1, 1, 1], power: 1 }, fill: [0, 0, 0], flat: 0.5, dark: 0.5 }",
        ),
        (
            "a negative fill",
            "{ sun: { azimuth: 0, elevation: 30, color: [1, 1, 1], power: 1 }, fill: [-1, 0, 0], flat: 1, dark: 0 }",
        ),
        (
            "an unknown field",
            "{ sun: { azimuth: 0, elevation: 30, color: [1, 1, 1], power: 1 }, fill: [0, 0, 0], flat: 1, dark: 0, moon: 1 }",
        ),
    ] {
        assert!(
            matches!(
                Layout::parse(&with(lights), PLAYER),
                Err(Error::Layout(_) | Error::Yaml(_))
            ),
            "{what}"
        );
    }
}

/// The face a ray dropped on `(x, y)` lands on.
fn floor_at(mesh: &Mesh, x: f32, y: f32) -> usize {
    let (_, face) = mesh
        .first_hit([x, y, 20.0], [0.0, 0.0, -1.0])
        .expect("a floor");
    assert_eq!(mesh.faces[face].surface, Surface::Grid);
    face
}

#[test]
fn the_day_bay_is_brighter_on_the_suns_side_of_a_reference() {
    let l = layout();
    let mesh = Mesh::build(&l);
    let baked = crate::stage::light::bake(&l, &mesh).unwrap();
    let cube = l.bays[1]
        .references
        .iter()
        .find(|r| r.name == "cube")
        .unwrap();
    let b = cube.bounds;
    let centre = [(b[0] + b[1]) / 2.0, (b[2] + b[3]) / 2.0];
    let sun = l.lights.sun.direction();
    let run = (sun[0] * sun[0] + sun[1] * sun[1]).sqrt();
    let (hx, hy) = (sun[0] / run, sun[1] / run);
    // Past the cube's corner, towards and away from the sun, where the
    // cube's shadow (one unit long at 45 degrees) falls on the far side.
    let reach = ((b[1] - b[0]).hypot(b[3] - b[2])) / 2.0 + 0.35;
    let toward = [centre[0] + hx * reach, centre[1] + hy * reach];
    let away = [centre[0] - hx * reach, centre[1] - hy * reach];
    let lit = baked.sample(
        floor_at(&mesh, toward[0], toward[1]),
        [toward[0], toward[1], 0.0],
    );
    let shade = baked.sample(floor_at(&mesh, away[0], away[1]), [away[0], away[1], 0.0]);
    assert!(lit[0] > shade[0] + 0.3, "{lit:?} against {shade:?}");
    // In its shadow the floor has the sky fill and nothing else.
    for c in 0..3 {
        assert!((shade[c] - l.lights.fill[c]).abs() < 0.03, "{shade:?}");
    }
    // In the sun it has the fill and the sun's share.
    let want = l.lights.fill[0] + l.lights.sun.color[0] * l.lights.sun.power * sun[2];
    assert!((lit[0] - want).abs() < 0.03, "{lit:?} against {want}");
    // A face the sun cannot reach has the fill's share of it only: the
    // cube's side away from the sun.
    let side = mesh
        .faces
        .iter()
        .position(|f| {
            f.bay == 1
                && f.normal[2].abs() < 1e-3
                && f.points.iter().all(|p| {
                    p[0] >= b[0] - 1e-3
                        && p[0] <= b[1] + 1e-3
                        && p[1] >= b[2] - 1e-3
                        && p[1] <= b[3] + 1e-3
                })
                && f.normal[0] * sun[0] + f.normal[1] * sun[1] < -0.5
        })
        .expect("a side of the cube turned from the sun");
    let p = mesh.faces[side].points[0];
    let dark_side = baked.sample(side, [p[0], p[1], b[5] / 2.0]);
    assert!(dark_side[0] < lit[0] - 0.3, "{dark_side:?} against {lit:?}");
}

/// What the dark bay's light is under.
const DARK_BAR: f32 = 0.05;

#[test]
fn the_flat_bay_is_even_and_the_dark_bay_is_under_its_bar() {
    use crate::stage::layout::Light;
    let l = layout();
    let mesh = Mesh::build(&l);
    let baked = crate::stage::light::bake(&l, &mesh).unwrap();
    let mut seen = [0usize; 3];
    for (i, f) in mesh.faces.iter().enumerate() {
        for p in &f.points {
            let got = baked.sample(i, *p);
            match l.bays[f.bay].light {
                Light::Flat => {
                    seen[0] += 1;
                    for c in got {
                        assert!(
                            (c - l.lights.flat).abs() < 1.0 / crate::stage::light::light_data().half,
                            "{got:?}"
                        );
                    }
                }
                Light::Dark => {
                    seen[1] += 1;
                    assert!(got.iter().all(|c| *c < DARK_BAR), "{got:?}");
                }
                Light::Day => seen[2] += 1,
            }
        }
    }
    assert!(seen.iter().all(|n| *n > 0), "{seen:?}");
    // The flat bay's faces share one cell and the dark bay's another: no
    // texels spent on them.
    let flat: std::collections::HashSet<_> = mesh
        .faces
        .iter()
        .enumerate()
        .filter(|(_, f)| l.bays[f.bay].light == Light::Flat)
        .map(|(i, _)| baked.cell(i).0)
        .collect();
    assert_eq!(flat.len(), 1);
    // The day floor has both lit and shaded texels.
    let floor = floor_at(&mesh, l.bays[1].origin[0], l.bays[1].origin[1]);
    let (cell, constant) = baked.cell(floor);
    assert!(!constant);
    let mut values = std::collections::HashSet::new();
    for y in cell.y..cell.y + cell.h {
        for x in cell.x..cell.x + cell.w {
            values.insert(baked.page.get(x, y));
        }
    }
    assert!(
        values.len() >= 2,
        "{} values on the day floor",
        values.len()
    );
}

#[test]
fn the_lightmap_cells_fit_the_page_and_the_vertices_read_their_own() {
    let l = layout();
    let mesh = Mesh::build(&l);
    let baked = crate::stage::light::bake(&l, &mesh).unwrap();
    let side = baked.side();
    assert!(
        side.is_power_of_two() && side <= crate::stage::light::light_data().max_page,
        "{side}"
    );
    assert_eq!(baked.page.height, side);
    let mut cells = Vec::new();
    for i in 0..mesh.faces.len() {
        let (c, _) = baked.cell(i);
        assert!(c.x + c.w <= side && c.y + c.h <= side, "{c:?}");
        if !cells.contains(&c) {
            cells.push(c);
        }
        // Each corner reads inside its own cell, texel centres only.
        for p in &mesh.faces[i].points {
            let [u, v] = baked.uv(i, *p);
            let (x, y) = (u * side as f32, v * side as f32);
            assert!(
                x >= c.x as f32
                    && x <= (c.x + c.w) as f32
                    && y >= c.y as f32
                    && y <= (c.y + c.h) as f32,
                "face {i}: ({x}, {y}) is outside {c:?}"
            );
        }
    }
    for (k, a) in cells.iter().enumerate() {
        for b in &cells[k + 1..] {
            let apart = a.x + a.w < b.x || b.x + b.w < a.x || a.y + a.h < b.y || b.y + b.h < a.y;
            assert!(apart, "{a:?} touches {b:?}");
        }
    }
    // Lit from the sun on a day face that looks at it; by its own normal
    // elsewhere.
    let sun = l.lights.sun.direction();
    let up = floor_at(&mesh, l.bays[1].origin[0], l.bays[1].origin[1]);
    assert_eq!(baked.incident(up, [0.0, 0.0, 1.0]), sun);
    let flat = floor_at(&mesh, 0.0, 0.0);
    assert_eq!(baked.incident(flat, [0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]);
}

#[test]
fn the_lightmap_is_the_same_twice() {
    let l = layout();
    let mesh = Mesh::build(&l);
    let a = crate::stage::light::bake(&l, &mesh).unwrap();
    let b = crate::stage::light::bake(&l, &mesh).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.texels().len(), a.page.pixels.len());
}

#[test]
fn the_sky_dome_faces_in_and_converts() {
    use crate::stage::sky;
    let tris = sky::triangles();
    assert!(!tris.is_empty());
    let model = sky::dome();
    let part = &model.geometries[0].parts[0];
    assert!(part.vertices.len() < 0xffff);
    // The strip reads back as the triangles, every one wound counter-
    // clockwise seen from the camera at the dome's centre.
    assert_eq!(sky::model_triangles(&model), tris);
    for t in &tris {
        let p: Vec<[f32; 3]> = t
            .iter()
            .map(|&k| part.vertices[k as usize].position)
            .collect();
        let (a, b) = (
            [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
            [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
        );
        let n = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let c = [
            p[0][0] + p[1][0] + p[2][0],
            p[0][1] + p[1][1] + p[2][1],
            p[0][2] + p[1][2] + p[2][2],
        ];
        assert!(
            n[0] * c[0] + n[1] * c[1] + n[2] * c[2] < 0.0,
            "{t:?} faces out"
        );
    }
    // Every vertex is one radius out, and its normal points back in.
    for v in &part.vertices {
        let r = v.position.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((r - sky::sky_data().radius).abs() < 0.01, "{r}");
        let n: f32 = (0..3).map(|a| v.normal[a] * v.position[a]).sum();
        assert!(n < 0.0);
    }
    // The picture's columns run with the dome's azimuth.
    let (az, el) = sky::angles(0.5, 0.25);
    assert_eq!((az, el), (0.0, 45.0));
    let (x, y) = sky::pixel(az, el);
    assert_eq!((x, y), (sky::sky_data().width as f32 / 2.0, sky::sky_data().height as f32 / 4.0));
    // It converts to a `mode` naming the one shader.
    let converted = sky::convert(map::TagId(0xe174_0001)).expect("the dome converts");
    assert_eq!(converted.group, map::FourCC(*b"mode"));
    assert!(!converted.raw.is_empty());
}

#[test]
fn the_sky_picture_marks_up_north_and_the_sun() {
    use crate::stage::sky;
    let l = layout();
    let img = sky::picture(&l);
    assert_eq!((img.width, img.height), (sky::sky_data().width, sky::sky_data().height));
    let bright = |c: [u8; 4]| u32::from(c[0]) + u32::from(c[1]) + u32::from(c[2]);
    // Up is blue and the horizon pale; below it is dark grey.
    let top = img.get(10, 0);
    let near = img.get(10, sky::sky_data().height / 2 - 6);
    let under = img.get(10, sky::sky_data().height / 2 + 20);
    assert!(top[2] > top[0] + 80, "{top:?}");
    assert!(bright(near) > bright(top), "{near:?} against {top:?}");
    assert!(bright(under) < bright(near) / 2, "{under:?}");
    // A dark line on the horizon itself.
    assert!(bright(img.get(10, sky::sky_data().height / 2)) < 90);
    // The sun's disc is where the layout puts it, and bright.
    let s = l.lights.sun;
    let (x, y) = sky::pixel(s.azimuth, s.elevation);
    let disc = img.get(x as u32, y as u32);
    assert!(disc.iter().take(3).all(|c| *c > 200), "{disc:?}");
    // The four letters are where the compass puts them, and nothing else
    // on the horizon band is ink.
    let ink = |az: f32| {
        let (cx, cy) = sky::pixel(az, sky::sky_data().letter_elevation);
        let r = (3 * sky::sky_data().letter_scale) as i64;
        let mut n = 0;
        for dy in -r..=r {
            for dx in -r..=r {
                let x = (cx as i64 + dx).rem_euclid(i64::from(sky::sky_data().width)) as u32;
                let y = (cy as i64 + dy) as u32;
                n += usize::from(img.get(x, y) == sky::sky_data().colors.ink);
            }
        }
        n
    };
    for crate::stage::data::Compass { letter: ch, azimuth: az } in sky::sky_data().compass.iter().copied() {
        assert!(ink(az) > 200, "no {ch} at {az}: {}", ink(az));
    }
    assert_eq!(ink(45.0), 0);
    assert_eq!(ink(-135.0), 0);
}

#[test]
fn the_scenario_names_the_sky() {
    use ::map::build::{Block, FixupKind};
    let mut root = Block::zeroed(0x40);
    // A skies block already there is replaced, not added to.
    root.block(crate::stage::sky::SKIES, 3, Block::zeroed(24));
    root.tag_ref(0x20, ::map::TagId::NULL);
    let sky = ::map::TagId(0xe174_0042);
    crate::stage::sky::name_sky(&mut root, sky);
    let blocks: Vec<_> = root
        .fixups
        .iter()
        .filter(|f| f.offset == crate::stage::sky::SKIES)
        .collect();
    assert_eq!(blocks.len(), 1);
    let FixupKind::Block { count, child } = &blocks[0].kind else {
        panic!("not a block");
    };
    assert_eq!(*count, 1);
    assert_eq!(child.bytes.len(), 8);
    assert_eq!(child.fixups.len(), 1);
    assert_eq!(child.fixups[0].offset, 0);
    assert_eq!(child.fixups[0].kind, FixupKind::TagRef(sky));
    // What the block did not hold is left alone.
    assert!(root.fixups.iter().any(|f| f.offset == 0x20));
}

#[test]
fn a_base_without_the_self_illuminated_stem_is_refused_by_name() {
    let (mut src, base) = base::make();
    src.tags.retain(|t| {
        !(t.key.group == crate::stage::kit::STEM && t.key.path == h1convert::shad::ILLUM.stem)
    });
    match crate::stage::kit::Facts::read(&mut src, &base.globals) {
        Err(Error::Base(why)) => assert!(why.contains(h1convert::shad::ILLUM.stem), "{why}"),
        other => panic!("{other:?}"),
    }
}

mod manifest {
    use crate::stage::data::{StageData, data};

    /// Every value `content/stage/kit.yaml` holds is the constant the
    /// builders had before it was data, so the built tags are the same bytes.
    #[test]
    fn manifest_is_the_retired_constants() {
        let d = data();
        assert_eq!(d.item, "stage");
        assert_eq!(d.version, 2);
        assert_eq!(d.biped, "objects\\characters\\masterchief\\masterchief_mp");
        assert_eq!(d.floor_material, "hard_terrain_stone");
        assert_eq!(d.salt, 0xe174);
        assert_eq!(crate::stage::version(), 2);

        let s = &d.sky;
        assert_eq!((s.radius, s.segments, s.rings), (1000.0, 48, 24));
        assert_eq!((s.width, s.height), (1024, 512));
        let compass: Vec<(char, f32)> = s.compass.iter().map(|c| (c.letter, c.azimuth)).collect();
        assert_eq!(compass, [('N', 0.0), ('W', 90.0), ('S', 180.0), ('E', -90.0)]);
        assert_eq!((s.letter_elevation, s.letter_scale), (16.0, 6));
        assert_eq!((s.disc, s.glow, s.line_half, s.light_cutoff), (3.0, 15.0, 0.4, 0.035));
        let c = &s.colors;
        assert_eq!(c.zenith, [38, 92, 200, 0xff]);
        assert_eq!(c.horizon, [190, 215, 245, 0xff]);
        assert_eq!(c.ground_near, [78, 80, 86, 0xff]);
        assert_eq!(c.ground_far, [34, 36, 40, 0xff]);
        assert_eq!(c.line, [20, 20, 24, 0xff]);
        assert_eq!(c.sun, [255, 246, 214, 0xff]);
        assert_eq!(c.plate, [236, 236, 236, 0xff]);
        assert_eq!(c.ink, [16, 16, 16, 0xff]);

        let l = &d.light;
        assert_eq!((l.texels_per_unit, l.min_density, l.max_page), (4.0, 0.5, 2048));
        assert_eq!((l.gap, l.constant_side, l.half, l.bias), (1, 2, 128.0, 0.01));

        let t = &d.texture;
        assert_eq!(t.scale, 2);
        let c = &t.colors;
        assert_eq!(c.check_light, [0xb4, 0xb4, 0xb4, 0xff]);
        assert_eq!(c.check_dark, [0xa4, 0xa4, 0xa4, 0xff]);
        assert_eq!(c.line, [0x80, 0x80, 0x80, 0xff]);
        assert_eq!(c.heavy, [0x50, 0x50, 0x50, 0xff]);
        assert_eq!(c.axis_x, [0xc0, 0x30, 0x30, 0xff]);
        assert_eq!(c.axis_y, [0x30, 0xa0, 0x30, 0xff]);
        assert_eq!(c.subject, [0xe8, 0x80, 0x20, 0xff]);
        assert_eq!(c.pad, [0x30, 0x60, 0xd0, 0xff]);
        assert_eq!(c.plate, [0xec, 0xec, 0xec, 0xff]);
        assert_eq!(c.ink, [0x10, 0x10, 0x10, 0xff]);
        assert_eq!(t.ruler.texels, 256);
        assert_eq!(t.ruler.base, [0xd8, 0xc8, 0x60, 0xff]);
        assert_eq!(t.ruler.tick, [0x40, 0x40, 0x40, 0xff]);
        assert_eq!(t.ruler.edge, [0x18, 0x18, 0x18, 0xff]);
    }

    #[test]
    fn an_unknown_or_missing_key_is_refused() {
        let text = super::super::data::SOURCE;
        let extra = format!("{text}\nmystery: 1\n");
        assert!(StageData::parse(&extra).is_err(), "an unknown key");
        let nested = text.replacen("  bias: 0.01", "  bias: 0.01\n  extra: 1", 1);
        assert_ne!(nested, text);
        assert!(StageData::parse(&nested).is_err(), "an unknown nested key");
        let missing = text.replacen("floor_material: hard_terrain_stone\n", "", 1);
        assert_ne!(missing, text);
        assert!(StageData::parse(&missing).is_err(), "a missing key");
        let wrong = text.replacen("item: stage", "item: other", 1);
        assert!(StageData::parse(&wrong).is_err(), "another item");
    }
}
