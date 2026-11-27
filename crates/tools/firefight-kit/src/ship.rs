//! The dropship a kit carries (docs/game/firefight.md, "The kit", and
//! docs/game/rules-world-api.md, "Vehicles and drop ships"): the ship's
//! tag, one parked placement named `ff_ship`, and for each drop zone a
//! point set `ff_path_NN` the host flies the ship along: run-in, hover,
//! run-out.
//!
//! The ship is the Phantom, copied in by the kit's stage two from a donor
//! campaign map (`transplant`), or, on a map built from Halo 1's campaign,
//! the Covenant dropship the map already holds. A zone gets a path only
//! when collision rays find its column clear ([`plan`]): a zone under a
//! roof, or one the ship cannot approach, spawns its squads on foot.
use std::io::{Read, Seek};

use anyhow::{Context, Result, ensure};
use bots::los::Walls;
use h1campaign::ai::h2::{Point, PointSet, ScenarioAi, Zone, layout};
use h1campaign::scenario::{OBJECT_NAME, OBJECT_NAMES, PALETTE, UNIQUE_ID_HIGH};
use map::{Map, TagId, build::{Block, TagSet}, FourCC};
use pathfind::geom::V3;
use scenario_edit::scene::metadata_block;
use firefight::ShipKind;
use serde::Serialize;

/// The parked ships' object names are `ff_ship_<kind>` ([`ShipKind::name`]),
/// which the guest reads the placements by.
pub const SHIP_PREFIX: &str = "ff_ship_";

/// The object name of a kind's parked ship.
pub fn ship_name(kind: ShipKind) -> String {
    format!("{SHIP_PREFIX}{}", kind.name())
}

/// A kind of ship's size, read from its `vehi` tag: how far its body
/// reaches round its path (the object's bounding radius), how many seats it
/// has (the vehicle's seat block) and how high its root must hover for the
/// hull to clear the ground (the render model's lowest position under the
/// root). A path is clear for a kind when rays that
/// far to each side of it are clear too.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Dims {
    pub radius: f32,
    pub seats: usize,
    pub hover: f32,
    /// Read from the tag; false for the defaults of a tag too short or too
    /// plain to say (an invented one in a test).
    pub from_tag: bool,
}

impl Dims {
    /// What a kind is taken to be when its tag gives none. The report says
    /// so ([`ShipReport::dims`]).
    pub const fn default_for(kind: ShipKind) -> Self {
        match kind {
            ShipKind::Phantom => Self { radius: 5.0, seats: 8, hover: DROP_HEIGHT, from_tag: false },
            ShipKind::Pelican => Self { radius: 8.0, seats: 6, hover: 4.0, from_tag: false },
        }
    }
}

const VEHI_SIZE: usize = 0x300;
const OBJE_BOUNDING_RADIUS: usize = 0x4;
const VEHI_SEATS: usize = 0x1c8;
const SEAT_STRIDE: usize = 0xb0;
const OBJE_MODEL: usize = 0x34;
const HLMT_RENDER_MODEL: usize = 0x0;
const MODE_COMPRESSION: usize = 0x14;
const MODE_COMPRESSION_STRIDE: usize = 0x38;

