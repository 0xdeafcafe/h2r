//! A level's weather for the 3D viewer: where it rains or snows, in
//! `map.glb`'s `asset.extras.weather` (docs/live/live-art.md, "Weather").
//!
//! Of a structure BSP only these fields are read, the layout the disc
//! agrees with (`geometry_from_the_disc` reports every level's weather):
//!
//! - the weather palette at `sbsp+0x84`, 0x88 bytes each: a 32-byte name,
//!   then the `weat` tag reference (ID at +0x24);
//! - the weather polyhedra at `sbsp+0x8c`, 0x18 bytes each: a bounding
//!   sphere (centre, radius), then a block of planes (normal, distance;
//!   a point is inside when `n·p <= d` for every plane). They are the
//!   places the weather is kept out of (under roofs and overhangs);
//! - the clusters at `sbsp+0x9c`, 0xb0 bytes each: bounds at +0x54 (x, y
//!   and z, each min then max) and the weather palette index (a short, -1
//!   for none) at +0x76.
//!
//! The weather's kind (rain, snow, or slow drifting motes for the rest:
//! mist, spores, debris) is read from words in the palette entry's and
//! the `weat` tag's names.

use anyhow::{Context, Result};
use map::{FourCC, TagId};
use serde::{Deserialize, Serialize};
use tags::geometry::TagView;

use crate::live_geometry::{MapReader, scenario_bsps, to_gltf};

const WEAT: FourCC = FourCC(*b"weat");

/// `sbsp` fields (module docs).
pub const BSP_WEATHER_PALETTE: usize = 0x84;
pub const BSP_WEATHER_POLYHEDRA: usize = 0x8c;
pub const BSP_CLUSTERS: usize = 0x9c;
pub const BSP_HEADER: usize = 0xa4;
pub const PALETTE_STRIDE: usize = 0x88;
pub const PALETTE_WEATHER: usize = 0x24;
pub const POLYHEDRON_STRIDE: usize = 0x18;
pub const POLYHEDRON_PLANES: usize = 0x10;
pub const PLANE_STRIDE: usize = 0x10;
pub const CLUSTER_STRIDE: usize = 0xb0;
pub const CLUSTER_BOUNDS: usize = 0x54;
pub const CLUSTER_WEATHER: usize = 0x76;

/// One weather system of a level (`asset.extras.weather[]`), in the glTF
/// frame of `map.glb` (y up).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weather {
    /// The palette entry's name, else the `weat` tag's file name.
    pub name: String,
    /// `rain`, `snow` or `motes` ([`weather_kind`]).
    pub kind: String,
    /// How thick it falls: 0.5 light, 1 normal, 2 heavy.
    pub amount: f32,
    /// Where it falls: the bounds of the clusters it is set on (min x, y,
    /// z, then max x, y, z).
    pub boxes: Vec<[f32; 6]>,
    /// Where it is kept out: convex volumes, each a list of planes
    /// (normal x, y, z, distance; inside when `n·p <= d` for all).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<Vec<[f32; 4]>>,
}

fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// What falls, from words in its names: `snow`; `rain` (also monsoon,
/// drizzle, storm); else `motes`.
pub fn weather_kind(names: &[&str]) -> &'static str {
    let words: Vec<String> = names
        .iter()
        .flat_map(|n| {
            n.rsplit(['\\', '/'])
                .next()
                .unwrap_or(n)
                .split(|c: char| !c.is_ascii_alphabetic())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>()
        })
        .collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(&w.as_str()));
    if has(&["snow", "snowfall", "blizzard"]) {
        "snow"
    } else if has(&["rain", "monsoon", "drizzle", "storm"]) {
        "rain"
    } else {
        "motes"
    }
}

/// How thick, from words in its names: `light` 0.5, `heavy` or `monsoon` 2.
pub fn weather_amount(names: &[&str]) -> f32 {
    let lower: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
    if lower
        .iter()
        .any(|n| n.contains("heavy") || n.contains("monsoon"))
    {
        2.0
    } else if lower
        .iter()
        .any(|n| n.contains("light") || n.contains("misty"))
    {
        0.5
    } else {
        1.0
    }
}

