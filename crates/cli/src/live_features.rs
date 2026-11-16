//! Teleporters, their links and lifts for a level's `map.glb`
//! (`asset.extras.features`, players contract section 13), read at run
//! time from the scenario: teleporters from its netgame flags, lifts from
//! the objects it places whose tag names say lift. Game coordinates.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use assets::ResourceFiles;
use map::TagId;
use serde::Serialize;
use tags::geometry::TagView;

use crate::live_geometry::{Bounds, MapReader, Placement, object_model};

/// The scenario's netgame flags (`scnr+0x118`, 0x20-byte entries):
/// position (`+0`), facing in radians (`+0xc`), type (`+0x10`, i16), team
/// (`+0x12`) and identifier (`+0x14`, i16): the public scenario layout,
/// checked on the disc by every retail teleporter pad pairing up.
const NETGAME_FLAGS: usize = 0x118;
const NETGAME_FLAG_STRIDE: usize = 0x20;
/// Netgame flag types: a teleporter's entry and its exit.
pub const TELEPORTER_SOURCE: i16 = 7;
pub const TELEPORTER_DESTINATION: i16 = 8;

/// Scenario placement blocks that can hold a lift: (name, placements,
/// stride, palette), the palette 8 bytes after its placements with 0x28-byte
/// entries, tag reference first (the object type table, docs/formats/objects.md).
const OBJECT_BLOCKS: [(&str, usize, usize); 4] = [
    ("scen", 0x50, 0x5c),
    ("mach", 0xa8, 0x48),
    ("ctrl", 0xb8, 0x44),
    ("bloc", 0x328, 0x4c),
];
const PALETTE_STRIDE: usize = 0x28;

/// Flags closer than this (world units) are one pad.
const SAME_PAD: f32 = 0.5;

/// A netgame flag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flag {
    pub pos: [f32; 3],
    pub facing: f32,
    pub kind: i16,
    pub channel: i16,
}

