//! Converted model shaders of a10's cryo tube on the disc: its glass
//! (`sotr` `levels\a10\shaders\cryo glass`) is rebuilt as an alpha
//! blended shader drawing its own glass texture and reflection cube, not
//! the base's stand-in model shader. Reads `$H1_ISO` and `$H2_DISC`;
//! prints names only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test model_shaders_disc -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Group, H1Map};
use h1objects::base::Base;
use h1objects::convert::{Converter, local};
use map::build::{Block, FixupKind};
use map::{Map, TagId};

fn h1_map(name: &str) -> H1Map {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map(name).expect("map on the disc").clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// Every tag reference in `b` and its child blocks.
fn tag_refs(b: &Block, out: &mut Vec<TagId>) {
    for f in &b.fixups {
        match &f.kind {
            FixupKind::TagRef(id) => out.push(*id),
            FixupKind::Block { child, .. } => tag_refs(child, out),
            _ => {}
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn a10_cryo_tube_glass_is_alpha_blended_with_its_own_maps() {
    let h1 = h1_map("a10");
    // The campaign base a10 converts onto (h1campaign).
    let mut base_map = h2_map("05a_deltaapproach");
    let library = h1convert::shad::Library::read(&mut base_map);
    let blended = library
        .stem(&h1convert::shad::ONE_ALPHA_ENV)
        .expect("the base has one_alpha_env");
    let base = Base::read(&mut base_map).unwrap();
    let stand_in = base
        .model_shader
        .as_ref()
        .expect("the base has a model shader")
        .name
        .clone();
    let mut conv = Converter::new(&h1, &base);
    let tube = h1
        .tags()
        .iter()
        .find(|t| {
            t.group == Group::new(b"mach") && t.name == "levels\\a10\\devices\\cryotube\\cryotube"
        })
        .expect("a10's cryo tube machine");
    conv.object(tube).unwrap();

    let tree = &conv.tree;
    // Only the tree's own (local) IDs name its tags; the stem and default
    // bitmaps are the base's.
    let named = |id: TagId| {
        (id.0 & 0xffff_0000 == local(0).0 & 0xffff_0000)
            .then(|| {
                tree.tags
                    .get((id.0 & 0xffff) as usize)
                    .map(|(n, _)| n.as_str())
            })
            .flatten()
    };
    let (_, glass) = tree
        .tags
        .iter()
        .find(|(n, c)| n == "levels\\a10\\shaders\\cryo glass" && c.group.0 == *b"shad")
        .expect("the cryo glass is a shader of its own, not the stand-in");
    let mut refs = Vec::new();
    tag_refs(&glass.root, &mut refs);
    let names: Vec<&str> = refs.iter().filter_map(|&id| named(id)).collect();
    eprintln!(
        "cryo glass: {} references; tree's: {names:?}; stand-in {stand_in}",
        refs.len()
    );
    assert!(refs.contains(&blended), "drawn with one_alpha_env");
    assert!(
        names.contains(&"levels\\a10\\bitmaps\\cryo glass"),
        "its glass texture"
    );
    assert!(
        names.contains(&"levels\\a10\\bitmaps\\cube map glass"),
        "its reflection cube"
    );
    let report = tree
        .reports
        .iter()
        .find(|r| r.source == tube.name)
        .expect("the tube's report");
    assert!(
        !report.no_home.iter().any(|n| n.contains("cryo glass")),
        "{:?}",
        report.no_home
    );
}