/// A box from game bounds (x, y, z ranges) in the glTF frame.
fn gltf_box(b: [f32; 6]) -> [f32; 6] {
    let lo = to_gltf([b[0], b[2], b[4]]);
    let hi = to_gltf([b[1], b[3], b[5]]);
    [
        lo[0].min(hi[0]),
        lo[1].min(hi[1]),
        lo[2].min(hi[2]),
        lo[0].max(hi[0]),
        lo[1].max(hi[1]),
        lo[2].max(hi[2]),
    ]
}

/// A BSP's weather (`view` over the BSP tag, `header` its first
/// [`BSP_HEADER`] bytes), the palette entries' `weat` tag names by `name`.
/// Entries without a tag or without clusters are left out.
pub fn bsp_weather(
    view: &TagView<'_>,
    header: &[u8],
    name: impl Fn(TagId) -> Option<String>,
) -> Result<Vec<Weather>> {
    let palette = view.block(header, BSP_WEATHER_PALETTE, PALETTE_STRIDE)?;
    if palette.is_empty() {
        return Ok(Vec::new());
    }
    let mut boxes: Vec<Vec<[f32; 6]>> = vec![Vec::new(); palette.len()];
    for c in view.block(header, BSP_CLUSTERS, CLUSTER_STRIDE)? {
        let w = c
            .get(CLUSTER_WEATHER..CLUSTER_WEATHER + 2)
            .map(|r| i16::from_le_bytes([r[0], r[1]]))
            .context("cluster")?;
        let Some(list) = usize::try_from(w).ok().and_then(|w| boxes.get_mut(w)) else {
            continue;
        };
        let b: [f32; 6] =
            std::array::from_fn(|k| f32_at(c, CLUSTER_BOUNDS + 4 * k).unwrap_or(f32::NAN));
        if b.iter().all(|v| v.is_finite()) && b[0] <= b[1] && b[2] <= b[3] && b[4] <= b[5] {
            list.push(gltf_box(b));
        }
    }
    let mut exclude = Vec::new();
    for p in view.block(header, BSP_WEATHER_POLYHEDRA, POLYHEDRON_STRIDE)? {
        let planes: Vec<[f32; 4]> = view
            .block(p, POLYHEDRON_PLANES, PLANE_STRIDE)?
            .into_iter()
            .filter_map(|e| {
                let n = to_gltf([f32_at(e, 0)?, f32_at(e, 4)?, f32_at(e, 8)?]);
                let d = f32_at(e, 0xc)?;
                (n.iter().chain([&d]).all(|v| v.is_finite())).then_some([n[0], n[1], n[2], d])
            })
            .collect();
        if planes.len() >= 4 {
            exclude.push(planes);
        }
    }
    let mut out = Vec::new();
    for (entry, boxes) in palette.into_iter().zip(boxes) {
        let id = u32_at(entry, PALETTE_WEATHER)
            .map(TagId)
            .unwrap_or(TagId::NULL);
        if id == TagId::NULL || boxes.is_empty() {
            continue;
        }
        let Some(tag_name) = name(id) else { continue };
        let own = entry[..0x20].split(|&b| b == 0).next().unwrap_or_default();
        let own = String::from_utf8_lossy(own).trim().to_string();
        let file = tag_name
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(&tag_name)
            .to_string();
        let names = [own.as_str(), tag_name.as_str()];
        out.push(Weather {
            name: if own.is_empty() { file } else { own.clone() },
            kind: weather_kind(&names).into(),
            amount: weather_amount(&names),
            boxes,
            exclude: exclude.clone(),
        });
    }
    Ok(out)
}

/// The level's weather: every BSP's, in order. A BSP whose weather cannot
/// be read adds none.
pub fn read_level_weather(map: &mut MapReader) -> Result<Vec<Weather>> {
    let mut out = Vec::new();
    for bsp in scenario_bsps(map)? {
        if bsp == TagId::NULL {
            continue;
        }
        let Ok(tag) = map.tag(bsp).cloned() else {
            continue;
        };
        let Some(location) = tag.location else {
            continue;
        };
        let Ok(bytes) = map.read_tag_bytes(bsp) else {
            continue;
        };
        let view = TagView::new(&bytes, location.address);
        let Ok(header) = view.at(0, BSP_HEADER) else {
            continue;
        };
        let names = |id: TagId| {
            map.tag(id)
                .ok()
                .filter(|t| t.group == WEAT)
                .map(|t| t.name.clone().unwrap_or_default())
        };
        if let Ok(w) = bsp_weather(&view, header, names) {
            out.extend(w);
        }
    }
    Ok(out)
}
