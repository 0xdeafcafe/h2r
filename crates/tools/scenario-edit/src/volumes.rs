//! The scenario's trigger volumes, and the kill triggers and BSP switches
//! that name them by index: read from the `scnr` root, drawn as oriented
//! boxes and picked by ray (docs/editor/polish-plan.md, slice P8).
//!
//! Layouts (docs/editor/scenario-editor.md, "Trigger volumes, kill
//! triggers and BSP switches"):
//!
//! - Trigger volumes, `+0x108`, stride `0x44`: name (string ID) `+0x0`,
//!   parent object name index (i16, -1 for none) `+0x4`, forward `+0xc`,
//!   up `+0x18`, origin `+0x24`, extents `+0x30`. The origin is a corner
//!   and the extents are full lengths along forward, left and up.
//! - Kill triggers, `+0x230`, stride 2: the trigger volume index (i16).
//! - BSP switches, `+0x130`, stride `0xe`: trigger volume, source BSP and
//!   destination BSP (i16 each).
//!
//! A volume with a parent object keeps its frame relative to that object,
//! which only a running game places, so it is listed and not drawn.
use crate::geometry::{Ray, Tri, Vec3, add, cross, dot, scale, sub};

/// The trigger volume block in the scenario root and its element size.
pub const TRIGGER_VOLUMES: (u32, u32) = (0x108, 0x44);
/// The BSP switch block and its element size.
pub const BSP_SWITCHES: (u32, u32) = (0x130, 0xe);
/// The kill trigger block and its element size.
pub const KILL_TRIGGERS: (u32, u32) = (0x230, 0x2);

/// What a trigger volume is drawn as. One box can be all three: every
/// volume is a trigger, and kill triggers and BSP switches name some.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VolumeKind {
    Trigger,
    Kill,
    BspSwitch,
}

impl VolumeKind {
    pub const ALL: [Self; 3] = [Self::Trigger, Self::Kill, Self::BspSwitch];

    /// The name the agent API and the layer list use.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Kill => "kill",
            Self::BspSwitch => "bsp_switch",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Which of a box's kinds wins its colour, label and pick: the most
    /// specific.
    const fn rank(self) -> u8 {
        match self {
            Self::Kill => 0,
            Self::BspSwitch => 1,
            Self::Trigger => 2,
        }
    }
}

/// An oriented box: a corner, two stored directions and full lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    /// Full lengths along forward, left and up.
    pub size: Vec3,
}

fn unit(v: Vec3) -> Option<Vec3> {
    let len = dot(v, v).sqrt();
    (len > 1e-6 && len.is_finite()).then(|| scale(v, 1.0 / len))
}

impl Frame {
    /// Unit forward, left and up. `None` for a frame the game could not
    /// draw either: a zero or parallel pair of directions, or a size that
    /// is not positive and finite.
    pub fn axes(&self) -> Option<[Vec3; 3]> {
        if !self.size.iter().all(|v| *v > 0.0 && v.is_finite())
            || !self.origin.iter().all(|v| v.is_finite())
        {
            return None;
        }
        let forward = unit(self.forward)?;
        let left = unit(cross(unit(self.up)?, forward))?;
        Some([forward, left, cross(forward, left)])
    }

    /// The eight corners; corner `i` adds the size along axis `k` where
    /// bit `k` of `i` is set.
    pub fn corners(&self) -> Option<[Vec3; 8]> {
        let axes = self.axes()?;
        Some(std::array::from_fn(|i| {
            (0..3)
                .filter(|k| i & (1 << k) != 0)
                .fold(self.origin, |p, k| add(p, scale(axes[k], self.size[k])))
        }))
    }

    pub fn center(&self) -> Option<Vec3> {
        let c = self.corners()?;
        Some(scale(add(c[0], c[7]), 0.5))
    }

    /// Half the diagonal: the radius of the sphere round the box.
    pub fn radius(&self) -> f32 {
        0.5 * dot(self.size, self.size).sqrt()
    }

