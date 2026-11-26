//! The game type markers of an open map (docs/game/gametype-markers.md,
//! "Drawing", "Studio"): the session's placements and trigger volumes
//! turned into the shared `game_markers` shapes, placed in the world, and
//! into the triangle and line lists of the layer each belongs to
//! ([`LayerId::SpawnPoints`], [`LayerId::Flags`], [`LayerId::NetgameItems`],
//! [`LayerId::Volume`]) for [`crate::render::Viewport::set_layer_overlay`].
//!
//! A placement this module has no marker for (a flag of a type the game
//! does not read, an item spawner whose model draws) is left to the
//! glyph path: [`Placed::members`] says which placements a marker
//! stands for.
use std::collections::BTreeMap;

use game_markers::{Area, Item, Kind, Mesh, Palette, Place, Solid, SpawnUse, Volume};

use crate::BlockKind;
use crate::geometry::{Gameplay, Marker, Vec3, cross, normalize, scale, sub};
use crate::layers::LayerId;
use crate::render::Vertex;
use crate::volumes::{VolumeKind, Volumes};

pub use game_markers;

/// The colour of a selected marker.
pub const SELECTED: [f32; 3] = [1.0, 1.0, 1.0];

/// One marker, placed.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    /// The placement as `scenario.*` names it (`netgame_flags[3]`; a
    /// hill of corners by its first), or `volume[2]` for a trigger volume.
    pub id: String,
    /// The layer that shows and hides it.
    pub layer: LayerId,
    pub marker: game_markers::Marker,
    pub place: Place,
    /// The placements it stands for: one, or a hill's corners.
    pub members: Vec<(BlockKind, usize)>,
    /// The trigger volume it draws.
    pub volume: Option<usize>,
    /// Where its area came from: `corners` of a hill, else `default`.
    pub area_from: Option<&'static str>,
    pub label: Option<String>,
    /// The flag's identifier: the hill's number.
    pub number: Option<i64>,
}

impl Placed {
    /// The marker's shape in the world, in `palette`'s colours, or white
    /// when `selected`.
    pub fn mesh(&self, palette: &Palette, selected: bool) -> Mesh {
        let mesh = if selected {
            self.marker.kind.mesh(SELECTED)
        } else {
            self.marker.mesh(palette)
        };
        mesh.placed(&self.place)
    }

    /// The box round the placed mesh.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.mesh(&Palette::default(), false).bounds()
    }
}

/// The marker of a netgame flag's type: the types the game reads
/// (`flag_type_name`), a kind with an area with its default area: the
/// plate for a flag return and a bomb goal, `Area::default` for a hill and
/// a territory.
pub fn flag_kind(flag_type: i64) -> Option<Kind> {
    Some(match flag_type {
        0 => Kind::FlagStand,
        1 => Kind::FlagReturn(Area::plate()),
        2 => Kind::BombSpawn,
        3 => Kind::BombGoal(Area::plate()),
        4 => Kind::BallSpawn,
        7 => Kind::TeleporterSender,
        8 => Kind::TeleporterReceiver,
        10 => Kind::Territory(Area::default()),
        11..=18 => Kind::Hill(Area::default()),
        _ => return None,
    })
}

/// What an item spawner makes, from the group of the tag it names: a
/// weapon or an item collection (netgame collections hold weapons), a
/// vehicle, else equipment.
pub fn item_of_group(group: [u8; 4]) -> Item {
    match &group {
        b"weap" | b"itmc" => Item::Weapon,
        b"vehi" | b"vehc" => Item::Vehicle,
        _ => Item::Equipment,
    }
}

/// A trigger volume's marker colour by what draws it.
pub fn volume_of(kind: VolumeKind) -> Volume {
    match kind {
        VolumeKind::Trigger => Volume::Trigger,
        VolumeKind::Kill => Volume::Kill,
        VolumeKind::BspSwitch => Volume::BspSwitch,
    }
}

fn team(raw: Option<i64>) -> Option<u8> {
    raw.filter(|t| (0..8).contains(t)).map(|t| t as u8)
}

fn place_of(m: &Marker) -> Place {
    Place {
        at: m.position,
        forward: m.axes[0],
        left: m.axes[1],
        up: m.axes[2],
    }
}