/// The size the `vehi` tag `path` of `map` gives, none when the map has no
/// such tag or it is too short to hold the fields.
pub(crate) fn dims_in<R: Read + Seek>(map: &mut Map<R>, path: &str) -> Option<Dims> {
    use map::TagQuery;
    use tags::geometry::TagView;
    let id = map.tag(TagQuery::NameInGroup(path, VEHI)).ok()?.id;
    let address = map.tag(id).ok()?.location?.address;
    let bytes = map.read_tag_bytes(id).ok()?;
    let v = TagView::new(&bytes, address);
    let root = v.at(0, VEHI_SIZE).ok()?;
    let f = |at: usize| f32::from_le_bytes(root[at..at + 4].try_into().unwrap());
    let seats = v.block(root, VEHI_SEATS, SEAT_STRIDE).ok()?.len();
    let radius = f(OBJE_BOUNDING_RADIUS);
    // The root sits this high over the floor when the hull's underside
    // (the render model's lowest position) just touches it.
    let datum = |bytes: &[u8], at: usize| TagId(u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()));
    let hlmt = datum(root, OBJE_MODEL + 4);
    let mut tag_root = |id: TagId, size: usize| -> Option<(Vec<u8>, u32)> {
        let address = map.tag(id).ok()?.location?.address;
        let bytes = map.read_tag_bytes(id).ok()?;
        (bytes.len() >= size).then_some((bytes, address))
    };
    let (hbytes, _) = tag_root(hlmt, HLMT_RENDER_MODEL + 8)?;
    let (mbytes, maddress) = tag_root(datum(&hbytes, HLMT_RENDER_MODEL + 4), MODE_COMPRESSION + 8)?;
    let mv = TagView::new(&mbytes, maddress);
    let mroot = mv.at(0, MODE_COMPRESSION + 8).ok()?;
    let bounds = mv.block(mroot, MODE_COMPRESSION, MODE_COMPRESSION_STRIDE).ok()?;
    let low = f32::from_le_bytes(bounds.first()?[0x10..0x14].try_into().unwrap());
    let hover = (-low).max(0.0);
    (radius > 0.0 && seats > 0).then_some(Dims { radius, seats, hover, from_tag: true })
}

/// The point of a path a kind hovers at: `hover_<kind>`.
pub fn hover_name(kind: ShipKind) -> String {
    format!("hover_{}", kind.name())
}
/// A zone's path is the point set `ff_path_NN`, NN the zone's number
/// (`ff_drop_NN`); a zone without one has no point set.
pub const PATH_PREFIX: &str = "ff_path_";
/// The first and last points of a path; between them one `hover_<kind>`
/// for each kind the path is cleared for.
pub const RUN_IN: &str = "run_in";
pub const RUN_OUT: &str = "run_out";
/// The retail Pelican's `vehi` tag, on the campaign maps that carry it.
pub const PELICAN: &str = r"objects\vehicles\pelican\pelican";
/// The retail Phantom's `vehi` tag: on 03a (Outskirts) and the other
/// campaign maps that carry it (docs/game/notes/2026-11-26-firefight-class-census.md).
pub const PHANTOM: &str = r"objects\vehicles\phantom\phantom";
/// Halo 1's Covenant dropship (the Spirit) as a map built from the
/// campaign keeps it ("a scripted set piece", `h1campaign::h2assets`),
/// with and without the `objects` prefix.
pub const SPIRIT: [&str; 2] = [
    r"vehicles\c_dropship\c_dropship",
    r"objects\vehicles\c_dropship\c_dropship",
];

const VEHI: FourCC = FourCC(*b"vehi");
/// The ship hovers this far above the zone's mean while it unloads.
pub const DROP_HEIGHT: f32 = 6.0;
/// The run-in and run-out fly this far above the tallest collision.
const SKY_CLEARANCE: f32 = 10.0;
/// And start this far outside the collision's horizontal bounds.
const RUN_MARGIN: f32 = 20.0;
/// The column's ray starts this far above the zone's floor.
const FLOOR_LIFT: f32 = 0.5;
/// Horizontal run lengths tried after the full reach, longest first.
const STEEP_REACH: [f32; 3] = [80.0, 50.0, 30.0];
/// Run directions tried for a zone, evenly round the compass.
const DIRECTIONS: usize = 8;
/// The placement's unique ID low word: past the converter's running count.
const UNIQUE_LOW: u32 = 0xff5e;
// A second ship's is the next (the pods' start at 0xff60).

/// Which ship a kit carries (`h2 firefight kit MAP --dropship`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Dropship {
    /// The Spirit when the map holds it, else the Phantom when a donor
    /// has it, else none: a map no ship can be had for builds as it did.
    #[default]
    Auto,
    /// The Phantom, from the map or a donor; a build without one fails.
    Phantom,
    /// The map's own Halo 1 dropship; a build without one fails.
    Spirit,
    /// No ship, no placement, no paths.
    None,
}

