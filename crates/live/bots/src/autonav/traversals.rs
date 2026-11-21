//! Scenario traversal placements. Record layouts match scenario-edit's
//! layout table, whose evidence includes the retail object-type table.

use std::io::{Read, Seek};

use tags::geometry::TagView;

use crate::cache::MapData;
use crate::nav::{Edge, Kind, STANDING, V3, standing_clear};

pub(super) struct Teleporter {
    position: V3,
    channel: i16,
    sender: bool,
}

#[derive(Clone, Copy)]
enum LiftKind {
    Vertical,
    Shaft,
    Elevator,
    CyclotronElevator,
    Launcher,
    PowerBoost,
}

struct Lift {
    position: V3,
    yaw: f32,
    kind: LiftKind,
}

#[derive(Default)]
pub(super) struct Placements {
    teleporters: Vec<Teleporter>,
    lifts: Vec<Lift>,
}

fn word(record: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(record[at..at + 4].try_into().unwrap())
}

fn short(record: &[u8], at: usize) -> i16 {
    i16::from_le_bytes(record[at..at + 2].try_into().unwrap())
}

fn point(record: &[u8], at: usize) -> V3 {
    std::array::from_fn(|k| f32::from_bits(word(record, at + k * 4)))
}

fn flags(view: &TagView<'_>) -> tags::Result<Vec<Teleporter>> {
    // The H2 converter applies types 7/8 and pairs at +0x14 (identifier).
    Ok(view
        .block(view.bytes(), 0x118, 0x20)?
        .into_iter()
        .filter_map(|f| {
            let kind = short(f, 0x10);
            let position = point(f, 0);
            (matches!(kind, 7 | 8) && position.into_iter().all(f32::is_finite)).then(|| {
                Teleporter {
                    position,
                    channel: short(f, 0x14),
                    sender: kind == 7,
                }
            })
        })
        .collect())
}

fn lift_kind(path: &str) -> Option<LiftKind> {
    let path = path.rsplit(['\\', '/']).next()?.to_ascii_lowercase();
    if path == "power_boost" {
        Some(LiftKind::PowerBoost)
    } else if path == "elevator_cyclotron" {
        Some(LiftKind::CyclotronElevator)
    } else if path.contains("man_cannon") || path.contains("mancannon") {
        Some(LiftKind::Launcher)
    } else if path.contains("elevator") {
        Some(LiftKind::Elevator)
    } else if matches!(
        path.as_str(),
        "col_power_lift_high"
            | "col_power_lift_low"
            | "hvac_lift"
            | "hvac_lift_short"
            | "foun_hvac_lift_short"
            | "steam_lift"
            | "lockout_lift_tube"
            | "cov_power_lift"
            | "midpillar_lift"
            | "grav_column_gemini"
    ) {
        Some(LiftKind::Shaft)
    } else if path == "backwash_lift"
        || path == "warlock_lift"
        || path == "grav_colossus"
        || path.contains("gravity_lift")
        || path.contains("grav_lift")
        || path.contains("gravlift")
    {
        Some(LiftKind::Vertical)
    } else {
        None
    }
}

impl Placements {
    pub(super) fn read<R: Read + Seek>(
        view: &TagView<'_>,
        map: &map::Map<R>,
    ) -> Result<Self, String> {
        let teleporters = flags(view).map_err(|e| e.to_string())?;
        let root = view.bytes();
        let mut lifts = Vec::new();
        for (at, stride, palette_at) in
            [(0x50, 0x5c, 0x58), (0xa8, 0x48, 0xb0), (0x328, 0x4c, 0x330)]
        {
            let palette = view
                .block(root, palette_at, 0x28)
                .map_err(|e| e.to_string())?;
            let classes: Vec<_> = palette
                .iter()
                .map(|p| {
                    let tag = map.tag(map::TagId(word(p, 4))).ok()?;
                    lift_kind(tag.name.as_deref()?)
                })
                .collect();
            if classes.iter().all(Option::is_none) {
                continue;
            }
            for record in view.block(root, at, stride).map_err(|e| e.to_string())? {
                let k = short(record, 0);
                let Some(&Some(kind)) = classes.get(k as usize) else {
                    continue;
                };
                // Common placement layout. Bit 0 is "not automatically placed".
                if word(record, 4) & 1 != 0 {
                    continue;
                }
                let position = point(record, 8);
                let yaw = f32::from_bits(word(record, 0x14));
                if yaw.is_finite() && position.into_iter().all(f32::is_finite) {
                    lifts.push(Lift {
                        position,
                        yaw,
                        kind,
                    });
                }
            }
        }
        Ok(Self { teleporters, lifts })
    }

