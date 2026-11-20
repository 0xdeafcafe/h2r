//! Every animation of every converted Blood Gulch graph reads as the
//! engine's codec functions read it, and so does every retail graph on
//! Coagulation, which is the evidence for the rules. Reads `$H1_ISO` and
//! `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it animation_codec_disc:: -- --ignored --nocapture
//! ```
//!
//! The rules are `tags::anim::AnimationGraph::engine_problems`: the clip
//! builds and samples at every frame (codec, channel counts, node count,
//! every read inside its stream), the blend screen index is in range, and
//! a codec 8 (blend screen) stream and its rotation stride are 16-byte
//! aligned in the resource, since the engine reads its frames with
//! `movaps` (`0x0028c589`, in codec 8's interpolated rotation function
//! `0x0028c530`).
//!
//! - `retail_graphs_read_as_the_engine_reads_them`: every animation of
//!   every `jmad` on Coagulation has no problem, and some are codec 8
//!   (so the alignment rule is retail's, not an invention).
//! - `converted_blood_gulch_graphs_read_as_the_engine_reads_them`: Blood
//!   Gulch's vehicles, bipeds, weapons and equipment converted; every
//!   animation of every converted graph has no problem. Through converter
//!   version 88 the vehicles' codec 8 steering and aim overlays sat at
//!   4-aligned offsets, and the game stopped with #GP at `0x0028c589`
//!   about 5 s into a game.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};
use std::sync::Arc;

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::Converter;
use map::build::TagSet;
use map::{CacheType, FourCC, Map, RawPointer, ResourceFile, TagId};
use tags::anim::AnimationGraph;
use tags::anim_sample::Codec;

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map_bytes(want: &str) -> Vec<u8> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap();
    iso.read(&e).unwrap()
}

fn h1_level(name: &str) -> H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// Raw resources of the map whose bytes these are and of the three
/// shared cache files (most retail animations live in those).
struct Local {
    map: Vec<u8>,
    shared: Vec<u8>,
    single_player_shared: Vec<u8>,
    main_menu: Vec<u8>,
}

impl Local {
    fn new(map: Vec<u8>) -> Self {
        Local {
            map,
            shared: h2_map_bytes("shared"),
            single_player_shared: h2_map_bytes("single_player_shared"),
            main_menu: h2_map_bytes("mainmenu"),
        }
    }

    fn span(&self, p: RawPointer, size: u32) -> tags::Result<&[u8]> {
        let file = match p.file {
            ResourceFile::Local => &self.map,
            ResourceFile::Shared => &self.shared,
            ResourceFile::SinglePlayerShared => &self.single_player_shared,
            ResourceFile::MainMenu => &self.main_menu,
        };
        let (start, end) = (p.offset as usize, p.offset as usize + size as usize);
        file
            .get(start..end)
            .ok_or(tags::Error::Invalid("resource past the end of the map"))
    }
}

impl tags::sound::ResourceReader for Local {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        self.span(p, size).map(|_| ())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.span(p, size).map(<[u8]>::to_vec)
    }
}

/// What checking one graph found.
#[derive(Default)]
struct Checked {
    animations: usize,
    blend_screen: usize,
    problems: Vec<String>,
}