impl Dropship {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "auto" => Self::Auto,
            "phantom" => Self::Phantom,
            "spirit" => Self::Spirit,
            "none" => Self::None,
            _ => return None,
        })
    }
}

/// What a build says of the dropship (`Package::ship`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShipReport {
    pub kind: ShipKind,
    pub kept: bool,
    /// The ship's tag name when kept.
    pub tag: String,
    /// Tags the transplant copied in (0 when the map held the ship).
    pub tags: usize,
    /// Tag memory the ship cost: its copied tags, placement, palette row,
    /// object name and point sets.
    pub bytes: usize,
    /// Zones with a path.
    pub paths: usize,
    /// Why it was cut.
    pub why: Option<String>,
    /// The size the kit planned its paths for, when kept.
    pub dims: Option<Dims>,
    /// Where the hover came from when it is the retail campaign's.
    pub hover_source: Option<String>,
}

impl ShipReport {
    pub(crate) fn cut(why: impl Into<String>) -> Self {
        Self::cut_kind(ShipKind::Phantom, why)
    }

    pub(crate) fn cut_kind(kind: ShipKind, why: impl Into<String>) -> Self {
        Self {
            kind,
            kept: false,
            tag: String::new(),
            tags: 0,
            bytes: 0,
            paths: 0,
            why: Some(why.into()),
            dims: None,
            hover_source: None,
        }
    }
}

impl std::fmt::Display for ShipReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.kind {
            ShipKind::Phantom => "dropship",
            ShipKind::Pelican => "pelican",
        };
        if self.kept {
            write!(
                f,
                "{label}: kept ({}, {} tags, {} bytes, {} zone paths{})",
                self.tag,
                self.tags,
                self.bytes,
                self.paths,
                match self.dims {
                    Some(d) => {
                        let size = if d.from_tag {
                            format!("reach {}, {} seats from the tag", d.radius, d.seats)
                        } else {
                            format!("the tag gives no size, so reach {}, {} seats are assumed", d.radius, d.seats)
                        };
                        let hover = match self.hover_source.as_deref() {
                            Some(source) => format!("hover {} from {source}", d.hover),
                            None if d.from_tag => format!("hover {} from the tag's hull", d.hover),
                            None => format!("hover {} assumed", d.hover),
                        };
                        format!("; {size}, {hover}")
                    }
                    None => String::new(),
                }
            )
        } else {
            write!(f, "{label}: cut ({})", self.why.as_deref().unwrap_or("not built"))
        }
    }
}

/// Where the build gets its ship.
pub(crate) enum Pick {
    /// A tag the map has.
    Have(TagId, String),
    /// A tag to copy from a donor, by name.
    Copy(&'static str),
}

/// The ship `want` asks for on `set`, or an error when it cannot be had
/// there. [`Dropship::None`] is not asked here.
pub(crate) fn pick(set: &TagSet, want: Dropship) -> Result<Pick> {
    let spirit = SPIRIT
        .iter()
        .find_map(|p| set.find(VEHI, p).map(|id| (id, (*p).to_owned())));
    let phantom = set.find(VEHI, PHANTOM).map(|id| (id, PHANTOM.to_owned()));
    let have = match want {
        Dropship::None => None,
        Dropship::Spirit => Some(spirit.context(
            "--dropship spirit needs the Halo 1 Covenant dropship in the map: build it from a Halo 1 campaign map",
        )?),
        Dropship::Phantom => phantom,
        Dropship::Auto => spirit.or(phantom),
    };
    Ok(match have {
        Some((id, name)) => Pick::Have(id, name),
        None => Pick::Copy(PHANTOM),
    })
}

/// The Pelican: the map's own, else a donor's copy.
pub(crate) fn pick_pelican(set: &TagSet) -> Pick {
    match set.find(VEHI, PELICAN) {
        Some(id) => Pick::Have(id, PELICAN.to_owned()),
        None => Pick::Copy(PELICAN),
    }
}

/// Whether the rays from `a` to `b`, and the same rays moved `radius` to
/// each side, are all clear: the segment a ship that wide can fly.
fn clear_wide(walls: &Walls, a: V3, b: V3, radius: f32) -> bool {
    if radius <= 0.0 {
        return walls.clear(a, b);
    }
    [[0.0, 0.0], [radius, 0.0], [-radius, 0.0], [0.0, radius], [0.0, -radius]]
        .iter()
        .all(|o| walls.clear([a[0] + o[0], a[1] + o[1], a[2]], [b[0] + o[0], b[1] + o[1], b[2]]))
}

/// A ship's way into a zone: the three points of its path, in flying order,
/// and the yaw it flies at (radians, the run-in's direction of travel).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Run {
    pub points: [V3; 3],
    pub yaw: f32,
    /// How far over the zone's mean the hover point was planned.
    pub lift: f32,
}