impl Flag {
    pub fn parse(b: &[u8]) -> Option<Flag> {
        let f = |o: usize| Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?));
        let i = |o: usize| Some(i16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?));
        Some(Flag {
            pos: [f(0)?, f(4)?, f(8)?],
            facing: f(0xc)?,
            kind: i(0x10)?,
            channel: i(0x14)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Teleporter {
    pub id: String,
    pub pos: [f32; 3],
    pub facing: f32,
    pub role: &'static str,
    pub channel: i16,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Link {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lift {
    pub id: String,
    pub pos: [f32; 3],
    pub dir: [f32; 3],
    pub length: f32,
    pub radius: f32,
    pub source: String,
}

/// `asset.extras.features`.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Features {
    pub teleporters: Vec<Teleporter>,
    pub links: Vec<Link>,
    pub lifts: Vec<Lift>,
}

impl Features {
    pub fn is_empty(&self) -> bool {
        self.teleporters.is_empty() && self.lifts.is_empty()
    }
}

/// Teleporter pads from netgame flags, and where each sends a player.
///
/// Source and destination flags within [`SAME_PAD`] of each other are one
/// pad: `two_way` when it has both, else `sender` or `receiver`. A pad's
/// `channel` is its source flag's identifier (else its destination's); its
/// `facing` is its destination flag's (the way a player arriving there
/// faces), else its source's. A link goes from every pad with a source
/// flag to every other pad with a destination flag of the same identifier.
/// Pads are numbered `t0`, `t1`, ... in flag order.
pub fn teleporters(flags: &[Flag]) -> (Vec<Teleporter>, Vec<Link>) {
    struct Pad {
        pos: [f32; 3],
        sources: Vec<Flag>,
        destinations: Vec<Flag>,
    }
    let mut pads: Vec<Pad> = Vec::new();
    for f in flags {
        let source = f.kind == TELEPORTER_SOURCE;
        if !source && f.kind != TELEPORTER_DESTINATION {
            continue;
        }
        if !f.pos.iter().all(|v| v.is_finite()) {
            continue;
        }
        let near = pads.iter_mut().find(|p| {
            let d: f32 = (0..3).map(|k| (p.pos[k] - f.pos[k]).powi(2)).sum();
            d <= SAME_PAD * SAME_PAD
        });
        let pad = match near {
            Some(p) => p,
            None => {
                pads.push(Pad {
                    pos: f.pos,
                    sources: Vec::new(),
                    destinations: Vec::new(),
                });
                pads.last_mut().unwrap()
            }
        };
        if source {
            pad.sources.push(*f);
        } else {
            pad.destinations.push(*f);
        }
    }
    let out: Vec<Teleporter> = pads
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (role, channel, facing) = match (p.sources.first(), p.destinations.first()) {
                (Some(s), Some(d)) => ("two_way", s.channel, d.facing),
                (Some(s), None) => ("sender", s.channel, s.facing),
                (None, Some(d)) => ("receiver", d.channel, d.facing),
                (None, None) => unreachable!("a pad has a flag"),
            };
            Teleporter {
                id: format!("t{i}"),
                pos: p.pos,
                facing,
                role,
                channel,
            }
        })
        .collect();
    let mut links = Vec::new();
    for (i, from) in pads.iter().enumerate() {
        for (j, to) in pads.iter().enumerate() {
            let linked = i != j
                && from
                    .sources
                    .iter()
                    .any(|s| to.destinations.iter().any(|d| d.channel == s.channel));
            if linked {
                links.push(Link {
                    from: out[i].id.clone(),
                    to: out[j].id.clone(),
                });
            }
        }
    }
    (out, links)
}

/// Whether an object's tag name (its last path component) names a lift:
/// the generic words `lift`, `grav` or `jumppad`. (Not `gravity`: a
/// gravity hammer is a weapon; not `elevator`: an elevator is a machine,
/// drawn and moved as one.)
pub fn is_lift(name: &str) -> bool {
    let own = name.rsplit(['\\', '/']).next().unwrap_or(name);
    own.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|w| matches!(w, "lift" | "grav" | "jumppad"))
}

/// A render model shorter than this (world units) is a base or an emitter,
/// not the lift's column: the bounding sphere sizes it instead.
const MIN_COLUMN: f32 = 1.0;
/// A lift's column when neither its model nor its bounding sphere is
/// [`MIN_COLUMN`] tall (its volume is only in its physics model, which is
/// not read): 4 units up, 1 across, from where it is placed.
const NOMINAL: Bounds = Bounds {
    min: [-0.5, -0.5, 0.0],
    max: [0.5, 0.5, 4.0],
};

/// A lift as a column standing on the bottom of `extent` (a model-space
/// box): from the middle of its base along the object's up axis, as long
/// as the box is tall, as wide as its wider side.
pub fn lift(id: String, at: &Placement, extent: Bounds, source: String) -> Lift {
    let s = at.scale();
    let mid = |k: usize| (extent.min[k] + extent.max[k]) / 2.0;
    let half = |k: usize| (extent.max[k] - extent.min[k]) / 2.0;
    Lift {
        id,
        pos: at.transform([mid(0), mid(1), extent.min[2]]),
        dir: at.rotate([0.0, 0.0, 1.0]),
        length: (extent.max[2] - extent.min[2]) * s,
        radius: half(0).max(half(1)) * s,
        source,
    }
}

/// The box around an object's bounding sphere (`obje` bounding radius and
/// offset).
pub fn sphere_box(radius: f32, offset: [f32; 3]) -> Bounds {
    Bounds {
        min: offset.map(|v| v - radius),
        max: offset.map(|v| v + radius),
    }
}

