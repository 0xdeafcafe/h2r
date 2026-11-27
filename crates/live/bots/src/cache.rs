//! A map's bot data, built offline from its structure BSP's collision and
//! cached under the data root (docs/game/bots.md, stage 2): the nav mesh
//! and the walls for line of sight. Derived from the disc, so never
//! committed.

use tags::{collision::Bsp, pathfinding::PathfindingData};

use crate::los::Walls;
use crate::nav::{Build, NavMesh};

/// The loaded map's data, for every bot's mind; `None` until a map with a
/// cache file loads.
static CURRENT: std::sync::RwLock<Option<std::sync::Arc<MapData>>> = std::sync::RwLock::new(None);

/// Why the loaded map's data no longer matches its tags, once a live
/// edit wrote a tag it was built from ([`mark_stale`]).
static STALE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Exchange the active pointer. Release the retired map outside any lock
/// shared with a bot sampling a frame. New data is fresh.
pub(crate) fn replace_current(d: Option<std::sync::Arc<MapData>>) -> Option<std::sync::Arc<MapData>> {
    *STALE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    std::mem::replace(&mut *CURRENT.write().unwrap_or_else(|e| e.into_inner()), d)
}

/// A live edit (docs/editor/realtime-edits.md, "Realtime coverage") wrote
/// a tag the loaded data was built from (the structure BSP's collision):
/// the bots keep using it, and [`stale`] says why until the next map's
/// data replaces it. The first reason is kept.
pub fn mark_stale(why: impl Into<String>) {
    STALE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| why.into());
}