/// Each zone's run, none where the collision leaves no clear column.
///
/// A zone's point is its first area's mean. The ship hovers
/// `lift` above it (a kind's own hover is set when the path is written,
/// [`path_sets`]). A zone gets a run only when
/// - a ray from just above its floor to the sky (the tallest collision plus
///   [`SKY_CLEARANCE`]) is clear, so no roof is over it, the ship's reach
///   is clear from the hover point up, and
/// - for one of eight compass directions, the ray from the run-in down to the hover point and the ray from
///   the hover point up to the run-out are clear.
///
/// Run-in and run-out lie on opposite sides at the sky height, outside the
/// collision's horizontal bounds by [`RUN_MARGIN`]. Directions are tried
/// from the one pointing away from the map's middle, so the same zone on
/// the same map always gets the same run.
pub(crate) fn plan(walls: &Walls, zones: &[Zone], radius: f32, lift: f32) -> Vec<Option<Run>> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in walls.tris.iter().flatten() {
        for j in 0..3 {
            lo[j] = lo[j].min(v[j]);
            hi[j] = hi[j].max(v[j]);
        }
    }
    if !lo.iter().chain(&hi).all(|x| x.is_finite()) {
        return vec![None; zones.len()];
    }
    let middle = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let half = libm::hypotf(hi[0] - lo[0], hi[1] - lo[1]) * 0.5;
    let sky = hi[2] + SKY_CLEARANCE;
    zones
        .iter()
        .map(|zone| {
            let mean = zone.areas.first()?.mean;
            if !mean.iter().all(|x| x.is_finite()) {
                return None;
            }
            let hover = [mean[0], mean[1], mean[2] + lift];
            // The floor's own column is a thin ray (the squad stands on
            // ground that may be beside a rise); the ship's body, its full
            // reach, is checked from the hover point up.
            if hover[2] >= sky
                || !walls.clear([mean[0], mean[1], mean[2] + FLOOR_LIFT], [mean[0], mean[1], sky])
                || !clear_wide(walls, hover, [hover[0], hover[1], sky], radius)
            {
                return None;
            }
            // Far enough that the run-in and run-out start outside.
            let reach = half
                + libm::hypotf(mean[0] - middle[0], mean[1] - middle[1])
                + RUN_MARGIN;
            let away = libm::atan2f(mean[1] - middle[1], mean[0] - middle[0]);
            // The full reach first, then steeper dives for a zone in a
            // canyon, where a long shallow run meets the cliffs.
            let reaches = [reach, STEEP_REACH[0], STEEP_REACH[1], STEEP_REACH[2]];
            reaches.into_iter().find_map(|reach| {
                (0..DIRECTIONS).find_map(|k| {
                    let angle = away + k as f32 * std::f32::consts::TAU / DIRECTIONS as f32;
                    let (dy, dx) = libm::sincosf(angle);
                    let end = |sign: f32| [hover[0] + sign * dx * reach, hover[1] + sign * dy * reach, sky];
                    let (run_in, run_out) = (end(1.0), end(-1.0));
                    (clear_wide(walls, run_in, hover, radius) && clear_wide(walls, hover, run_out, radius)).then_some(Run {
                        points: [run_in, hover, run_out],
                        yaw: libm::atan2f(-dy, -dx),
                        lift,
                    })
                })
            })
        })
        .collect()
}

