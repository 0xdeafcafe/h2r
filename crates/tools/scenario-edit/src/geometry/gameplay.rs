//! Authored fields used by the gameplay glyphs, labels and outlines.
use std::collections::BTreeMap;

use super::{Marker, Vec3};
use crate::{BlockKind, Scenario, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gameplay {
    pub team: Option<i64>,
    pub flag_type: Option<i64>,
    pub identifier: Option<i64>,
    pub respawn: Option<i64>,
}

impl Gameplay {
    pub fn of(scenario: Option<&Scenario>, m: &Marker) -> Self {
        let Some(scenario) = scenario else {
            return Self::default();
        };
        let field = |name| {
            scenario
                .get(m.block, m.index, name)
                .ok()
                .and_then(Value::as_int)
        };
        match m.block {
            BlockKind::StartingLocations => Self {
                team: field("team"),
                ..Self::default()
            },
            BlockKind::NetgameFlags => Self {
                team: field("team"),
                flag_type: field("type"),
                identifier: field("identifier"),
                ..Self::default()
            },
            BlockKind::NetgameEquipment => Self {
                respawn: field("respawn time"),
                ..Self::default()
            },
            _ => Self::default(),
        }
    }

    pub fn label(&self) -> Option<String> {
        if let Some(time) = self.respawn {
            return Some(match time {
                0 => "Respawn (collection default)".to_owned(),
                t if t > 0 => format!("Respawn {t} s"),
                raw => format!("Respawn {raw}"),
            });
        }
        let mut text = self.flag_type.map(|kind| match kind {
            11..=18 => format!("Hill {}", kind - 10),
            _ => flag_type_name(kind).map_or_else(|| format!("Flag type {kind}"), str::to_owned),
        });
        if let (Some(label), Some(id)) = (&mut text, self.identifier) {
            label.push_str(&format!(" · ID {id}"));
        }
        // The palette covers the eight teams and neutral. Preserve a designator
        // outside that range as its authored number.
        if let Some(team) = self.team.filter(|team| !(0..=8).contains(team)) {
            let team_label = format!("Team {team}");
            match &mut text {
                Some(label) => {
                    label.push_str(" · ");
                    label.push_str(&team_label);
                }
                None => text = Some(team_label),
            }
        }
        text
    }
}

/// Objective types read by the engine (docs/game/bots.md, section 3.7),
/// plus the teleporter pair identified in h1convert::placements::H2Flag.
pub fn flag_type_name(kind: i64) -> Option<&'static str> {
    Some(match kind {
        0 => "CTF flag spawn",
        1 => "CTF flag return",
        2 => "Bomb spawn",
        3 => "Bomb return",
        4 => "Oddball spawn",
        7 => "Teleporter source",
        8 => "Teleporter destination",
        10 => "Territory marker",
        11..=18 => "Hill marker",
        _ => return None,
    })
}

