//! Ground firing positions use the walkable sector and its floor normal.

use pathfind::geom::newell;
use tags::pathfinding::PathfindingData;

use crate::h1::FiringPosition;

pub(crate) fn location(mesh: &PathfindingData, f: &FiringPosition) -> Option<(u32, [f32; 2])> {
    let [x, y, _] = f.position;
    let sector = mesh.sector_for_surface(f.surface as usize, x, y)?;
    let vertices = mesh.sector_polygon(sector as usize)?;
    let floor = vertices
        .into_iter()
        .map(|i| mesh.vertices.get(i).copied())
        .collect::<Option<Vec<_>>>()?;
    let [nx, ny, nz] = newell(&floor);
    (nz > 0.0).then(|| (sector, [ny.atan2(nx), nz.atan2(nx.hypot(ny))]))
}