    pub(super) fn add_links(&self, data: &mut MapData) {
        let snap = |p| {
            data.nav
                .nearest(p, 0.8, 0.6, 1.5)
                .filter(|&(_, at)| standing_clear(&data.walls, at))
        };
        let mut edges = Vec::new();
        for f in self.teleporters.iter().filter(|f| f.sender) {
            let Some((from, a)) = snap(f.position) else {
                continue;
            };
            for r in self
                .teleporters
                .iter()
                .filter(|r| !r.sender && r.channel == f.channel)
            {
                // Some receivers are in the air above the ground (Containment).
                // Require a clear fall, and never widen the sender's trigger.
                let receiver = snap(r.position).or_else(|| {
                    let (to, b) = data.nav.nearest(r.position, 0.8, 0.0, 3.5)?;
                    let head = |p: V3| [p[0], p[1], p[2] + STANDING];
                    (standing_clear(&data.walls, b)
                        && data.walls.clear(head(r.position), head(b))
                        && data.walls.clear(r.position, [b[0], b[1], b[2] + 0.05]))
                    .then_some((to, b))
                });
                let Some((to, b)) = receiver else {
                    continue;
                };
                edges.push(Edge {
                    from,
                    to,
                    a,
                    b,
                    kind: Kind::Teleporter,
                });
            }
        }
        edges.extend(self.lifts.iter().filter_map(|lift| lift_link(data, lift)));
        data.nav.append_shortcuts(edges);
    }
}

fn lift_link(data: &MapData, lift: &Lift) -> Option<Edge> {
    let (from, a) = data.nav.nearest(lift.position, 1.2, 1.4, 2.0)?;
    // Ascension's placement faces out of the playable platform; the
    // walkable receiver is behind that axis. Generic launchers use +X.
    let yaw = if matches!(lift.kind, LiftKind::PowerBoost) {
        lift.yaw + std::f32::consts::PI
    } else {
        lift.yaw
    };
    let (sin, cos) = yaw.sin_cos();
    let launcher = matches!(lift.kind, LiftKind::Launcher | LiftKind::PowerBoost);
    // Estimated envelopes for the retail lift families. A destination
    // has to be walkable and the predicted ride must clear the BSP.
    let (reach, min_rise, max_rise, min_forward) = match lift.kind {
        LiftKind::Launcher | LiftKind::PowerBoost => (24.0, 0.75, 14.0, 3.0),
        LiftKind::Shaft => (5.0, 2.0, 12.0, 0.0),
        LiftKind::Vertical => (4.5, 0.75, 6.0, 0.0),
        LiftKind::Elevator => (3.0, 0.75, 12.0, 0.0),
        // The retail shaft has trim only 1.3 above its base. Require a
        // full ride to reach the BSP deck about 5.2 above it.
        LiftKind::CyclotronElevator => (3.0, 3.5, 12.0, 0.0),
    };
    data.nav
        .polys
        .iter()
        .enumerate()
        .filter_map(|(to, p)| {
            if to == from as usize {
                return None;
            }
            let aim = if launcher {
                [a[0] + 12.0 * cos, a[1] + 12.0 * sin, a[2]]
            } else {
                a
            };
            let b = p.interior_near(aim, 0.45);
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let rise = b[2] - a[2];
            let distance = dx.hypot(dy);
            if !(min_rise..=max_rise).contains(&rise)
                || distance > reach
                || (launcher
                    && (dx * cos + dy * sin < min_forward || (dx * sin - dy * cos).abs() > 5.0))
                || !standing_clear(&data.walls, b)
            {
                return None;
            }
            let raised = |p: V3| [p[0], p[1], p[2] + STANDING];
            let crown = if launcher {
                // Get above the entrance rim before crossing the gap.
                [
                    a[0] + 0.35 * (b[0] - a[0]),
                    a[1] + 0.35 * (b[1] - a[1]),
                    b[2] + 2.0,
                ]
            } else {
                [lift.position[0], lift.position[1], b[2] + 0.25]
            };
            let feet = |p: V3| [p[0], p[1], p[2] + 0.05];
            let ride_clear = |c| {
                data.walls.clear(feet(a), feet(c))
                    && data.walls.clear(feet(c), feet(b))
                    && data.walls.clear(raised(a), raised(c))
                    && data.walls.clear(raised(c), raised(b))
            };
            // The follower steers toward the deck in flight. A shaft mouth
            // can clear laterally before the deck overhang is reached.
            if !ride_clear(crown)
                && !(matches!(lift.kind, LiftKind::Shaft)
                    && [0.25, 0.5, 0.75].into_iter().any(|t| {
                        ride_clear([
                            crown[0] + t * (b[0] - crown[0]),
                            crown[1] + t * (b[1] - crown[1]),
                            crown[2],
                        ])
                    }))
            {
                return None;
            }
            let score = (b[0] - aim[0]).hypot(b[1] - aim[1]) + rise * 0.25;
            let kind = if matches!(lift.kind, LiftKind::Elevator | LiftKind::CyclotronElevator) {
                Kind::Elevator
            } else {
                Kind::Lift
            };
            Some((
                score,
                Edge {
                    from,
                    to: to as u32,
                    a,
                    b,
                    kind,
                },
            ))
        })
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, e)| e)
}

#[cfg(test)]
mod tests;