/// The level's features: teleporters from its netgame flags; with `lifts`,
/// lifts from every placed scenery, machine, control or crate whose tag
/// names a lift ([`is_lift`]), sized by its render model's bounds when the
/// model is at least [`MIN_COLUMN`] tall, else by its object bounding
/// sphere when that is, else [`NOMINAL`]; `source` says which. A lift
/// whose object cannot be read is left out and listed in `skipped`
/// (`map/<stem>/lift_<block>_<index>`).
pub fn read_features(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    stem: &str,
    with_lifts: bool,
    skipped: &mut BTreeMap<String, String>,
) -> Result<Features> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let root = view.at(0, 0x400)?;
    let flags: Vec<Flag> = view
        .block(root, NETGAME_FLAGS, NETGAME_FLAG_STRIDE)?
        .into_iter()
        .filter_map(Flag::parse)
        .collect();
    let (teleporters, links) = teleporters(&flags);

    let ctx = tags::object::Context::read(map)?;
    let mut extents: HashMap<TagId, Result<(Bounds, &'static str), String>> = HashMap::new();
    let mut lifts = Vec::new();
    let blocks = if with_lifts { &OBJECT_BLOCKS[..] } else { &[] };
    for &(block, at, stride) in blocks {
        let palette: Vec<(TagId, String)> = view
            .block(root, at + 8, PALETTE_STRIDE)?
            .into_iter()
            .map(|e| {
                let id = TagId(u32::from_le_bytes([e[4], e[5], e[6], e[7]]));
                let name = map.tag(id).ok().and_then(|t| t.name.clone());
                (id, name.unwrap_or_default())
            })
            .collect();
        if !palette.iter().any(|(_, n)| is_lift(n)) {
            continue;
        }
        for (i, rec) in view.block(root, at, stride)?.into_iter().enumerate() {
            let Some(place) = Placement::parse(rec) else {
                continue;
            };
            let Some((id, name)) = usize::try_from(place.palette)
                .ok()
                .and_then(|p| palette.get(p))
            else {
                continue;
            };
            if *id == TagId::NULL || !is_lift(name) {
                continue;
            }
            let extent = extents.entry(*id).or_insert_with(|| {
                let header = ctx
                    .decode(*id)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| {
                        let obje = t.layer("obje").context("no object header")?;
                        Ok(tags::object::ObjectHeader::from_record(obje)?)
                    })
                    .map_err(|e| format!("{e:#}"))?;
                match object_model(map, &ctx, files, *id, None) {
                    Ok(Some(m)) if m.mesh.bounds.max[2] - m.mesh.bounds.min[2] >= MIN_COLUMN => {
                        Ok((m.mesh.bounds, "render model bounds"))
                    }
                    _ if 2.0 * header.bounding_radius >= MIN_COLUMN => Ok((
                        sphere_box(header.bounding_radius, header.bounding_offset),
                        "bounding sphere",
                    )),
                    _ => Ok((NOMINAL, "nominal size")),
                }
            });
            let stem_of = name.rsplit(['\\', '/']).next().unwrap_or(name);
            match extent {
                Ok((b, how)) => lifts.push(lift(
                    format!("l{}", lifts.len()),
                    &place,
                    *b,
                    format!("{block} {stem_of} {i}: {how}"),
                )),
                Err(why) => {
                    skipped.insert(format!("map/{stem}/lift_{block}_{i}"), why.clone());
                }
            }
        }
    }
    Ok(Features {
        teleporters,
        links,
        lifts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(pos: [f32; 3], facing: f32, kind: i16, channel: i16) -> Flag {
        Flag {
            pos,
            facing,
            kind,
            channel,
        }
    }

    #[test]
    fn a_flag_reads_type_and_identifier() {
        let mut b = vec![0u8; NETGAME_FLAG_STRIDE];
        for (k, v) in [1.5f32, -2.0, 3.25, 0.5].into_iter().enumerate() {
            b[k * 4..k * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b[0x10..0x12].copy_from_slice(&7i16.to_le_bytes());
        b[0x12..0x14].copy_from_slice(&8i16.to_le_bytes()); // team: ignored
        b[0x14..0x16].copy_from_slice(&3i16.to_le_bytes());
        assert_eq!(
            Flag::parse(&b),
            Some(flag([1.5, -2.0, 3.25], 0.5, TELEPORTER_SOURCE, 3))
        );
        assert_eq!(Flag::parse(&b[..0x14]), None);
    }

    #[test]
    fn pads_pair_up_by_channel_and_merge_into_two_way() {
        let a = [-16.0, 0.0, 0.2];
        let b = [18.0, 0.0, 0.2];
        let c = [0.0, 30.0, 5.0];
        let flags = [
            flag(a, 0.0, TELEPORTER_SOURCE, 0),
            flag(b, -3.1, TELEPORTER_SOURCE, 1),
            flag([0.0; 3], 0.0, 2, 0), // a flag of another kind
            flag([b[0] + 0.1, b[1], b[2]], 0.0, TELEPORTER_DESTINATION, 0),
            flag(a, 3.1, TELEPORTER_DESTINATION, 1),
            // A one-way pair: c sends to a lone receiver d.
            flag(c, 1.0, TELEPORTER_SOURCE, 5),
            flag([40.0, 0.0, 0.0], 2.0, TELEPORTER_DESTINATION, 5),
            flag([f32::NAN, 0.0, 0.0], 0.0, TELEPORTER_SOURCE, 9),
        ];
        let (pads, links) = teleporters(&flags);
        let roles: Vec<(&str, &str, i16, f32)> = pads
            .iter()
            .map(|p| (p.id.as_str(), p.role, p.channel, p.facing))
            .collect();
        assert_eq!(
            roles,
            [
                ("t0", "two_way", 0, 3.1),
                ("t1", "two_way", 1, 0.0),
                ("t2", "sender", 5, 1.0),
                ("t3", "receiver", 5, 2.0),
            ]
        );
        assert_eq!(pads[1].pos, b, "the first flag's position");
        let links: Vec<(&str, &str)> = links
            .iter()
            .map(|l| (l.from.as_str(), l.to.as_str()))
            .collect();
        assert_eq!(links, [("t0", "t1"), ("t1", "t0"), ("t2", "t3")]);
        assert_eq!(teleporters(&[]), (Vec::new(), Vec::new()));
    }

    #[test]
    fn lifts_by_name_stand_on_their_model_box() {
        assert!(is_lift(r"scenarios\objects\multi\x\grav_thing"));
        assert!(is_lift(r"a\b\cov_power_lift"));
        assert!(is_lift(r"a\b\tower_jumppad"));
        assert!(!is_lift(r"a\b\elevator_a"), "a machine");
        assert!(!is_lift(r"a\b\gravity_hammer"), "a weapon");
        assert!(!is_lift(r"a\b\liftoff"), "whole words");
        assert!(!is_lift(r"a\lift\crate"), "the tag's own name only");

        // A lift tilted onto its side (pitch 90 degrees: up becomes -x),
        // scaled 2, at (10, 0, 5).
        let at = Placement {
            palette: 0,
            position: [10.0, 0.0, 5.0],
            rotation: [0.0, std::f32::consts::FRAC_PI_2, 0.0],
            scale: 2.0,
        };
        let extent = Bounds {
            min: [-1.0, -0.5, 0.0],
            max: [1.0, 0.5, 6.0],
        };
        let l = lift("l0".into(), &at, extent, "test".into());
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4);
        assert!(near(l.pos, [10.0, 0.0, 5.0]), "{:?}", l.pos);
        assert!(near(l.dir, [-1.0, 0.0, 0.0]), "{:?}", l.dir);
        assert_eq!((l.length, l.radius), (12.0, 2.0));

        let upright = Placement {
            rotation: [0.0; 3],
            scale: 0.0,
            ..at
        };
        let l = lift(
            "l1".into(),
            &upright,
            sphere_box(3.0, [0.0, 0.0, 4.0]),
            "s".into(),
        );
        assert!(near(l.pos, [10.0, 0.0, 6.0]));
        assert!(near(l.dir, [0.0, 0.0, 1.0]));
        assert_eq!((l.length, l.radius), (6.0, 3.0));
    }

    /// Lists every level's teleporters, links and lifts from the owner's
    /// disc (`H2_DISC`, else `disc/halo2.iso` in the main checkout) into
    /// `features.txt` in the geometry check directory, outside the
    /// repository (`$H2_GEOMETRY_CHECK`, else the path below).
    #[test]
    #[ignore = "needs the disc; writes outside the repository"]
    fn features_from_the_disc() {
        use crate::live_art::Disc;
        use map::CacheType;
        let disc_path = std::env::var("H2_DISC").unwrap_or_else(|_| {
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../../disc/halo2.iso").into()
        });
        let keep = std::env::var("H2_GEOMETRY_CHECK").unwrap_or_else(|_| {
            "/Users/afr/.config/rush/keep/h2recomp-b5afe71a/bed6c8dc/map-quality".into()
        });
        let disc = Disc::scan(std::path::Path::new(&disc_path)).unwrap();
        let mut out = String::new();
        let maps = disc
            .maps_of(CacheType::Multiplayer)
            .map(|m| (m, true))
            .chain(disc.maps_of(CacheType::Campaign).map(|m| (m, false)));
        for (m, with_lifts) in maps {
            let (mut map, mut files) = disc.open(m).unwrap();
            let mut skipped = BTreeMap::new();
            let f =
                read_features(&mut map, &mut files, m.name(), with_lifts, &mut skipped).unwrap();
            out += &format!("== {}\n", m.name());
            for t in &f.teleporters {
                out += &format!(
                    "  {} {:<8} channel {:>2} at ({:.2}, {:.2}, {:.2}) facing {:.2}\n",
                    t.id, t.role, t.channel, t.pos[0], t.pos[1], t.pos[2], t.facing
                );
            }
            for l in &f.links {
                out += &format!("  link {} -> {}\n", l.from, l.to);
            }
            for l in &f.lifts {
                out += &format!(
                    "  {} at ({:.2}, {:.2}, {:.2}) dir ({:.2}, {:.2}, {:.2}) length {:.2} radius {:.2} from {}\n",
                    l.id,
                    l.pos[0],
                    l.pos[1],
                    l.pos[2],
                    l.dir[0],
                    l.dir[1],
                    l.dir[2],
                    l.length,
                    l.radius,
                    l.source
                );
            }
            for (k, why) in &skipped {
                out += &format!("  skipped {k}: {why}\n");
            }
        }
        print!("{out}");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(std::path::Path::new(&keep).join("features.txt"), out).unwrap();
    }

    #[test]
    fn features_serialise_as_the_contract_has_them() {
        let (teleporters, links) = teleporters(&[
            flag([1.0, 2.0, 3.0], 0.5, TELEPORTER_SOURCE, 0),
            flag([9.0, 2.0, 3.0], 1.5, TELEPORTER_DESTINATION, 0),
        ]);
        let f = Features {
            teleporters,
            links,
            lifts: vec![Lift {
                id: "l0".into(),
                pos: [0.0, 0.0, 1.0],
                dir: [0.0, 0.0, 1.0],
                length: 8.0,
                radius: 1.5,
                source: "bloc lift 3: render model bounds".into(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({
                "teleporters": [
                    { "id": "t0", "pos": [1.0, 2.0, 3.0], "facing": 0.5,
                      "role": "sender", "channel": 0 },
                    { "id": "t1", "pos": [9.0, 2.0, 3.0], "facing": 1.5,
                      "role": "receiver", "channel": 0 }
                ],
                "links": [ { "from": "t0", "to": "t1" } ],
                "lifts": [ { "id": "l0", "pos": [0.0, 0.0, 1.0],
                             "dir": [0.0, 0.0, 1.0], "length": 8.0,
                             "radius": 1.5,
                             "source": "bloc lift 3: render model bounds" } ]
            })
        );
        assert!(Features::default().is_empty());
    }
}
