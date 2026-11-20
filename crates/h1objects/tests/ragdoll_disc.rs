//! The converted Halo 1 cyborg's ragdoll against retail's: the Chief's
//! and the Elite's physics models on the campaign base
//! (`05a_deltaapproach`) as the control, the cyborg converted from a10.
//! Reads `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! Retail is first held to what `crate::ragdoll` assumes of it (every
//! constraint joins two bodies down one node chain, through its own edge;
//! knees and elbows are limited hinges, the rest ragdoll joints; the
//! frames are the child's rest transform, so they meet at rest). The
//! converted ragdoll is then held to the same shape, its limits to
//! retail's range for each joint, and its physics model to the decoder's
//! validation.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it ragdoll_disc:: -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::Converted;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::physics::Transform;
use h1objects::ragdoll::{self, Frame, JointClass};
use map::build::{Block, FixupKind};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Map<Cursor<Vec<u8>>> {
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
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
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

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

fn reference(r: &Record, path: &str) -> Option<TagId> {
    match r.at(path)? {
        Val::Ref {
            target: Target::Id(id),
            ..
        } => Some(*id),
        _ => None,
    }
}

fn local(tree: &Tree, id: TagId) -> Option<&Converted> {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return None;
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1)
}

fn block(r: &Record, p: &str) -> Vec<Record> {
    match r.at(p) {
        Some(Val::Block(b)) => b.clone(),
        other => panic!("{p}: {other:?}"),
    }
}
fn int(r: &Record, p: &str) -> i64 {
    match r.at(p) {
        Some(Val::Int(i)) => *i,
        other => panic!("{p}: {other:?}"),
    }
}
fn float(r: &Record, p: &str) -> f32 {
    match r.at(p) {
        Some(Val::Float(f)) => *f,
        other => panic!("{p}: {other:?}"),
    }
}
fn floats(r: &Record, p: &str) -> [f32; 3] {
    match r.at(p) {
        Some(Val::List(v)) => std::array::from_fn(|k| match v[k] {
            Val::Float(f) => f,
            _ => panic!("{p}"),
        }),
        other => panic!("{p}: {other:?}"),
    }
}
fn frame(c: &Record, which: &str) -> Frame {
    let f = |a: &str| floats(c, &format!("header.{which}.{a}"));
    Frame {
        forward: f("forward"),
        left: f("left"),
        up: f("up"),
        position: f("position"),
    }
}

/// One constraint read back: kind (1 hinge, 2 ragdoll), nodes, frames,
/// limits (hinge `(min, max)`; ragdoll twist, cone, plane pairs).
#[derive(Debug)]
struct Read {
    kind: i64,
    a: i16,
    b: i16,
    fa: Frame,
    fb: Frame,
    limits: Vec<(f32, f32)>,
}

/// The constraints of a physics model, checked against its edges and
/// bodies: every constraint's edge is in range, holds its nodes and names
/// it back; both nodes ride different bodies; there are no other
/// constraint kinds.
fn constraints(p: &Record) -> Vec<Read> {
    let edges = block(p, "node_edges");
    let bodies: Vec<i64> = block(p, "rigid_bodies")
        .iter()
        .map(|b| int(b, "node"))
        .collect();
    for other in ["block_f0"] {
        assert!(block(p, other).is_empty(), "{other}");
    }
    let mut out = Vec::new();
    for (kind, path) in [(1, "limited_hinge_constraints"), (2, "ragdoll_constraints")] {
        for (i, c) in block(p, path).iter().enumerate() {
            let (a, b) = (int(c, "header.node_a"), int(c, "header.node_b"));
            let e = &edges[int(c, "header.edge") as usize];
            assert_eq!(
                (int(e, "node_a"), int(e, "node_b")),
                (a, b),
                "{path}[{i}] edge"
            );
            let refs = block(e, "constraints");
            assert!(
                refs.iter()
                    .any(|r| (int(r, "constraint_type"), int(r, "constraint_index"))
                        == (kind, i as i64)),
                "{path}[{i}]: its edge does not name it"
            );
            let ba = bodies.iter().position(|&n| n == a);
            let bb = bodies.iter().position(|&n| n == b);
            assert!(
                ba.is_some() && bb.is_some() && ba != bb,
                "{path}[{i}] bodies {a} {b}"
            );
            let limits = if kind == 1 {
                vec![(float(c, "min_angle"), float(c, "max_angle"))]
            } else {
                ["twist", "cone", "plane"]
                    .iter()
                    .map(|l| (float(c, &format!("min_{l}")), float(c, &format!("max_{l}"))))
                    .collect()
            };
            out.push(Read {
                kind,
                a: a as i16,
                b: b as i16,
                fa: frame(c, "frame_a"),
                fb: frame(c, "frame_b"),
                limits,
            });
        }
    }
    out
}