/// Whether each zone's run (the widest kept ship's) is clear for a ship of
/// `dims`: its run-in down to the zone's hover point at the kind's own
/// height and up to the run-out, rays cast the kind's reach to each side.
pub(crate) fn recheck(walls: &Walls, runs: &[Option<Run>], dims: &Dims) -> Vec<bool> {
    runs.iter()
        .map(|run| {
            run.is_some_and(|run| {
                let mut hover = run.points[1];
                hover[2] += dims.hover - run.lift;
                clear_wide(walls, run.points[0], hover, dims.radius)
                    && clear_wide(walls, hover, run.points[2], dims.radius)
            })
        })
        .collect()
}

/// A point set `ff_path_NN` for each zone with a run: `run_in`, a
/// `hover_<kind>` for each of `kinds`, `run_out`.
pub(crate) fn path_sets(runs: &[Option<Run>], bsp: i16, kinds: &[(ShipKind, Dims, Vec<bool>)]) -> Vec<PointSet> {
    let mut sets = Vec::new();
    for (zone, run) in runs.iter().enumerate() {
        let Some(run) = run else { continue };
        sets.push(PointSet {
            name: format!("{PATH_PREFIX}{zone:02}"),
            points: {
                let at = |name: String, position: V3| Point {
                    name,
                    position,
                    reference_frame: -1,
                    // No collision surface: the points are in the air.
                    surface: u32::MAX,
                    facing: [run.yaw, 0.0],
                };
                let mut points = vec![at(RUN_IN.to_owned(), run.points[0])];
                for (kind, dims, valid) in kinds {
                    if !valid.get(zone).copied().unwrap_or(false) {
                        continue;
                    }
                    let mut hover = run.points[1];
                    hover[2] += dims.hover - run.lift;
                    points.push(at(hover_name(*kind), hover));
                }
                points.push(at(RUN_OUT.to_owned(), run.points[2]));
                points
            },
            bsp,
            manual_reference_frame: -1,
            flags: 0,
        });
    }
    // A zone no kept kind can use has no path.
    sets.retain(|s| s.points.len() > 2);
    sets
}

/// The scenario blocks a ship adds, `(field, count, block)`.
pub(crate) struct Authored {
    pub fields: Vec<(usize, u32, Block)>,
    /// Zones with a path.
    pub paths: usize,
}

fn name32(name: &str, size: usize) -> Block {
    let mut b = Block::zeroed(size);
    let n = name.len().min(31);
    b.bytes[..n].copy_from_slice(&name.as_bytes()[..n]);
    b
}