    /// The twelve edges.
    pub fn edges(&self) -> Option<Vec<[Vec3; 2]>> {
        let c = self.corners()?;
        let mut out = Vec::with_capacity(12);
        for (i, &from) in c.iter().enumerate() {
            for bit in [1, 2, 4] {
                if i & bit == 0 {
                    out.push([from, c[i | bit]]);
                }
            }
        }
        Some(out)
    }

    /// The six faces as twelve triangles with outward normals.
    pub fn faces(&self) -> Option<Vec<Tri>> {
        let c = self.corners()?;
        let axes = self.axes()?;
        let mut out = Vec::with_capacity(12);
        for k in 0..3 {
            let (a, b) = ((k + 1) % 3, (k + 2) % 3);
            for high in [false, true] {
                let base = if high { 1 << k } else { 0 };
                let quad = [base, base | 1 << a, base | 1 << a | 1 << b, base | 1 << b];
                let n = if high { axes[k] } else { scale(axes[k], -1.0) };
                let p = quad.map(|i| c[i]);
                out.push(Tri {
                    p: [p[0], p[1], p[2]],
                    n,
                });
                out.push(Tri {
                    p: [p[0], p[2], p[3]],
                    n,
                });
            }
        }
        Some(out)
    }

    /// Distance along the ray to where it enters the box. A ray that
    /// starts inside does not hit it, so a camera inside a large volume
    /// still clicks what is in front of it.
    pub fn hit(&self, ray: &Ray) -> Option<f32> {
        let axes = self.axes()?;
        let local = sub(ray.origin, self.origin);
        let (mut near, mut far) = (f32::NEG_INFINITY, f32::INFINITY);
        for k in 0..3 {
            let o = dot(local, axes[k]);
            let d = dot(ray.dir, axes[k]);
            if d.abs() < 1e-9 {
                if o < 0.0 || o > self.size[k] {
                    return None;
                }
                continue;
            }
            let (a, b) = ((0.0 - o) / d, (self.size[k] - o) / d);
            near = near.max(a.min(b));
            far = far.min(a.max(b));
        }
        (near <= far && near > 0.0).then_some(near)
    }
}

/// One trigger volume.
#[derive(Clone, Debug, PartialEq)]
pub struct TriggerVolume {
    /// The name's string ID, and its text when the map's table has it.
    pub name_id: u32,
    pub name: Option<String>,
    /// The object name index the frame is relative to, if any.
    pub parent: Option<u16>,
    pub frame: Frame,
}

impl TriggerVolume {
    /// World space: no parent object.
    pub fn in_world(&self) -> bool {
        self.parent.is_none()
    }
}

/// One BSP switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BspSwitch {
    pub volume: i16,
    pub from: i16,
    pub to: i16,
}

/// An entry that names a trigger volume by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    /// Kill trigger `index`.
    Kill(usize),
    /// BSP switch `index`, from one BSP to another.
    BspSwitch { index: usize, from: i16, to: i16 },
}

impl Holder {
    pub fn kind(self) -> VolumeKind {
        match self {
            Self::Kill(_) => VolumeKind::Kill,
            Self::BspSwitch { .. } => VolumeKind::BspSwitch,
        }
    }
    pub fn index(self) -> usize {
        match self {
            Self::Kill(i) | Self::BspSwitch { index: i, .. } => i,
        }
    }
}

/// The scenario's volumes. A block that does not read leaves all three
/// empty and says why in [`Self::error`]; the placements still open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volumes {
    pub triggers: Vec<TriggerVolume>,
    /// The trigger volume index each kill trigger names.
    pub kills: Vec<i16>,
    pub switches: Vec<BspSwitch>,
    pub error: Option<String>,
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn vec3_at(b: &[u8], o: usize) -> Vec3 {
    std::array::from_fn(|k| f32::from_bits(u32_at(b, o + 4 * k)))
}