/// Every marker of the placements and volumes, in block order, then the
/// volumes'. `item` says what an item spawner makes and `modelled`
/// whether its model draws instead: a modelled spawner gets no marker.
/// Three or more corner flags of one hill (type and identifier) are one
/// `Hill` with the area of their hull at the centre it gives.
pub fn placed(
    markers: &[Marker],
    gameplay: impl Fn(&Marker) -> Gameplay,
    item: impl Fn(&Marker) -> Item,
    modelled: impl Fn(&Marker) -> bool,
    volumes: Option<&Volumes>,
) -> Vec<Placed> {
    placed_with(markers, gameplay, item, modelled, volumes, |_| None)
}

/// [`placed`] with the areas the document gives netgame flags: a hill,
/// territory or site flag with an area is one marker at the flag with that
/// area, and is no corner of a hull.
pub fn placed_with(
    markers: &[Marker],
    gameplay: impl Fn(&Marker) -> Gameplay,
    item: impl Fn(&Marker) -> Item,
    modelled: impl Fn(&Marker) -> bool,
    volumes: Option<&Volumes>,
    area_of: impl Fn(&Marker) -> Option<Area>,
) -> Vec<Placed> {
    let data: Vec<Gameplay> = markers.iter().map(&gameplay).collect();
    let id_of = |m: &Marker| format!("{}[{}]", block_name(m.block), m.index);
    let mut groups = BTreeMap::<(i64, i64), Vec<usize>>::new();
    for (i, m) in markers.iter().enumerate() {
        if let (BlockKind::NetgameFlags, Some(t @ 11..=18), Some(id)) =
            (m.block, data[i].flag_type, data[i].identifier)
            && area_of(m).is_none()
        {
            groups.entry((t, id)).or_default().push(i);
        }
    }
    // The groups that make one hill: three or more corners with a hull.
    let hulls: BTreeMap<(i64, i64), ([f32; 3], Area)> = groups
        .iter()
        .filter(|(_, g)| g.len() >= 3)
        .filter_map(|(k, g)| {
            let corners: Vec<Vec3> = g.iter().map(|&i| markers[i].position).collect();
            Some((*k, Area::of_corners(&corners)?))
        })
        .collect();

    let mut out = Vec::new();
    for (i, m) in markers.iter().enumerate() {
        let d = data[i];
        match m.block {
            BlockKind::StartingLocations => out.push(Placed {
                id: id_of(m),
                layer: LayerId::SpawnPoints,
                marker: game_markers::Marker::new(Kind::Spawn(SpawnUse::Respawn), team(d.team)),
                place: place_of(m),
                members: vec![(m.block, m.index)],
                volume: None,
                area_from: None,
                label: d.label(),
                number: None,
            }),
            BlockKind::NetgameEquipment if !modelled(m) => out.push(Placed {
                id: id_of(m),
                layer: LayerId::NetgameItems,
                marker: game_markers::Marker::new(Kind::Item(item(m)), None),
                place: place_of(m),
                members: vec![(m.block, m.index)],
                volume: None,
                area_from: None,
                label: d.label(),
                number: None,
            }),
            BlockKind::NetgameFlags => {
                let key = d.flag_type.zip(d.identifier);
                if let Some(area) = area_of(m)
                    && let Some(kind) = d.flag_type.and_then(flag_kind)
                    && kind.area().is_some()
                {
                    let kind = kind.with_area(area);
                    out.push(Placed {
                        id: id_of(m),
                        layer: LayerId::Flags,
                        marker: game_markers::Marker::new(kind, team(d.team)),
                        place: place_of(m),
                        members: vec![(m.block, m.index)],
                        volume: None,
                        area_from: None,
                        label: d.label(),
                        number: d.identifier,
                    });
                    continue;
                }
                if let Some(k) = key
                    && let Some((centre, area)) = hulls.get(&k)
                {
                    let group = &groups[&k];
                    // The hill is made once, at its first corner.
                    if group[0] == i {
                        out.push(Placed {
                            id: id_of(m),
                            layer: LayerId::Flags,
                            marker: game_markers::Marker::new(
                                Kind::Hill(area.clone()),
                                team(d.team),
                            ),
                            place: Place::facing(*centre, 0.0),
                            members: group
                                .iter()
                                .map(|&j| (markers[j].block, markers[j].index))
                                .collect(),
                            volume: None,
                            area_from: Some("corners"),
                            label: d.label(),
                            number: d.identifier,
                        });
                    }
                    continue;
                }
                let Some(kind) = d.flag_type.and_then(flag_kind) else {
                    continue;
                };
                let area_from = kind.area().map(|_| "default");
                out.push(Placed {
                    id: id_of(m),
                    layer: LayerId::Flags,
                    marker: game_markers::Marker::new(kind, team(d.team)),
                    place: place_of(m),
                    members: vec![(m.block, m.index)],
                    volume: None,
                    area_from,
                    label: d.label(),
                    number: d.identifier,
                });
            }
            _ => {}
        }
    }
    if let Some(volumes) = volumes {
        for (i, v) in volumes.triggers.iter().enumerate() {
            if !v.in_world() {
                continue;
            }
            // A box the game could not draw has no axes: left out.
            let (Some([forward, left, up]), Some(centre)) = (v.frame.axes(), v.frame.center())
            else {
                continue;
            };
            let place = Place {
                at: centre,
                forward,
                left,
                up,
            };
            let half = scale(v.frame.size, 0.5);
            for kind in volumes.kinds(i) {
                out.push(Placed {
                    id: format!("volume[{i}]"),
                    layer: LayerId::Volume(kind),
                    marker: game_markers::Marker::new(
                        Kind::Volume(volume_of(kind), Solid::Box { half }),
                        None,
                    ),
                    place,
                    members: Vec::new(),
                    volume: Some(i),
                    area_from: None,
                    label: Some(volumes.label(i)),
                    number: None,
                });
            }
        }
    }
    out
}

