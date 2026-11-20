//! The structure query that keeps a biped's ground contacts, on converted
//! Blood Gulch's blue ramp and on retail Coagulation.
//!
//! The engine keeps a body's contacts with the structure through the
//! BV-tree agent's box query (`mopp::query_box`, E `0x00300150` and
//! `0x002ff8b0`): each tick it queries the MOPP code with the body's box
//! (its shape's AABB at the collision tolerance, grown by its motion) and
//! drops the contact points of every surface not returned. Live, climbing
//! the converted blue ramp, the player's contact list held the ramp
//! (surface 2833, key `0x20000b11`) through tick 1171 and was empty at
//! 1172 (physics.support, agent API), at x 43.56 on the ramp.
//!
//! ```sh
//! cargo test --release -p h1convert --test it ramp_contacts:: -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use mopp::geom::structure_key;

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// Converted Blood Gulch's structure collision, as the map carries it.
fn bloodgulch() -> tags::collision::Bsp {
    let path = disc("H1_ISO", "halo1.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map("bloodgulch").cloned().unwrap();
    let h1 = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let r = h1.structure_bsps().unwrap();
    let s = h1.structure_bsp(&r[0]).unwrap();
    let bsp = s.collision_bsps().unwrap().remove(0);
    let b = h1convert::collision::convert(&bsp).unwrap();
    const BASE: u32 = 0x8000_0000;
    let bytes = b.encode_blocks(BASE);
    tags::collision::Bsp::decode_structure(&bytes, BASE, 0).unwrap()
}

/// A retail level's first structure BSP: its collision and MOPP block.
fn retail(name: &str) -> (tags::collision::Bsp, Vec<u8>) {
    use map::{FourCC, Map};
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    for e in iso.entries().unwrap() {
        let stem = e.path.rsplit(['/', '\\']).next().unwrap_or("");
        if stem != format!("{name}.map") {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut m = Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let bsps: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"sbsp") && t.location.is_some())
            .map(|t| (t.id, t.location.unwrap().address))
            .collect();
        for (id, address) in bsps {
            let b = m.read_tag_bytes(id).unwrap();
            let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            if u(0x14) != 1 {
                continue;
            }
            let at = (u(0x18) - address) as usize;
            let c = tags::collision::Bsp::decode_structure(&b, address, at).unwrap();
            // sbsp+0x1e0: the MOPP block (count, address).
            let (n, ptr) = (u(0x1e0) as usize, u(0x1e4));
            let o = (ptr - address) as usize;
            return (c, b[o..o + n].to_vec());
        }
    }
    panic!("{name} not on the disc");
}

/// The box the engine queries for a standing pill: feet at `p`, radius
/// 0.2, height 0.7 (the converted cyborg's), grown by the collision
/// tolerance `tol` and by one tick of motion `v`.
fn pill_box(p: [f32; 3], v: [f32; 3], tol: f32) -> mopp::Aabb {
    let r = 0.2 + tol;
    let mut min = [p[0] - r, p[1] - r, p[2] - tol];
    let mut max = [p[0] + r, p[1] + r, p[2] + 0.7 + tol];
    for a in 0..3 {
        let d = v[a] / 30.0;
        min[a] += d.min(0.0);
        max[a] += d.max(0.0);
    }
    mopp::Aabb { min, max }
}

/// The live climb: (tick, havok position, velocity) from physics.support,
/// the replay of `bg-ramp-centre`. The ramp contact is there through 1171
/// and gone at 1172.
const CLIMB: [(u32, [f32; 3], [f32; 3]); 2] = [
    (
        1171,
        [43.627_1, -79.232_37, 1.175_839],
        [-1.868_171, -0.544_498, 1.129_583],
    ),
    (
        1172,
        [43.561, -79.24, 1.207],
        [-1.994_706, -0.224_562, 0.932_650],
    ),
];
const RAMP: u32 = 0x2000_0b11;
const TOLERANCES: [f32; 4] = [0.0, 0.02, 0.05, 0.1];

/// The ramp under the live climb comes back from the engine's box query at
/// both ticks, at every plausible tolerance.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn blue_ramp_box_query() {
    let bsp = bloodgulch();
    let code = mopp::Code::parse(&h1convert::convert::structure_mopp(&bsp).unwrap()).unwrap();
    println!(
        "converted bloodgulch: offset {:?} scale {} ({} bytes of code)",
        code.offset,
        code.scale,
        code.bytecode.len()
    );
    let mut missed = Vec::new();
    for (tick, p, v) in CLIMB {
        for tol in TOLERANCES {
            let b = pill_box(p, v, tol);
            let engine = mopp::query_box(&code, &b).unwrap();
            let model = mopp::query_aabb(&code, &b).unwrap();
            let lo = [0, 1, 2].map(|a| code.to_full(a, b.min[a]));
            println!(
                "tick {tick} tol {tol}: box query ramp {} ({} keys), box model ramp {} ({} keys); low corner full {:?} (root cell z {})",
                engine.contains(&RAMP),
                engine.len(),
                model.contains(&RAMP),
                model.len(),
                lo.map(|v| v.round() as i64),
                (lo[2].round() as i64 - 1) >> 16
            );
            if !engine.contains(&RAMP) {
                missed.push((tick, tol));
            }
        }
    }
    assert!(missed.is_empty(), "ramp missing at {missed:?}");
}

/// A pill standing on any point of any surface overlaps that surface, so
/// the box query must return it. Checked at each surface's vertices and
/// centroid, on converted Blood Gulch (our compiler) and on retail
/// Coagulation (Bungie's), with the tolerances above.
#[test]
#[ignore = "reads both discs ($H1_ISO, $H2_DISC)"]
fn pills_on_surfaces_keep_them() {
    let bg = bloodgulch();
    let bg_block = h1convert::convert::structure_mopp(&bg).unwrap();
    let (coag, coag_block) = retail("coagulation");
    let mut report = Vec::new();
    for (name, bsp, block) in [
        ("converted bloodgulch", &bg, bg_block),
        ("retail coagulation", &coag, coag_block),
    ] {
        let code = mopp::Code::parse(&block).unwrap();
        let surfaces = mopp::geom::structure_surfaces(bsp);
        let (mut queries, mut misses) = (0usize, Vec::new());
        for s in &surfaces {
            let n = s.vertices.len() as f32;
            let centroid = [0, 1, 2].map(|a| s.vertices.iter().map(|p| p[a]).sum::<f32>() / n);
            for p in s.vertices.iter().copied().chain([centroid]) {
                for tol in TOLERANCES {
                    queries += 1;
                    let b = pill_box(p, [0.0; 3], tol);
                    if !mopp::query_box(&code, &b).unwrap().contains(&s.key) {
                        misses.push((s.key, p, tol));
                    }
                }
            }
        }
        println!(
            "{name}: {} surfaces, {queries} pill queries, {} missed {:?}",
            surfaces.len(),
            misses.len(),
            &misses[..misses.len().min(5)]
        );
        if !misses.is_empty() {
            report.push(format!("{name}: {} of {queries}", misses.len()));
        }
        // The ramp's own key, as a sanity check of the keying.
        if name.contains("bloodgulch") {
            assert!(surfaces.iter().any(|s| s.key == structure_key(0xb11)));
        }
    }
    assert!(report.is_empty(), "{report:?}");
}