/// Join the authored points of each hill or territory in scenario order.
/// Two points make a segment. Three or more make a closed outline when drawn.
pub fn flag_outlines(markers: &[Marker], data: impl Fn(&Marker) -> Gameplay) -> Vec<Vec<Vec3>> {
    let mut outlines = BTreeMap::<(i64, i64), Vec<Vec3>>::new();
    for marker in markers
        .iter()
        .filter(|m| m.block == BlockKind::NetgameFlags)
    {
        let Gameplay {
            flag_type: Some(kind @ 10..=18),
            identifier: Some(id),
            ..
        } = data(marker)
        else {
            continue;
        };
        outlines
            .entry((kind, id))
            .or_default()
            .push(marker.position);
    }
    outlines
        .into_values()
        .filter(|points| points.len() >= 2)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Ray, add, dot, marker_triangles_styled, pick, rotation, sub};
    use std::io::Cursor;

    fn marker(block: BlockKind, index: usize) -> Marker {
        Marker {
            block,
            index,
            position: [index as f32, 3.0, -4.0],
            axes: rotation([0.0; 3]),
            scale: 1.0,
            object: None,
        }
    }

    #[test]
    fn spawn_arrow_points_along_its_authored_facing() {
        let scenario = Scenario::read(
            &mut map::Map::from_reader(Cursor::new(crate::fixture::fixture())).unwrap(),
        )
        .unwrap();
        let m = crate::geometry::markers(&scenario)
            .into_iter()
            .find(|m| m.block == BlockKind::StartingLocations)
            .unwrap();
        // The fixture's starting location faces half a radian from +X.
        let facing = [0.5_f32.cos(), 0.5_f32.sin(), 0.0];
        let arrow_tip = marker_triangles_styled(&m, None)
            .into_iter()
            .flatten()
            .map(|p| sub(p, m.position))
            .max_by(|a, b| dot(*a, facing).total_cmp(&dot(*b, facing)))
            .unwrap();
        assert!(dot(arrow_tip, facing) > 0.9);
        assert!(dot(arrow_tip, [-facing[1], facing[0], 0.0]).abs() < 1e-5);
        assert!(arrow_tip[2].abs() < 0.1);
        assert_eq!(Gameplay::of(Some(&scenario), &m).team, Some(1));
    }

    #[test]
    fn picking_reaches_the_spawn_arrow_and_flag_pennant_tips() {
        let cases = [
            (BlockKind::StartingLocations, None, [0.95, 0.0, 0.04]),
            (BlockKind::NetgameFlags, Some(8), [0.75, 0.0, 1.60]),
        ];
        for (kind, flag_type, offset) in cases {
            let m = marker(kind, 0);
            let tip = add(m.position, offset);
            assert!(
                marker_triangles_styled(&m, flag_type)
                    .into_iter()
                    .flatten()
                    .any(|point| {
                        let delta = sub(point, tip);
                        dot(delta, delta) < 1e-8
                    })
            );
            let ray = Ray {
                origin: add(tip, [0.0, -5.0, 0.0]),
                dir: [0.0, 1.0, 0.0],
            };
            assert_eq!(pick(&[m], &ray), Some((kind, 0)));
        }
    }

    #[test]
    fn flag_type_changes_the_pennant_and_the_renderer_uses_its_team_tint() {
        let flag = marker(BlockKind::NetgameFlags, 2);
        let tint = [0.25, 0.75, 0.5];
        let render = |kind| {
            crate::render::glyph_vertices_styled(&[flag], &[], |_| false, |_| (tint, Some(kind)))
        };
        let source = render(7);
        assert!(source.iter().all(|v| v.color == tint));
        let tip_height = |vertices: &[crate::render::Vertex]| {
            vertices
                .iter()
                .filter(|v| v.position[0] > flag.position[0] + 0.7)
                .map(|v| v.position[2] - flag.position[2])
                .reduce(f32::max)
                .unwrap()
        };
        assert!(tip_height(&render(8)) > tip_height(&source) + 0.4);
        assert_ne!(
            marker_triangles_styled(&flag, Some(10)),
            marker_triangles_styled(&flag, Some(11))
        );
        assert_eq!(
            marker_triangles_styled(&flag, Some(99)),
            marker_triangles_styled(&flag, None)
        );
    }

    #[test]
    fn gameplay_labels_use_authored_fields_and_keep_unknown_values() {
        let scenario = Scenario::read(
            &mut map::Map::from_reader(Cursor::new(crate::fixture::fixture())).unwrap(),
        )
        .unwrap();
        let equip = marker(BlockKind::NetgameEquipment, 0);
        let data = Gameplay::of(Some(&scenario), &equip);
        assert_eq!(data.label().as_deref(), Some("Respawn 30 s"));
        let default_time = Gameplay {
            respawn: Some(0),
            ..data
        };
        assert_eq!(
            default_time.label().as_deref(),
            Some("Respawn (collection default)")
        );
        let no_data = Gameplay::of(None, &equip);
        assert_eq!(no_data, Gameplay::default());
        assert!(no_data.label().is_none());

        let flag = Gameplay::of(Some(&scenario), &marker(BlockKind::NetgameFlags, 0));
        assert_eq!((flag.team, flag.flag_type), (Some(1), Some(2)));
        let unknown = Gameplay {
            flag_type: Some(41),
            identifier: Some(3),
            team: Some(30),
            ..flag
        };
        assert_eq!(
            unknown.label().as_deref(),
            Some("Flag type 41 · ID 3 · Team 30")
        );
    }

    #[test]
    fn flag_outlines_join_only_matching_type_and_identifier_in_authored_order() {
        let fields = [
            (11, 4),
            (10, 4),
            (11, 1),
            (8, 4),
            (11, 4),
            (10, 4),
            (8, 4),
            (11, 4),
        ];
        let markers: Vec<_> = (0..fields.len())
            .map(|i| marker(BlockKind::NetgameFlags, i))
            .collect();
        let outlines = flag_outlines(&markers, |m| {
            let (kind, id) = fields[m.index];
            Gameplay {
                flag_type: Some(kind),
                identifier: Some(id),
                ..Default::default()
            }
        });
        assert_eq!(
            outlines,
            [
                vec![markers[1].position, markers[5].position],
                vec![
                    markers[0].position,
                    markers[4].position,
                    markers[7].position
                ],
            ],
        );
    }
}