impl Volumes {
    /// From the three blocks' elements (each at its stride).
    pub fn from_rows(triggers: &[Vec<u8>], kills: &[Vec<u8>], switches: &[Vec<u8>]) -> Self {
        Self {
            triggers: triggers
                .iter()
                .map(|e| TriggerVolume {
                    name_id: u32_at(e, 0),
                    name: None,
                    parent: u16::try_from(i16_at(e, 4)).ok(),
                    frame: Frame {
                        forward: vec3_at(e, 0xc),
                        up: vec3_at(e, 0x18),
                        origin: vec3_at(e, 0x24),
                        size: vec3_at(e, 0x30),
                    },
                })
                .collect(),
            kills: kills.iter().map(|e| i16_at(e, 0)).collect(),
            switches: switches
                .iter()
                .map(|e| BspSwitch {
                    volume: i16_at(e, 0),
                    from: i16_at(e, 2),
                    to: i16_at(e, 4),
                })
                .collect(),
            error: None,
        }
    }

    pub fn failed(why: impl Into<String>) -> Self {
        Self {
            error: Some(why.into()),
            ..Self::default()
        }
    }

    /// Name each volume from the map's string-ID table (index in the low
    /// 24 bits).
    pub fn resolve_names(&mut self, strings: &[Option<String>]) {
        for v in &mut self.triggers {
            v.name = strings
                .get((v.name_id & 0x00ff_ffff) as usize)
                .cloned()
                .flatten()
                .filter(|s| !s.is_empty());
        }
    }

    /// The entries that name trigger volume `index`.
    pub fn holders(&self, index: usize) -> Vec<Holder> {
        let names = |v: i16| usize::try_from(v).ok() == Some(index);
        let kills = self
            .kills
            .iter()
            .enumerate()
            .filter(|(_, v)| names(**v))
            .map(|(i, _)| Holder::Kill(i));
        let switches = self
            .switches
            .iter()
            .enumerate()
            .filter(|(_, s)| names(s.volume))
            .map(|(i, s)| Holder::BspSwitch {
                index: i,
                from: s.from,
                to: s.to,
            });
        kills.chain(switches).collect()
    }

    /// The kinds trigger volume `index` is drawn as, most specific first.
    pub fn kinds(&self, index: usize) -> Vec<VolumeKind> {
        if index >= self.triggers.len() {
            return Vec::new();
        }
        let mut out: Vec<VolumeKind> = self.holders(index).iter().map(|h| h.kind()).collect();
        out.push(VolumeKind::Trigger);
        out.sort_by_key(|k| k.rank());
        out.dedup();
        out
    }

    /// How many boxes a layer of `kind` has: every volume for triggers,
    /// the distinct volumes the kill triggers or switches name otherwise.
    pub fn count(&self, kind: VolumeKind) -> usize {
        (0..self.triggers.len())
            .filter(|&i| self.kinds(i).contains(&kind))
            .count()
    }

    /// The name, or the volume's index when the map has no text for it.
    pub fn label(&self, index: usize) -> String {
        match self.triggers.get(index).and_then(|v| v.name.as_deref()) {
            Some(name) => name.to_owned(),
            None => format!("trigger volume {index}"),
        }
    }

    /// The viewport label: the name, then each holder.
    pub fn caption(&self, index: usize) -> String {
        let mut text = self.label(index);
        for h in self.holders(index) {
            match h {
                Holder::Kill(_) => text.push_str(" · kill"),
                Holder::BspSwitch { from, to, .. } => {
                    text.push_str(&format!(" · BSP {from} to {to}"));
                }
            }
        }
        text
    }