/// Every animation of graph `id` against the engine's rules.
fn check_graph(
    map: &mut Map<Cursor<Vec<u8>>>,
    raw: &mut Local,
    id: TagId,
    name: &str,
    out: &mut Checked,
) {
    let g = match AnimationGraph::read(map, id) {
        Ok(g) => g,
        Err(e) => {
            out.problems.push(format!("{name}: graph: {e}"));
            return;
        }
    };
    let strings = map.strings().to_vec();
    let mut resources: BTreeMap<usize, Arc<[u8]>> = BTreeMap::new();
    for (i, a) in g.animations.iter().enumerate() {
        let anim_name = a
            .name
            .resolve(&strings)
            .ok()
            .flatten()
            .unwrap_or("?")
            .to_owned();
        let r = a.resource_index as usize;
        if !resources.contains_key(&r) {
            match g.read_resource(r, &mut *raw) {
                Ok(bytes) => {
                    resources.insert(r, bytes);
                }
                Err(e) => {
                    out.problems
                        .push(format!("{name} {i} {anim_name}: resource {r}: {e}"));
                    continue;
                }
            }
        }
        let resource = &resources[&r];
        out.animations += 1;
        if g.clip_from_shared(i, resource)
            .is_ok_and(|c| c.codecs()[1] == Some(Codec::BlendScreen))
        {
            out.blend_screen += 1;
        }
        for p in g.engine_problems(i, resource) {
            out.problems.push(format!(
                "{name} {i} {anim_name} (offset {:#x}, static {:#x}, animated {:#x}): {p}",
                a.data_offset, a.sizes.static_size, a.sizes.animated_size
            ));
        }
    }
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_graphs_read_as_the_engine_reads_them() {
    let bytes = h2_map_bytes("coagulation");
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let base = Base::read(&mut map).unwrap();
    let mut raw = Local::new(bytes);
    let mut out = Checked::default();
    let mut graphs = 0;
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"jmad")) {
        check_graph(&mut map, &mut raw, t.0, &t.2, &mut out);
        graphs += 1;
    }
    println!(
        "{graphs} graphs, {} animations, {} codec 8",
        out.animations, out.blend_screen
    );
    for p in &out.problems {
        println!("{p}");
    }
    assert!(out.problems.is_empty(), "{} problems", out.problems.len());
    assert!(out.blend_screen > 0, "no codec 8 animation on Coagulation");
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn converted_blood_gulch_graphs_read_as_the_engine_reads_them() {
    let h1: &'static H1Map = Box::leak(Box::new(h1_level("bloodgulch")));
    let mut m = Map::from_reader(Cursor::new(h2_map_bytes("coagulation"))).unwrap();
    let base: &'static Base = Box::leak(Box::new(Base::read(&mut m).unwrap()));
    let mut c = Converter::new(h1, base);
    // Vehicles first: their graphs hold the codec 8 overlays.
    let mut vehicles = 0;
    for group in [b"vehi", b"bipd", b"weap", b"eqip"] {
        for t in h1
            .tags()
            .iter()
            .filter(|t| t.group.0.to_be_bytes() == *group)
        {
            match c.object(t) {
                Ok(_) => vehicles += usize::from(group == b"vehi"),
                Err(e) => println!("{}: not converted: {e}", t.name),
            }
        }
    }
    assert!(vehicles >= 3, "{vehicles} vehicles converted");

    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    // The graphs name what they play: the first-person graphs' sound
    // events name converted sounds (docs/halo1/models.md, "Animation
    // sounds"). Those go in first, with their gestalt, and the graphs'
    // placeholders point at them; a graph naming another converted tag
    // (a `local` placeholder) would need it in the map too, so none may.
    let mut tree = c.tree;
    tree.try_install_audio(&mut set).unwrap();
    assert!(tree.audio_notes.is_empty(), "{:?}", tree.audio_notes);
    fn locals(b: &map::build::Block, out: &mut Vec<usize>) {
        use map::build::FixupKind;
        for f in &b.fixups {
            match &f.kind {
                FixupKind::Block { child, .. }
                | FixupKind::Data(child)
                | FixupKind::Pointer(child) => locals(child, out),
                FixupKind::TagRef(t) | FixupKind::TagId(t) => {
                    out.extend(h1objects::convert::local_index(*t))
                }
                _ => {}
            }
        }
    }
    let mut installed = Vec::new();
    for (name, t) in &tree.tags {
        if t.group == FourCC(*b"jmad") {
            let mut named = Vec::new();
            locals(&t.root, &mut named);
            assert!(named.is_empty(), "{name} names converted tags {named:?}");
            let id = t.clone().install(&mut set, name).expect("install");
            installed.push((id, name.clone()));
        }
    }
    let (bytes, _) = set.build().expect("map builds");
    let mut map = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let mut raw = Local::new(bytes);
    let mut out = Checked::default();
    for (id, name) in &installed {
        check_graph(&mut map, &mut raw, *id, name, &mut out);
    }
    println!(
        "{} graphs, {} animations, {} codec 8",
        installed.len(),
        out.animations,
        out.blend_screen
    );
    for p in &out.problems {
        println!("{p}");
    }
    assert!(out.blend_screen > 0, "no codec 8 animation converted");
    assert!(out.problems.is_empty(), "{} problems", out.problems.len());
}
