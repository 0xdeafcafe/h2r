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

/// The file's first bytes and version.
pub const MAGIC: &[u8; 4] = b"H2BT";
pub const VERSION: u32 = 1;

/// One structure BSP's bot data.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapData {
    pub nav: NavMesh,
    pub walls: Walls,
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
    let nav = NavMesh::from_pathfinding(&pf, &Build::default());
    let mut polys: Vec<Vec<V3>> = Vec::with_capacity(bsp.surfaces.len());
    for s in 0..bsp.surfaces.len() {
        if let Some(ring) = pathfind::generate::surface_edges(bsp, s) {
            polys.push(ring.iter().map(|&(_, v)| bsp.vertices[v].point).collect());
        }
    }
    let walls = Walls::from_polygons(polys.iter().map(Vec::as_slice));
    Ok(MapData { nav, walls })
}

impl MapData {
    pub fn to_bytes(&self) -> Vec<u8> {
        let nav = self.nav.to_bytes();
        let mut o = Vec::with_capacity(nav.len() + 36 * self.walls.tris.len() + 16);
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&VERSION.to_le_bytes());
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
        o
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let u32_at = |at: usize| -> Option<u32> {
            Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
        };
        if b.get(..4)? != MAGIC || u32_at(4)? != VERSION {
            return None;
        }
        let n = u32_at(8)? as usize;
        let nav = NavMesh::from_bytes(b.get(12..12usize.checked_add(n)?)?)?;
        let mut at = 12 + n;
        let count = u32_at(at)? as usize;
        at += 4;
        if b.len() != at.checked_add(count.checked_mul(36)?)? {
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
        Some(MapData {
            nav,
            walls: Walls::new(tris),
        })
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
            nav: NavMesh::new(vec![floor, other], vec![e]),
            walls: Walls::new(vec![[[5.0, 0.0, 0.0], [5.0, 1.0, 0.0], [5.0, 0.0, 1.0]]]),
        };
        let b = d.to_bytes();
        assert_eq!(MapData::from_bytes(&b), Some(d));
        assert_eq!(MapData::from_bytes(&b[..b.len() - 1]), None);
        assert_eq!(MapData::from_bytes(&[]), None);
    }

    #[test]
    fn file_names_are_plain() {
        assert_eq!(file_name("lockout", 0), "lockout.0.bots");
        assert_eq!(file_name("../x y", 1), "___x_y.1.bots");
    }
}
