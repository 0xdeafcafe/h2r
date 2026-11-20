//! A map's bot data, built offline from its structure BSP's collision and
//! cached under the data root (docs/game/bots.md, stage 2): the nav mesh
//! and the walls for line of sight. Derived from the disc, so never
//! committed.

use tags::collision::Bsp;

use crate::los::{V3, Walls};
use crate::nav::{Build, NavMesh};

/// The loaded map's data, for every bot's mind; `None` until a map with a
/// cache file loads.
static CURRENT: std::sync::RwLock<Option<std::sync::Arc<MapData>>> = std::sync::RwLock::new(None);

/// Sets the loaded map's data.
pub fn set_current(d: Option<std::sync::Arc<MapData>>) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = d;
}

/// The loaded map's data.
pub fn current() -> Option<std::sync::Arc<MapData>> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The file's first bytes and format version. Version 1 had no
/// [`Stamp`]; it reads as stale ([`crate::autonav::Status::OldFormat`]).
pub const MAGIC: &[u8; 4] = b"H2BT";
pub const VERSION: u32 = 3;

/// The nav generator's version. **Bump it whenever the data a map builds
/// changes**: `bots::nav`, `bots::los`, [`build`] or `crates/formats/pathfind`'s
/// generator. Every file built by another version is stale and is built
/// again (`crate::autonav`); the `generator_output_is_pinned` test in
/// `nav/tests.rs` fails when the output changes and this does not.
pub const GENERATOR: u32 = 5;

/// The header's length: magic, format version, [`Stamp`].
pub const HEADER_LEN: usize = 20;

/// What a file was built by and from: the generator version and the
/// source map's fingerprint ([`crate::autonav::fingerprint`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub generator: u32,
    pub fingerprint: u64,
}

impl Stamp {
    /// This generator, for the map with `fingerprint`.
    pub fn now(fingerprint: u64) -> Self {
        Stamp {
            generator: GENERATOR,
            fingerprint,
        }
    }
}

/// A file's format version and, for this format, its stamp, from its
/// first [`HEADER_LEN`] bytes. `None`: not a bot data file.
pub fn read_header(b: &[u8]) -> Option<(u32, Option<Stamp>)> {
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    };
    if b.get(..4)? != MAGIC {
        return None;
    }
    let version = u32_at(4)?;
    if version != VERSION {
        return Some((version, None));
    }
    let fingerprint = u64::from_le_bytes(b.get(12..20)?.try_into().ok()?);
    Some((
        version,
        Some(Stamp {
            generator: u32_at(8)?,
            fingerprint,
        }),
    ))
}

/// One structure BSP's bot data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapData {
    pub nav: NavMesh,
    pub walls: Walls,
    pub places: crate::places::Places,
}

/// The file name for `map` (the cache's map name) and structure BSP
/// `bsp`.
pub fn file_name(map: &str, bsp: usize) -> String {
    let safe: String = map
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.{bsp}.bots")
}

/// Builds a structure BSP's data from its collision.
pub fn build(bsp: &Bsp) -> Result<MapData, String> {
    let pf = pathfind::generate::generate(bsp, 0, 0, &pathfind::generate::Options::default())
        .map_err(|e| e.to_string())?;
    let mut polys: Vec<Vec<V3>> = Vec::with_capacity(bsp.surfaces.len());
    for s in 0..bsp.surfaces.len() {
        if let Some(ring) = pathfind::generate::surface_edges(bsp, s) {
            polys.push(ring.iter().map(|&(_, v)| bsp.vertices[v].point).collect());
        }
    }
    let walls = Walls::from_polygons(polys.iter().map(Vec::as_slice));
    let nav = NavMesh::from_collision(&pf, &Build::default(), &walls);
    Ok(MapData {
        nav,
        walls,
        places: Default::default(),
    })
}

impl MapData {
    /// The file: magic, version, `stamp`, the mesh, the walls.
    pub fn to_bytes(&self, stamp: Stamp) -> Vec<u8> {
        let nav = self.nav.to_bytes();
        let mut o = Vec::with_capacity(nav.len() + 36 * self.walls.tris.len() + 28);
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&VERSION.to_le_bytes());
        o.extend_from_slice(&stamp.generator.to_le_bytes());
        o.extend_from_slice(&stamp.fingerprint.to_le_bytes());
        o.extend_from_slice(&(nav.len() as u32).to_le_bytes());
        o.extend_from_slice(&nav);
        o.extend_from_slice(&(self.walls.tris.len() as u32).to_le_bytes());
        for t in &self.walls.tris {
            for v in t {
                for f in v {
                    o.extend_from_slice(&f.to_le_bytes());
                }
            }
        }
        self.places.encode(&mut o);
        o
    }

    /// The data and its stamp; `None` for another format or a damaged
    /// file.
    pub fn from_bytes(b: &[u8]) -> Option<(Self, Stamp)> {
        let u32_at = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
        };
        let (_, stamp) = read_header(b)?;
        let stamp = stamp?;
        let h = HEADER_LEN;
        let n = u32_at(h)? as usize;
        let nav = NavMesh::from_bytes(b.get(h + 4..(h + 4).checked_add(n)?)?)?;
        let mut at = h + 4 + n;
        let count = u32_at(at)? as usize;
        at += 4;
        if b.len() < at.checked_add(count.checked_mul(36)?)? {
            return None;
        }
        let mut tris = Vec::with_capacity(count);
        for _ in 0..count {
            let mut t = [[0.0f32; 3]; 3];
            for v in &mut t {
                for f in v.iter_mut() {
                    *f = f32::from_bits(u32_at(at)?);
                    at += 4;
                }
            }
            tris.push(t);
        }
        Some((
            MapData {
                nav,
                walls: Walls::new(tris),
                places: crate::places::Places::decode(&b[at..])?,
            },
            stamp,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::{Kind, Poly};

    #[test]
    fn the_file_round_trips_and_refuses_damage() {
        let floor = Poly::new(vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0]]);
        let other = Poly::new(vec![[2.0, 0.0, 0.0], [4.0, 0.0, 0.0], [2.0, 2.0, 0.0]]);
        let e = crate::nav::Edge {
            from: 0,
            to: 1,
            a: [2.0, 0.0, 0.0],
            b: [2.0, 2.0, 0.0],
            kind: Kind::Walk,
        };
        let d = MapData {
            places: Default::default(),
            nav: NavMesh::new(vec![floor, other], vec![e]),
            walls: Walls::new(vec![[[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]]]),
        };
        let stamp = Stamp::now(0x0123_4567_89ab_cdef);
        let b = d.to_bytes(stamp);
        assert_eq!(MapData::from_bytes(&b), Some((d, stamp)));
        assert_eq!(read_header(&b), Some((VERSION, Some(stamp))));
        assert_eq!(MapData::from_bytes(&b[..b.len() - 1]), None);
        assert_eq!(MapData::from_bytes(&[]), None);
    }

    #[test]
    fn a_version_1_file_is_another_format() {
        // Version 1: magic, version, then the mesh's length; no stamp.
        let mut b = MAGIC.to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(read_header(&b), Some((1, None)));
        assert_eq!(MapData::from_bytes(&b), None);
        assert_eq!(read_header(b"NOPE\x02\0\0\0"), None);
    }

    #[test]
    fn file_names_are_plain() {
        assert_eq!(file_name("lockout", 0), "lockout.0.bots");
        assert_eq!(file_name("../x y", 1), "___x_y.1.bots");
    }
}