/// Author each ship's placement, `ff_ship_<kind>`, parked at the first
/// run's run-in with its yaw, and a point set `ff_path_NN` for each zone
/// with a run, on the ships' `vehi` tags. The base keeps its own vehicles,
/// names and point sets; one of ours already there is refused. `bsp` is the
/// BSP the runs were planned on. None when no zone has a run or no ship is
/// kept.
pub(crate) fn author<R: Read + Seek>(
    map: &mut Map<R>,
    ships: &[(ShipKind, TagId)],
    runs: &[Option<Run>],
    bsp: i16,
    sizes: &[(ShipKind, Dims, Vec<bool>)],
) -> Result<Option<Authored>> {
    let (Some(first), false) = (runs.iter().flatten().next(), ships.is_empty()) else {
        return Ok(None);
    };
    let scenario = map.scenario().context("map has no scenario")?.id;
    let root = map.read_tag_bytes(scenario)?;
    let (placements_at, palette_at, stride) = tags::object::OBJECT_TYPES[1]
        .placement
        .context("vehicles have no placement block")?;
    let (placements_at, palette_at, stride) = (usize::from(placements_at), usize::from(palette_at), usize::from(stride));
    let names = metadata_block(map, &root, OBJECT_NAMES as usize, OBJECT_NAME)?;
    ensure!(
        !names.iter().any(|n| n.starts_with(SHIP_PREFIX.as_bytes())),
        "the base already has objects named {SHIP_PREFIX}<kind>"
    );
    let mut palette = metadata_block(map, &root, palette_at, PALETTE)?;
    let placements = metadata_block(map, &root, placements_at, stride)?;
    let index = |len: usize, what: &str| {
        i16::try_from(len).map_err(|_| anyhow::anyhow!("{what} index overflow"))
    };
    let had = palette.len();
    let mut name_bytes = names.concat();
    let mut place_bytes = placements.concat();
    let (mut name_count, mut place_count) = (names.len(), placements.len());
    let mut added_rows: Vec<(usize, TagId)> = Vec::new();
    for (n, (kind, ship)) in ships.iter().enumerate() {
        let row = match palette.iter().position(|r| r[4..8] == ship.0.to_le_bytes()) {
            Some(row) => row,
            None => {
                let mut entry = Block::zeroed(PALETTE);
                entry.tag_ref(0, *ship);
                palette.push(entry.bytes);
                added_rows.push((palette.len() - 1, *ship));
                palette.len() - 1
            }
        };
        // The object name, then the placement that carries its index.
        let mut name_row = name32(&ship_name(*kind), OBJECT_NAME);
        name_row.put_u16(0x20, 1);
        name_row.put_u16(0x22, index(place_count, "placement")? as u16);
        name_bytes.extend(&name_row.bytes);

        let mut place = Block::zeroed(stride);
        place.put_u16(0, index(row, "palette")? as u16);
        place.put_u16(2, index(name_count, "object name")? as u16);
        // Placed at the start: parked, waiting for its first squad.
        place.put_u32(4, 0);
        let start = first.points[0];
        for (j, v) in [start[0], start[1], start[2], first.yaw, 0.0, 0.0].into_iter().enumerate() {
            place.put_f32(8 + 4 * j, v);
        }
        place.put_u32(0x28, UNIQUE_ID_HIGH << 16 | (UNIQUE_LOW + n as u32));
        place.put_u16(0x2c, 0xffff);
        place.bytes[0x2e] = 1;
        place.bytes[0x2f] = 1;
        place.put_u16(0x32, 0xffff);
        place_bytes.extend(&place.bytes);
        name_count += 1;
        place_count += 1;
    }
    let mut fields: Vec<(usize, u32, Block)> = vec![
        (OBJECT_NAMES as usize, name_count as u32, Block::new(name_bytes)),
        (placements_at, place_count as u32, Block::new(place_bytes)),
    ];
    if palette.len() > had {
        let mut block = Block::new(palette.concat());
        for (row, tag) in added_rows {
            block.tag_ref((row * PALETTE) as u32, tag);
        }
        fields.push((palette_at, palette.len() as u32, block));
    }

    let mut sets = ScenarioAi::read_map(map)
        .map_err(|e| anyhow::anyhow!("reading the scenario's point sets: {e}"))?
        .point_sets;
    ensure!(
        !sets.iter().any(|s| s.name.starts_with(PATH_PREFIX)),
        "the base already has {PATH_PREFIX}NN point sets"
    );
    let made = path_sets(runs, bsp, sizes);
    let paths = made.len();
    sets.extend(made);
    let authored = ScenarioAi {
        point_sets: sets,
        ..Default::default()
    }
    .blocks();
    let (at, count, block) = authored
        .fields
        .into_iter()
        .find(|(at, _, _)| *at as usize == layout::SCRIPTING_DATA.0)
        .context("no scripting data authored for the point sets")?;
    fields.push((at as usize, count, block));
    Ok(Some(Authored { fields, paths }))
}

/// How high the retail campaign flies each kind of ship over the floor when
/// it holds a drop: the heights of the ships' `_facing` waypoints (the
/// points a command script flies to and holds at, `cs_fly_to_and_face`) in
/// the scenarios' point sets named for the ship, over the collision floor
/// straight below each point. Each sample says where it came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HoverSurvey {
    pub phantom: Vec<HoverSample>,
    pub pelican: Vec<HoverSample>,
}

/// One waypoint the survey measured.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HoverSample {
    pub map: String,
    pub set: String,
    pub point: String,
    /// The point's height over the floor below it.
    pub height: f32,
}

/// A ray goes this far down looking for the floor.
const FLOOR_REACH: f32 = 60.0;