/// (parent, child) of a constraint as retail orders them, checked to be
/// ancestor and descendant.
fn parent_child(c: &Read, parents: &[i16]) -> (i16, i16) {
    let (p, ch) = if c.kind == 1 { (c.a, c.b) } else { (c.b, c.a) };
    let mut up = parents[ch as usize];
    while up != p {
        assert!(up >= 0, "{c:?}: {p} is not an ancestor of {ch}");
        up = parents[up as usize];
    }
    (p, ch)
}

fn near(x: [f32; 3], y: [f32; 3], tol: f32) -> bool {
    (0..3).all(|k| (x[k] - y[k]).abs() <= tol)
}

/// The frames meet at rest: frame A carried into the world by node A's
/// rest transform is frame B carried by node B's (positions within
/// `tol` world units, axes within `axes`; retail's physics models were
/// authored on a pose up to a degree off the render model's).
fn frames_meet(c: &Read, parents: &[i16], rest: &[Transform], tol: f32, axes: f32) -> bool {
    let fa = c.fa.carried(&ragdoll::relative(parents, rest, -1, c.a));
    let fb = c.fb.carried(&ragdoll::relative(parents, rest, -1, c.b));
    near(fa.position, fb.position, tol)
        && near(fa.forward, fb.forward, axes)
        && near(fa.left, fb.left, axes)
        && near(fa.up, fb.up, axes)
}

/// The retail biped `name`'s physics model, node names, parents and rest
/// transforms (from its render model).
fn retail(
    base: &Base,
    strings: &[Option<String>],
    name: &str,
) -> Option<(Record, Vec<String>, Vec<i16>, Vec<Transform>)> {
    let id = base.find(b"bipd", name)?;
    let b = base.record(def(b"bipd"), id).unwrap();
    let h = base
        .record(def(b"hlmt"), reference(&b, "unit.obje.model")?)
        .unwrap();
    let p = base
        .record(def(b"phmo"), reference(&h, "physics_model")?)
        .unwrap();
    let m = base
        .record(def(b"mode"), reference(&h, "render_model")?)
        .unwrap();
    let nodes = block(&m, "nodes");
    let names = nodes
        .iter()
        .map(|n| match n.at("name") {
            Some(Val::String(tags::defs::Str::Index(i))) => strings
                .get((*i & 0xff_ffff) as usize)
                .cloned()
                .flatten()
                .unwrap_or_default(),
            Some(Val::String(tags::defs::Str::Text(t))) => t.clone(),
            _ => String::new(),
        })
        .collect();
    let parents = nodes
        .iter()
        .map(|n| int(n, "parent_index") as i16)
        .collect();
    let rest = nodes
        .iter()
        .map(|n| {
            let q = match n.at("orientation") {
                Some(Val::List(v)) => std::array::from_fn(|k| match v[k] {
                    Val::Float(f) => f,
                    _ => panic!(),
                }),
                other => panic!("{other:?}"),
            };
            Transform {
                r: h1models::mode::quat_matrix(q),
                t: floats(n, "translation"),
            }
        })
        .collect();
    Some((p, names, parents, rest))
}