/// Why the loaded data is stale, if a live edit made it so.
pub fn stale() -> Option<String> {
    STALE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The loaded map's data.
pub fn current() -> Option<std::sync::Arc<MapData>> {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The file's first bytes and format version. Version 1 had no
/// [`Stamp`]; it reads as stale ([`crate::autonav::Status::OldFormat`]).
/// Version 4 added the breakable glass after the walls.
pub const MAGIC: &[u8; 4] = b"H2BT";
pub const VERSION: u32 = 4;

/// The nav generator's version. **Bump it whenever the data a map builds
/// changes**: `bots::nav`, `bots::los`, [`build`] or `crates/formats/pathfind`'s
/// generator. Every file built by another version is stale and is built
/// again (`crate::autonav`); the `generator_output_is_pinned` test in
/// `nav/tests.rs` fails when the output changes and this does not.
pub const GENERATOR: u32 = 7;

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

/// The directory of every map's bot data (`.bots`, `.vis`) under the data
/// root: `<data>/bots`. The nav worker writes there and the game loads
/// from there.
pub fn dir(data_root: &std::path::Path) -> std::path::PathBuf {
    data_root.join("bots")
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
    build_with(bsp, crate::vis::Threads::foreground())
}

pub(crate) fn build_with(bsp: &Bsp, threads: crate::vis::Threads) -> Result<MapData, String> {
    Ok(build_geometry(bsp, 0, 0, threads)?.1)
}

/// Generate the engine's pathfinding element alongside the bot mesh that
/// uses its sector indices. The element carries the owning BSP's checksum
/// and one reference per instanced-geometry placement.
pub fn build_with_pathfinding(
    bsp: &Bsp,
    instances: usize,
    checksum: u32,
) -> Result<(PathfindingData, MapData), String> {
    build_geometry(bsp, instances, checksum, crate::vis::Threads::foreground())
}

fn build_geometry(
    bsp: &Bsp,
    instances: usize,
    checksum: u32,
    threads: crate::vis::Threads,
) -> Result<(PathfindingData, MapData), String> {
    let mut tris = Vec::with_capacity(bsp.surfaces.len());
    let pf = pathfind::generate::generate_with_surfaces(
        bsp,
        instances,
        checksum,
        &pathfind::generate::Options::default(),
        |ring| Walls::fan_polygon(ring.iter().map(|&(_, v)| bsp.vertices[v].point), &mut tris),
    )
    .map_err(|e| e.to_string())?;
    let walls = Walls::new(tris).with_glass(crate::glass::panes(bsp));
    let nav = NavMesh::from_collision(&pf, &Build::default(), &walls, threads);
    let data = MapData {
        nav,
        walls,
        places: Default::default(),
    };
    Ok((pf, data))
}

/// The line-of-sight walls of `bsp` alone: every surface's ring fanned
/// into triangles, and its breakable glass. What [`build`] makes for
/// walls, without the nav mesh: a live collision edit's
/// (`crate::autonav::collision_edited`).
pub fn walls_of(bsp: &Bsp) -> Walls {
    let mut tris = Vec::with_capacity(bsp.surfaces.len());
    for s in 0..bsp.surfaces.len() {
        let ring = bsp.surface_ring(s);
        Walls::fan_polygon(ring.iter().filter_map(|&v| bsp.vertices.get(v).map(|v| v.point)), &mut tris);
    }
    Walls::new(tris).with_glass(crate::glass::panes(bsp))
}

/// The walls of `bsp` without its sky shell: every surface that reaches
/// the highest point of any surface. A structure BSP closes its world in
/// a lid at the sky height and walls rising to it (on Coagulation, 115
/// units up, with sloped surfaces joining them to the level), which is no
/// roof or wall of anything a ship flies through. Line of sight keeps them ([`walls_of`]); a vertical or aerial
/// probe (the dropship's column, `firefight-kit`'s `ship::plan`) must not.
pub fn walls_without_sky(bsp: &Bsp) -> Walls {
    /// The lid's vertices differ in the fifth decimal place.
    const SKY_TOLERANCE: f32 = 0.01;
    let rings: Vec<Vec<[f32; 3]>> = (0..bsp.surfaces.len())
        .map(|s| {
            bsp.surface_ring(s)
                .iter()
                .filter_map(|&v| bsp.vertices.get(v).map(|v| v.point))
                .collect()
        })
        .collect();
    let top = rings.iter().flatten().map(|p| p[2]).fold(f32::MIN, f32::max);
    let mut tris = Vec::with_capacity(rings.len());
    // A level whose every surface is at its top (one flat floor) has no
    // shell over it: the top is the floor, and it stays.
    let flat = rings.iter().all(|r| r.iter().any(|p| p[2] >= top - SKY_TOLERANCE));
    for ring in &rings {
        if flat || !ring.iter().any(|p| p[2] >= top - SKY_TOLERANCE) {
            Walls::fan_polygon(ring.iter().copied(), &mut tris);
        }
    }
    Walls::new(tris)
}

impl MapData {
    /// The file: magic, version, `stamp`, the mesh, the walls, the glass
    /// (a count, then per pane its breakable index, its vertex count and
    /// the vertices), the tactical places.
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
        o.extend_from_slice(&(self.walls.glass.len() as u32).to_le_bytes());
        for p in &self.walls.glass {
            o.push(p.index);
            o.push(p.verts.len().min(255) as u8);
            for v in p.verts.iter().take(255) {
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
        let panes = u32_at(at)? as usize;
        at += 4;
        let mut glass = Vec::with_capacity(panes.min(256));
        for _ in 0..panes {
            let (index, n) = (*b.get(at)?, usize::from(*b.get(at + 1)?));
            at += 2;
            let mut verts = Vec::with_capacity(n);
            for _ in 0..n {
                let mut v = [0.0f32; 3];
                for f in v.iter_mut() {
                    *f = f32::from_bits(u32_at(at)?);
                    at += 4;
                }
                verts.push(v);
            }
            glass.push(crate::glass::Pane { index, verts });
        }
        Some((
            MapData {
                nav,
                walls: Walls::new(tris).with_glass(glass),
                places: crate::places::Places::decode(b.get(at..)?)?,
            },
            stamp,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::{Kind, Poly};

    /// One invented square surface at height `z`, its four edges round it
    /// (left surface the square).
    fn square(z: f32) -> Bsp {
        use tags::collision::{Edge, Surface, Vertex};
        let mut bsp = Bsp::default();
        for (x, y) in [(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)] {
            bsp.vertices.push(Vertex { point: [x, y, z], edge: 0 });
        }
        for i in 0..4u16 {
            bsp.edges.push(Edge {
                start_vertex: i,
                end_vertex: (i + 1) % 4,
                forward_edge: (i + 1) % 4,
                reverse_edge: u16::MAX,
                left_surface: 0,
                right_surface: u16::MAX,
            });
        }
        bsp.surfaces.push(Surface { plane: 0, first_edge: 0, flags: 0, breakable_surface: u8::MAX, material: 0 });
        bsp
    }

    #[test]
    fn walls_of_a_bsp_fan_every_surface_ring() {
        let walls = walls_of(&square(1.0));
        // A square is two triangles.
        assert_eq!(walls.tris.len(), 2);
        assert!(walls.tris.iter().flatten().all(|v| v[2] == 1.0));
        // The square blocks a segment through it and not one beside it.
        assert!(!walls.clear([2.0, 2.0, 0.0], [2.0, 2.0, 2.0]));
        assert!(walls.clear([6.0, 2.0, 0.0], [6.0, 2.0, 2.0]));
    }

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
            walls: Walls::new(vec![[[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]]])
                .with_glass(vec![crate::glass::Pane {
                    index: 7,
                    verts: vec![[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]],
                }]),
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