impl HoverSurvey {
    /// The median height of a kind's samples, and how many there are.
    pub fn median(&self, kind: ShipKind) -> Option<(f32, usize)> {
        let samples = match kind {
            ShipKind::Phantom => &self.phantom,
            ShipKind::Pelican => &self.pelican,
        };
        let mut heights: Vec<f32> = samples.iter().map(|s| s.height).collect();
        heights.sort_by(f32::total_cmp);
        let n = heights.len();
        (n > 0).then(|| {
            let mid = if n % 2 == 1 { heights[n / 2] } else { (heights[n / 2 - 1] + heights[n / 2]) / 2.0 };
            (mid, n)
        })
    }

    /// The maps the samples came from, for the report.
    pub fn maps(&self, kind: ShipKind) -> String {
        let samples = match kind {
            ShipKind::Phantom => &self.phantom,
            ShipKind::Pelican => &self.pelican,
        };
        let mut by_map: Vec<(&str, usize)> = Vec::new();
        for s in samples {
            match by_map.iter_mut().find(|(m, _)| *m == s.map) {
                Some((_, n)) => *n += 1,
                None => by_map.push((&s.map, 1)),
            }
        }
        by_map.iter().map(|(m, n)| format!("{m} {n}")).collect::<Vec<_>>().join(", ")
    }

    /// Add the ships' waypoints of a campaign map's scenario.
    pub fn add_map<R: Read + Seek>(&mut self, name: &str, map: &mut Map<R>) -> Result<()> {
        use h1campaign::scenario::{BSP, BSPS};
        use tags::{collision::Bsp, geometry::TagView};
        let sets = ScenarioAi::read_map(map)
            .map_err(|e| anyhow::anyhow!("{name}: reading the point sets: {e}"))?
            .point_sets;
        let named = |set: &PointSet| {
            [ShipKind::Phantom, ShipKind::Pelican]
                .into_iter()
                .find(|k| set.name.contains(k.name()))
        };
        if !sets.iter().any(|s| named(s).is_some()) {
            return Ok(());
        }
        let scenario = map.scenario().context("map has no scenario")?.id;
        let scnr = map.read_tag_bytes(scenario)?;
        let mut floors: Vec<Option<Walls>> = Vec::new();
        for row in metadata_block(map, &scnr, BSPS as usize, BSP)? {
            let id = TagId(u32::from_le_bytes(row[0x14..0x18].try_into().unwrap()));
            let loc = map.tag(id)?.location.context("BSP missing its payload")?;
            let bytes = map.read_tag_bytes(id)?;
            let v = TagView::new(&bytes, loc.address);
            let root = v.at(0, 0x148)?;
            let coll = v.block(root, tags::pathfinding::SBSP_COLLISION_FIELD, Bsp::STRUCTURE_SIZE)?;
            floors.push(match coll.as_slice() {
                [record] => {
                    let at = record.as_ptr() as usize - bytes.as_ptr() as usize;
                    Some(bots::cache::walls_without_sky(&Bsp::decode_structure(&bytes, loc.address, at)?))
                }
                _ => None,
            });
        }
        for set in &sets {
            let Some(kind) = named(set) else { continue };
            let Some(Some(walls)) = usize::try_from(set.bsp).ok().and_then(|b| floors.get(b)) else {
                continue;
            };
            for point in &set.points {
                // The waypoints a ship holds at are the ones with a
                // `<name>_facing` partner.
                let facing = format!("{}_facing", point.name);
                if point.name.ends_with("_facing") || !set.points.iter().any(|p| p.name == facing) {
                    continue;
                }
                let at = point.position;
                if let Some((t, _)) = walls.hit(at, [at[0], at[1], at[2] - FLOOR_REACH]) {
                    let sample = HoverSample {
                        map: name.to_owned(),
                        set: set.name.clone(),
                        point: point.name.clone(),
                        height: t * FLOOR_REACH,
                    };
                    match kind {
                        ShipKind::Phantom => self.phantom.push(sample),
                        ShipKind::Pelican => self.pelican.push(sample),
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