/// Lays a built tag out at `base` (blocks after their parents, aligned) so
/// the decoder can read it.
fn lay_out(b: &Block, base: u32) -> Vec<u8> {
    fn place(b: &Block, out: &mut Vec<u8>, base: u32) -> u32 {
        let align = b.align.max(4) as usize;
        while out.len() % align != 0 {
            out.push(0);
        }
        let at = out.len();
        out.extend_from_slice(&b.bytes);
        for f in &b.fixups {
            let o = at + f.offset as usize;
            let (n, addr) = match &f.kind {
                FixupKind::Block { count, child } => {
                    if *count == 0 && child.bytes.is_empty() {
                        (0, 0)
                    } else {
                        (*count, place(child, out, base))
                    }
                }
                FixupKind::Data(child) => (child.bytes.len() as u32, place(child, out, base)),
                _ => continue,
            };
            out[o..o + 4].copy_from_slice(&n.to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&addr.to_le_bytes());
        }
        base + at as u32
    }
    let mut out = Vec::new();
    place(b, &mut out, base);
    out
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn the_converted_cyborgs_ragdoll_against_retail() {
    let mut base_map = h2_map("05a_deltaapproach");
    let strings = base_map.strings().to_vec();
    let base = Base::read(&mut base_map).unwrap();

    // Retail: the control, and each joint class's limit range.
    let mut range: BTreeMap<(JointClass, usize), (f32, f32)> = BTreeMap::new();
    let mut kinds: BTreeMap<JointClass, i64> = BTreeMap::new();
    let mut controls = 0;
    for name in [
        "objects\\characters\\masterchief\\masterchief",
        "objects\\characters\\elite\\elite",
    ] {
        let Some((p, names, parents, rest)) = retail(&base, &strings, name) else {
            println!("{name}: not on the base");
            continue;
        };
        controls += 1;
        let cs = constraints(&p);
        let bodies = block(&p, "rigid_bodies").len();
        println!("{name}: {bodies} bodies, {} constraints", cs.len());
        assert_eq!(cs.len() + 1, bodies, "{name}: a tree over the bodies");
        for c in &cs {
            let (pn, ch) = parent_child(c, &parents);
            let class = JointClass::of(&names[ch as usize]);
            println!(
                "  {:>8} {} <- {} {:?}: {:?}",
                if c.kind == 1 { "hinge" } else { "ragdoll" },
                names[pn as usize],
                names[ch as usize],
                class,
                c.limits
            );
            // Kinds by class, as crate::ragdoll assigns them.
            let want = if matches!(class, JointClass::Knee | JointClass::Elbow) {
                1
            } else {
                2
            };
            assert_eq!(c.kind, want, "{name}: {class:?}");
            kinds.insert(class, c.kind);
            // The frames: the child's on its origin, and both meet at rest
            // (the construction crate::ragdoll uses). Retail's ragdoll
            // joints tilt the parent's frame a little off rest (the cone
            // is not centred on the bone), so those meet by position only.
            let own = if ch == c.a { c.fa } else { c.fb };
            assert!(near(own.position, [0.0; 3], 1e-4), "{name}: {c:?}");
            if c.kind == 1 {
                // Columns: the child's axes in the parent's space (rows
                // would put the knee's bend the other way).
                assert!(frames_meet(c, &parents, &rest, 2e-3, 0.02), "{name}: {c:?}");
            } else {
                let fa = c.fa.carried(&ragdoll::relative(&parents, &rest, -1, c.a));
                let fb = c.fb.carried(&ragdoll::relative(&parents, &rest, -1, c.b));
                assert!(near(fa.position, fb.position, 2e-3), "{name}: {c:?}");
                assert!(
                    near(own.forward, [-1.0, 0.0, 0.0], 0.05),
                    "{name}: twist axis {c:?}"
                );
            }
            for (k, &(lo, hi)) in c.limits.iter().enumerate() {
                let e = range.entry((class, k)).or_insert((f32::MAX, f32::MIN));
                e.0 = e.0.min(lo.min(hi));
                e.1 = e.1.max(lo.max(hi));
            }
        }
    }
    assert!(controls > 0, "no retail ragdoll on the base");

    // The converted cyborg.
    let h1 = h1_level(&std::env::var("H1_LEVEL").unwrap_or_else(|_| "a10".into()));
    let t = h1
        .tags()
        .iter()
        .find(|t| t.group.0.to_be_bytes() == *b"bipd" && t.name.ends_with("cyborg"))
        .unwrap()
        .clone();
    let mut conv = Converter::new(&h1, &base);
    let id = conv.object(&t).unwrap();
    let r = defs::from_built(def(b"bipd"), &local(&conv.tree, id).unwrap().root).unwrap();
    // The tag-side gate besides the physics model: `bipd +0x1f0` bit 11
    // (E 0x000e1097, 0x001d6c18) as the Chief's.
    let word = |r: &Record| match r.at("unknown_1f0") {
        Some(Val::Bytes(b)) => u32::from_le_bytes(b[..4].try_into().unwrap()),
        other => panic!("{other:?}"),
    };
    let chief = base
        .record(
            def(b"bipd"),
            base.find(b"bipd", "objects\\characters\\masterchief\\masterchief")
                .unwrap(),
        )
        .unwrap();
    println!(
        "bipd +0x1f0: converted {:#x}, chief {:#x}",
        word(&r),
        word(&chief)
    );
    assert_eq!(word(&r) >> 11 & 1, word(&chief) >> 11 & 1);
    let hl = local(&conv.tree, reference(&r, "unit.obje.model").unwrap()).unwrap();
    let h = defs::from_built(def(b"hlmt"), &hl.root).unwrap();
    let built = local(&conv.tree, reference(&h, "physics_model").unwrap()).unwrap();
    let p = defs::from_built(def(b"phmo"), &built.root).unwrap();
    let pnodes = block(&p, "nodes");
    let names: Vec<String> = pnodes
        .iter()
        .map(|n| match n.at("name") {
            Some(Val::String(tags::defs::Str::Text(t))) => t.clone(),
            other => panic!("{other:?}"),
        })
        .collect();
    let parents: Vec<i16> = pnodes.iter().map(|n| int(n, "parent") as i16).collect();
    let cs = constraints(&p);
    let bodies = block(&p, "rigid_bodies");
    println!(
        "converted cyborg: {} bodies, {} constraints",
        bodies.len(),
        cs.len()
    );
    assert_eq!(cs.len() + 1, bodies.len(), "a tree over the bodies");
    let (mut hinges, mut rags) = (0, 0);
    for c in &cs {
        let (pn, ch) = parent_child(c, &parents);
        let class = JointClass::of(&names[ch as usize]);
        println!(
            "  {:>8} {} <- {} {class:?}: {:?}",
            if c.kind == 1 { "hinge" } else { "ragdoll" },
            names[pn as usize],
            names[ch as usize],
            c.limits
        );
        if c.kind == 1 {
            hinges += 1
        } else {
            rags += 1
        }
        if let Some(&k) = kinds.get(&class) {
            assert_eq!(c.kind, k, "{class:?}: retail's kind");
        }
        for (k, &(lo, hi)) in c.limits.iter().enumerate() {
            assert!(lo <= hi, "{class:?}: {lo} > {hi}");
            if let Some(&(rlo, rhi)) = range.get(&(class, k)) {
                assert!(
                    rlo <= lo && hi <= rhi,
                    "{class:?} limit {k}: ({lo}, {hi}) outside retail's ({rlo}, {rhi})"
                );
            }
        }
    }
    assert!(
        hinges >= 2 && rags >= 3,
        "knees and elbows hinge; hips, shoulders, head ragdoll"
    );

    // The engine's post-process view: the decoder reads the built model
    // and its validation passes.
    const BASE: u32 = 0x8000_0000;
    let bytes = lay_out(&built.root, BASE);
    let model = tags::collision::PhysicsModel::decode(&bytes, BASE).unwrap();
    let v = model.validate();
    assert!(v.is_empty(), "{v:?}");
}
