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

// The map, on an invented base: every tag the stage reads, made here.

mod base {
    use std::collections::HashMap;

    use ::map::{CacheType, FourCC, TagGroup};
    use tags::repo::{Body, LooseTag, Node, RawRes, Region, TagKey, TagSource, Tree, Value};

    use crate::kit::{BIPED, FIXED, LTMP, SBSP, SCNR, STEM};
    use crate::map::Base;

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
            TagKey::new(g(b"bipd"), BIPED),
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
                (0xb4, Value::String(crate::kit::FLOOR_MATERIAL.into())),
            ],
        );
        tags.push(tag(globals.clone(), Region::Metadata, vec![root, list]));

        // The player.
        let mut root = node(0x280, 1, Vec::new());
        root.bytes[0x268..0x26c].copy_from_slice(&HEIGHT.to_le_bytes());
        root.bytes[0x270..0x274].copy_from_slice(&RADIUS.to_le_bytes());
        tags.push(tag(biped.clone(), Region::Metadata, vec![root]));

        // The stem and the default bitmaps, each a 16 by 16 image.
        tags.push(tag(
            TagKey::new(STEM, h1convert::shad::TEX_BUMP.stem),
            Region::Metadata,
            vec![node(4, 1, Vec::new())],
        ));
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

fn project() -> crate::map::Project {
    crate::map::Project::builtin().expect("the shipped project parses")
}

#[test]
fn the_project_parses_and_refuses_what_it_cannot_build() {
    let p = project();
    assert_eq!((p.name.as_str(), p.mode.as_str()), ("stage", "fresh"));
    assert_eq!(p.kits, ["stage"]);
    for (from, to) in [
        ("mode = \"fresh\"", "mode = \"onto\""),
        ("kits = [\"stage\"]", "kits = [\"stage\", \"pogo\"]"),
        ("output = \"package\"", "output = \"map\""),
        ("name = \"stage\"", "name = \"a stage\""),
    ] {
        let text = crate::PROJECT.replace(from, to);
        assert_ne!(text, crate::PROJECT, "{from}");
        assert!(
            matches!(crate::map::Project::parse(&text), Err(Error::Project(_))),
            "{to}"
        );
    }
}

#[test]
fn the_facts_come_from_the_base() {
    let (mut src, base) = base::make();
    let facts = crate::kit::Facts::read(&mut src, &base.globals).unwrap();
    let want = Player {
        height: base::HEIGHT,
        radius: base::RADIUS,
    };
    assert_eq!(facts.player, want);
    assert_eq!(facts.material, 1);
    assert_eq!(facts.fixed.len(), crate::kit::FIXED.len());
    // A base without the floor's material is refused by name.
    let (mut src, base) = base::make();
    for t in &mut src.tags {
        if t.key == base.globals
            && let tags::repo::Body::Tree(tree) = &mut t.body
        {
            tree.nodes[1].fields.pop();
        }
    }
    match crate::kit::Facts::read(&mut src, &base.globals) {
        Err(Error::Base(why)) => assert!(why.contains(crate::kit::FLOOR_MATERIAL), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_map_is_built_on_an_invented_base() {
    use scenario_edit::BlockKind;
    use scenario_edit::scenario::{Scenario, Value};

    let (mut src, base) = base::make();
    let p = project();
    let built = crate::map::build(&mut src, &base, &p).expect("the stage builds");
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
    for k in crate::kit::Keys::new(&p.scenario).all() {
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
    let again = crate::map::build(&mut src, &base, &p).unwrap();
    assert_eq!(again.bytes, built.bytes);
    assert_eq!(again.report.key, built.report.key);
}

#[test]
fn a_base_already_holding_the_stage_is_refused() {
    let (mut src, base) = base::make();
    let mut p = project();
    p.scenario = base::SCENARIO.into();
    assert!(matches!(
        crate::map::build(&mut src, &base, &p),
        Err(Error::Base(_))
    ));
}

#[test]
fn the_package_is_refused_inside_a_git_checkout() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(crate::map::git_checkout(here).is_some());
}