    /// The nearest world-space box the ray enters among those with a kind
    /// `shown` accepts.
    pub fn pick(&self, ray: &Ray, shown: impl Fn(VolumeKind) -> bool) -> Option<(f32, usize)> {
        self.triggers
            .iter()
            .enumerate()
            .filter(|(i, v)| v.in_world() && self.kinds(*i).into_iter().any(&shown))
            .filter_map(|(i, v)| v.frame.hit(ray).map(|t| (t, i)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(origin: Vec3, forward: Vec3, size: Vec3) -> Frame {
        Frame {
            origin,
            forward,
            up: [0.0, 0.0, 1.0],
            size,
        }
    }

    fn row(name: u32, parent: i16, f: &Frame) -> Vec<u8> {
        let mut e = vec![0u8; 0x44];
        e[0..4].copy_from_slice(&name.to_le_bytes());
        e[4..6].copy_from_slice(&parent.to_le_bytes());
        for (at, v) in [
            (0xc, f.forward),
            (0x18, f.up),
            (0x24, f.origin),
            (0x30, f.size),
        ] {
            for k in 0..3 {
                e[at + 4 * k..at + 4 * k + 4].copy_from_slice(&v[k].to_le_bytes());
            }
        }
        e
    }

    #[test]
    fn a_box_starts_at_its_corner_and_runs_along_forward_left_and_up() {
        // Turned a quarter: forward is +y, so left is -x.
        let f = frame([10.0, 0.0, 0.0], [0.0, 2.0, 0.0], [4.0, 2.0, 1.0]);
        let c = f.corners().unwrap();
        assert_eq!(c[0], [10.0, 0.0, 0.0]);
        assert_eq!(c[1], [10.0, 4.0, 0.0]);
        assert_eq!(c[2], [8.0, 0.0, 0.0]);
        assert_eq!(c[4], [10.0, 0.0, 1.0]);
        assert_eq!(f.center().unwrap(), [9.0, 2.0, 0.5]);
        assert_eq!(f.edges().unwrap().len(), 12);
        let faces = f.faces().unwrap();
        assert_eq!(faces.len(), 12);
        // Every face's normal points away from the centre.
        let centre = f.center().unwrap();
        for t in &faces {
            assert!(dot(sub(t.p[0], centre), t.n) > 0.0, "{t:?}");
        }
        for bad in [[0.0, 1.0, 1.0], [1.0, f32::NAN, 1.0]] {
            assert!(frame([0.0; 3], [1.0, 0.0, 0.0], bad).corners().is_none());
        }
        assert!(frame([0.0; 3], [0.0, 0.0, 3.0], [1.0; 3]).axes().is_none());
    }

    #[test]
    fn a_ray_through_a_box_picks_it_and_one_from_inside_does_not() {
        let f = frame([10.0, 0.0, 0.0], [0.0, 1.0, 0.0], [4.0, 2.0, 1.0]);
        let toward = |origin: Vec3, to: Vec3| Ray {
            origin,
            dir: crate::geometry::normalize(sub(to, origin)),
        };
        let centre = f.center().unwrap();
        let t = f.hit(&toward([9.0, 2.0, 20.0], centre)).unwrap();
        assert!((t - 19.0).abs() < 1e-4, "{t}");
        assert!(f.hit(&toward([9.0, 2.0, 20.0], [30.0, 2.0, 0.5])).is_none());
        assert!(f.hit(&toward(centre, [0.0; 3])).is_none());
        // Pointing away.
        assert!(f.hit(&toward([9.0, 2.0, 20.0], [9.0, 2.0, 40.0])).is_none());
    }

    #[test]
    fn kill_triggers_and_switches_name_the_volumes_they_draw() {
        let a = frame([0.0; 3], [1.0, 0.0, 0.0], [2.0; 3]);
        let b = frame([10.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0; 3]);
        let parented = frame([20.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0; 3]);
        let mut switch = vec![0u8; 0xe];
        switch[0..2].copy_from_slice(&0i16.to_le_bytes());
        switch[2..4].copy_from_slice(&1i16.to_le_bytes());
        switch[4..6].copy_from_slice(&2i16.to_le_bytes());
        let mut v = Volumes::from_rows(
            &[
                row(1, -1, &a),
                row(0x0300_0002, -1, &b),
                row(0, 4, &parented),
            ],
            &[1i16.to_le_bytes().to_vec(), 9i16.to_le_bytes().to_vec()],
            &[switch],
        );
        v.resolve_names(&[Some(String::new()), Some("door".into()), Some("pit".into())]);
        assert_eq!(v.label(0), "door");
        assert_eq!(v.label(1), "pit");
        assert_eq!(v.label(2), "trigger volume 2");
        assert_eq!(v.triggers[2].parent, Some(4));
        assert_eq!(v.holders(1), [Holder::Kill(0)]);
        assert_eq!(v.kinds(1), [VolumeKind::Kill, VolumeKind::Trigger]);
        assert_eq!(v.kinds(0), [VolumeKind::BspSwitch, VolumeKind::Trigger]);
        assert_eq!(v.caption(0), "door · BSP 1 to 2");
        // The kill trigger naming volume 9 names nothing that exists.
        assert_eq!(v.count(VolumeKind::Kill), 1);
        assert_eq!(v.count(VolumeKind::Trigger), 3);

        let down = |x: f32| Ray {
            origin: [x, 1.0, 30.0],
            dir: [0.0, 0.0, -1.0],
        };
        assert_eq!(v.pick(&down(11.0), |_| true).map(|h| h.1), Some(1));
        assert_eq!(
            v.pick(&down(11.0), |k| k == VolumeKind::Kill).map(|h| h.1),
            Some(1)
        );
        assert_eq!(v.pick(&down(1.0), |k| k == VolumeKind::Kill), None);
        // A parented volume's frame is not in world space: never picked.
        assert_eq!(v.pick(&down(21.0), |_| true), None);
    }

    /// The invented map's two volumes, kill trigger and BSP switch read
    /// back through the scenario, and a session picks and frames them.
    #[test]
    fn the_fixture_volumes_read_pick_and_frame() {
        use crate::fixture::{self, VOLUMES};
        use crate::layers::{LayerId, Layers};
        use crate::{MapSource, Scenario, Session};

        let bytes = fixture::fixture_with_volumes();
        let mut map = map::Map::from_reader(std::io::Cursor::new(bytes.clone())).unwrap();
        let volumes = Scenario::read(&mut map).unwrap().volumes;
        assert_eq!(volumes.error, None);
        assert_eq!(volumes.triggers.len(), 2);
        for (v, (origin, forward, size)) in volumes.triggers.iter().zip(VOLUMES) {
            assert_eq!(v.frame.origin, origin);
            assert_eq!(v.frame.forward, forward);
            assert_eq!(v.frame.size, size);
            assert!(v.in_world());
        }
        assert_eq!(volumes.holders(1), [Holder::Kill(0)]);
        assert_eq!(
            volumes.holders(0),
            [Holder::BspSwitch {
                index: 0,
                from: 0,
                to: 1
            }]
        );
        // The plain fixture has none, and reads as before.
        let mut plain = map::Map::from_reader(std::io::Cursor::new(fixture::fixture())).unwrap();
        assert_eq!(
            Scenario::read(&mut plain).unwrap().volumes,
            Volumes::default()
        );

        let dir =
            std::env::temp_dir().join(format!("scenario-edit-volumes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("volumes.map");
        std::fs::write(&path, &bytes).unwrap();
        let mut ed = Session::new();
        ed.build_scenes = false;
        ed.open(MapSource::file(&path)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        // Straight down through the turned box's middle.
        let kill = volumes.triggers[1].frame.center().unwrap();
        let ray = Ray {
            origin: add(kill, [0.0, 0.0, 50.0]),
            dir: [0.0, 0.0, -1.0],
        };
        let mut layers = Layers::default();
        assert_eq!(ed.pick_volume(&ray, &layers), Some(1));
        layers.set(LayerId::Volume(VolumeKind::Trigger), false);
        assert_eq!(
            ed.pick_volume(&ray, &layers),
            Some(1),
            "the Kill layer still shows it"
        );
        layers.set(LayerId::Volume(VolumeKind::Kill), false);
        assert_eq!(
            ed.pick_volume(&ray, &layers),
            None,
            "a hidden volume is not picked"
        );

        ed.select(Some((crate::BlockKind::Weapons, 0)));
        ed.select_volume(Some(1));
        assert_eq!(ed.selected(), None, "a volume replaces the placements");
        assert_eq!(ed.selected_volume(), Some(1));
        let (centre, radius) = ed.selection_sphere().unwrap();
        assert_eq!(centre, kill);
        assert!((radius - volumes.triggers[1].frame.radius()).abs() < 1e-6);
        ed.select(Some((crate::BlockKind::Weapons, 0)));
        assert_eq!(
            ed.selected_volume(),
            None,
            "a placement replaces the volume"
        );
    }
}