/// A block's API name: `netgame_flags`.
pub fn block_name(kind: BlockKind) -> String {
    let mut out = String::new();
    for (i, c) in format!("{kind:?}").chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Every layer a marker is drawn in, so a drawer can clear the ones with
/// no marker.
pub fn layers() -> impl Iterator<Item = LayerId> {
    [LayerId::SpawnPoints, LayerId::Flags, LayerId::NetgameItems]
        .into_iter()
        .chain(VolumeKind::ALL.map(LayerId::Volume))
}

/// The vertices of a mesh as the overlay draws them: the faces, then the
/// lines. The overlay's blend sets the faces' opacity; a colour's alpha is
/// not carried.
pub fn vertices(mesh: &Mesh) -> (Vec<Vertex>, Vec<Vertex>) {
    let rgb = |c: [f32; 4]| [c[0], c[1], c[2]];
    let mut faces = Vec::with_capacity(mesh.tris.len() * 3);
    for t in &mesh.tris {
        let n = cross(sub(t.points[1], t.points[0]), sub(t.points[2], t.points[0]));
        let n = if n.iter().all(|x| x.is_finite()) && n != [0.0; 3] {
            normalize(n)
        } else {
            [0.0, 0.0, 1.0]
        };
        for (p, c) in t.points.into_iter().zip(t.colours) {
            faces.push(Vertex {
                position: p,
                normal: n,
                color: rgb(c),
                center: p,
            });
        }
    }
    let mut lines = Vec::with_capacity(mesh.lines.len() * 2);
    for l in &mesh.lines {
        for p in [l.a, l.b] {
            lines.push(Vertex {
                position: p,
                normal: [0.0, 0.0, 1.0],
                color: rgb(l.colour),
                center: p,
            });
        }
    }
    (faces, lines)
}

/// Each layer's faces and lines. A layer with no marker is absent.
pub fn overlays(
    placed: &[Placed],
    palette: &Palette,
    selected: impl Fn(&Placed) -> bool,
) -> BTreeMap<LayerId, (Vec<Vertex>, Vec<Vertex>)> {
    let mut out = BTreeMap::<LayerId, (Vec<Vertex>, Vec<Vertex>)>::new();
    for p in placed {
        let (faces, lines) = vertices(&p.mesh(palette, selected(p)));
        let layer = out.entry(p.layer).or_default();
        layer.0.extend(faces);
        layer.1.extend(lines);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::rotation;
    use crate::{Scenario, fixture};
    use std::io::Cursor;

    fn flag(index: usize, at: Vec3) -> Marker {
        Marker {
            block: BlockKind::NetgameFlags,
            index,
            position: at,
            axes: rotation([0.0; 3]),
            scale: 1.0,
            object: None,
        }
    }

    fn open(bytes: Vec<u8>) -> Scenario {
        Scenario::read(&mut map::Map::from_reader(Cursor::new(bytes)).unwrap()).unwrap()
    }

    fn of(scenario: &Scenario, with_volumes: bool) -> Vec<Placed> {
        let markers = crate::geometry::markers(scenario);
        placed(
            &markers,
            |m| Gameplay::of(Some(scenario), m),
            |m| {
                scenario
                    .object_tag(m.block, m.index)
                    .map_or(Item::Equipment, |r| item_of_group(r.group.0))
            },
            |_| false,
            with_volumes.then_some(&scenario.volumes),
        )
    }

    /// Each placement block of the invented cache gives its kind and team.
    #[test]
    fn each_placement_block_gives_its_kind_and_team() {
        let scenario = open(fixture::fixture());
        let all = of(&scenario, false);
        let mut kinds: Vec<(&str, Option<u8>, LayerId)> = all
            .iter()
            .map(|p| (p.marker.kind.name(), p.marker.team, p.layer))
            .collect();
        kinds.sort();
        let mut want = vec![
            ("spawn", Some(1), LayerId::SpawnPoints),
            ("spawn", Some(1), LayerId::SpawnPoints),
            // The flag is of type 2, a bomb spawn, on team 1.
            ("bomb_spawn", Some(1), LayerId::Flags),
            // The netgame item names an item collection.
            ("weapon_spawn", None, LayerId::NetgameItems),
        ];
        want.sort();
        assert_eq!(kinds, want);
        let flag = all.iter().find(|p| p.layer == LayerId::Flags).unwrap();
        assert_eq!(flag.id, "netgame_flags[0]");
        assert_eq!(flag.members, [(BlockKind::NetgameFlags, 0)]);
    }

    /// A spawn faces where its placement does.
    #[test]
    fn a_spawn_faces_its_authored_yaw() {
        let scenario = open(fixture::fixture());
        let all = of(&scenario, false);
        let spawn = all.iter().find(|p| p.layer == LayerId::SpawnPoints).unwrap();
        let want = [0.5_f32.cos(), 0.5_f32.sin(), 0.0];
        for k in 0..3 {
            assert!((spawn.place.forward[k] - want[k]).abs() < 1e-5);
        }
    }

    /// A modelled item spawner draws its model, and a flag of a type the
    /// game does not read is left to the glyph path.
    #[test]
    fn a_modelled_item_and_an_unread_flag_get_no_marker() {
        let scenario = open(fixture::fixture());
        let markers = crate::geometry::markers(&scenario);
        let none = placed(
            &markers,
            |m| Gameplay::of(Some(&scenario), m),
            |_| Item::Weapon,
            |m| m.block == BlockKind::NetgameEquipment,
            None,
        );
        assert!(none.iter().all(|p| p.layer != LayerId::NetgameItems));
        let unread = placed(
            &[flag(0, [0.0; 3])],
            |_| Gameplay {
                flag_type: Some(5),
                ..Gameplay::default()
            },
            |_| Item::Weapon,
            |_| false,
            None,
        );
        assert!(unread.is_empty());
    }

    /// Three corners of a hill are one `Hill` at the hull's centre; two
    /// are two, and another identifier is another hill.
    #[test]
    fn a_hills_corners_give_one_hill() {
        let corners = [
            flag(0, [0.0, 0.0, 1.0]),
            flag(1, [4.0, 0.0, 1.0]),
            flag(2, [4.0, 4.0, 1.0]),
            flag(3, [0.0, 4.0, 1.0]),
            flag(4, [50.0, 0.0, 0.0]),
            flag(5, [52.0, 0.0, 0.0]),
        ];
        let fields = [(11, 1), (11, 1), (11, 1), (11, 1), (12, 1), (12, 1)];
        let out = placed(
            &corners,
            |m| Gameplay {
                flag_type: Some(fields[m.index].0),
                identifier: Some(fields[m.index].1),
                team: Some(2),
                ..Gameplay::default()
            },
            |_| Item::Weapon,
            |_| false,
            None,
        );
        // One hill of four corners, then two lone corners of hill 2.
        assert_eq!(out.len(), 3);
        let hill = &out[0];
        assert_eq!(hill.marker.kind.name(), "hill");
        assert_eq!(hill.members.len(), 4);
        assert_eq!(hill.id, "netgame_flags[0]");
        assert_eq!(hill.area_from, Some("corners"));
        assert_eq!(hill.place.at, [2.0, 2.0, 1.0]);
        assert_eq!(hill.marker.team, Some(2));
        assert!(matches!(&hill.marker.kind, Kind::Hill(a) if matches!(a.shape, game_markers::Shape::Path(_))));
        for lone in &out[1..] {
            assert_eq!(lone.members.len(), 1);
            assert_eq!(lone.area_from, Some("default"));
            assert!(matches!(&lone.marker.kind, Kind::Hill(a) if *a == Area::default()));
        }
    }

    /// A flag with an area is one hill at the flag with that area, and
    /// is no corner of a hull; its neighbours' corners still pair up.
    #[test]
    fn a_flag_with_an_area_is_one_hill_at_the_flag() {
        let flags = [
            flag(0, [10.0, 0.0, 0.0]),
            flag(1, [20.0, 0.0, 0.0]),
            flag(2, [24.0, 0.0, 0.0]),
        ];
        let l = Area::radius(5.0);
        let out = placed_with(
            &flags,
            |_| Gameplay {
                flag_type: Some(11),
                identifier: Some(0),
                ..Gameplay::default()
            },
            |_| Item::Weapon,
            |_| false,
            None,
            |m| (m.index == 0).then(|| l.clone()),
        );
        // The shaped flag, then the two lone corners (no hull of two).
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].place.at, [10.0, 0.0, 0.0]);
        assert_eq!(out[0].area_from, None);
        assert!(matches!(&out[0].marker.kind, Kind::Hill(a) if *a == l));
        assert!(out[1..].iter().all(|p| p.area_from == Some("default")));
    }

    /// The invented volumes draw a box a layer each kind they are, at
    /// their middle and on their axes.
    #[test]
    fn a_volume_is_a_box_at_its_middle_in_each_layer_it_is() {
        let scenario = open(fixture::fixture_with_volumes());
        let all = of(&scenario, true);
        let boxes: Vec<_> = all.iter().filter(|p| p.volume.is_some()).collect();
        // Volume 0 is a trigger and a BSP switch, volume 1 a trigger and a
        // kill trigger.
        assert_eq!(boxes.len(), 4);
        let layers: Vec<_> = boxes.iter().map(|p| (p.volume.unwrap(), p.layer)).collect();
        for want in [
            (0, LayerId::Volume(VolumeKind::Trigger)),
            (0, LayerId::Volume(VolumeKind::BspSwitch)),
            (1, LayerId::Volume(VolumeKind::Trigger)),
            (1, LayerId::Volume(VolumeKind::Kill)),
        ] {
            assert!(layers.contains(&want), "{want:?}");
        }
        let first = boxes.iter().find(|p| p.volume == Some(0)).unwrap();
        assert_eq!(first.place.at, [102.0, 102.0, 2.0]);
        assert!(matches!(
            &first.marker.kind,
            Kind::Volume(_, Solid::Box { half }) if *half == [2.0, 2.0, 2.0]
        ));
        // The second is turned a quarter: its marker's x is the world's y.
        let second = boxes.iter().find(|p| p.volume == Some(1)).unwrap();
        assert!(second.place.forward[1] > 0.99);
        let (lo, hi) = second.bounds().unwrap();
        assert!((hi[1] - lo[1] - 6.0).abs() < 1e-4, "{lo:?} {hi:?}");
    }

    /// The overlays hold each marker's own lines and faces in its layer,
    /// and a selected marker is white.
    #[test]
    fn overlays_split_by_layer_and_a_selected_marker_is_white() {
        let scenario = open(fixture::fixture());
        let all = of(&scenario, false);
        let palette = Palette::default();
        let overlays = overlays(&all, &palette, |p| p.layer == LayerId::Flags);
        assert_eq!(overlays.len(), 3);
        let (faces, lines) = &overlays[&LayerId::Flags];
        let flag = all.iter().find(|p| p.layer == LayerId::Flags).unwrap();
        let mesh = flag.mesh(&palette, true);
        assert_eq!(lines.len(), mesh.lines.len() * 2);
        assert_eq!(faces.len(), mesh.tris.len() * 3);
        assert!(lines.iter().chain(faces).all(|v| v.color == SELECTED));
        let (_, spawn_lines) = &overlays[&LayerId::SpawnPoints];
        assert!(spawn_lines.iter().all(|v| v.color != SELECTED));
        let tint = palette.teams[1];
        assert!(spawn_lines.iter().all(|v| v.color == tint));
        assert_eq!(layers().count(), 6);
    }
}
