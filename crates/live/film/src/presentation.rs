//! Sparse, quantised presentation records inside the chronological tick stream.
//! The simulation always consumes its own input log. This dictionary is for viewers.
use anyhow::{Context, Result, bail, ensure};
use live_proto::{
    film::objects::{AnimPose, HillPose, ObjectPose, ObjectiveKind, Scene},
    game::Frame,
    pack,
};
use std::{collections::BTreeMap, f64::consts::TAU};

const CAPACITY: usize = 2048; // retail object-header pool
const MAX_NAME: usize = 256;
const MAX_CORNERS: usize = 256;
const MAX_RECORD: usize = 8 << 20;
const CM_PER_UNIT: f64 = 304.8;
const LANES: usize = 24;
const NAME_BIT: u64 = 1 << 26;
const LEGAL_MASK: u64 = ((1 << LANES) - 1) | NAME_BIT;

fn var(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

fn signed(n: i64, out: &mut Vec<u8>) {
    var(((n as u64) << 1) ^ ((n >> 63) as u64), out);
}

struct Input<'a>(&'a [u8]);
impl<'a> Input<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(n).context("truncated film pose")?;
        self.0 = tail;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn var(&mut self) -> Result<u64> {
        let mut value = 0u64;
        for shift in (0..=63).step_by(7) {
            let byte = self.byte()?;
            ensure!(shift < 63 || byte <= 1, "oversized pose varint");
            value |= u64::from(byte & 127) << shift;
            if byte < 128 {
                ensure!(shift == 0 || byte != 0, "noncanonical pose varint");
                return Ok(value);
            }
        }
        bail!("unterminated pose varint")
    }

    fn signed(&mut self) -> Result<i64> {
        let n = self.var()?;
        Ok(((n >> 1) as i64) ^ -((n & 1) as i64))
    }

    fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.var()?;
        ensure!(n <= max as u64, "pose count exceeds limit");
        Ok(n as usize)
    }
}

fn centimetres(p: [f64; 3]) -> Result<[i32; 3]> {
    let mut out = [0; 3];
    for (v, q) in p.into_iter().zip(&mut out) {
        let cm = (v * CM_PER_UNIT).round();
        ensure!(
            (i32::MIN as f64..=i32::MAX as f64).contains(&cm),
            "invalid pose coordinate"
        );
        *q = cm as i32;
    }
    Ok(out)
}

fn angles(d: [f64; 3]) -> Result<[i64; 2]> {
    ensure!(
        d.iter().all(|v| v.is_finite()) && d.iter().any(|v| *v != 0.),
        "invalid pose direction"
    );
    let [x, y, z] = d;
    let yaw = (y.atan2(x) * (65536.0 / TAU)).round() as i64;
    let pitch = (z.atan2(x.hypot(y)) * (65536.0 / TAU)).round() as i64;
    Ok([yaw.rem_euclid(65536), pitch])
}

fn direction(yaw: i64, pitch: i64) -> [f64; 3] {
    let (sy, cy) = (yaw as f64 * TAU / 65536.0).sin_cos();
    let (sp, cp) = (pitch as f64 * TAU / 65536.0).sin_cos();
    [cy * cp, sy * cp, sp]
}

fn opt<T: Into<i64>>(n: Option<T>) -> i64 {
    n.map_or(0, |v| v.into() + 1)
}

#[derive(Clone, PartialEq, Eq)]
struct Pose {
    v: [i64; LANES],
    name: Option<String>,
}
impl Default for Pose {
    fn default() -> Self {
        let mut v = [0; LANES];
        v[11] = 16384; // world +Z
        Self { v, name: None }
    }
}

impl Pose {
    fn quantise(o: &ObjectPose) -> Result<Self> {
        let mut q = Self::default();
        let v = &mut q.v;
        v[0] = i64::from(o.kind);
        v[1] = opt(o.definition);
        v[2] = opt(o.placement);
        v[3] = opt(o.spawn);
        v[4] = opt(o.parent);
        v[5..8].copy_from_slice(&centimetres(o.pos)?.map(i64::from));
        v[8..10].copy_from_slice(&angles(o.forward)?);
        v[10..12].copy_from_slice(&angles(o.up)?);
        v[12] = opt(o.seat);
        if let Some(d) = o.aim {
            v[13] = 1;
            v[14..16].copy_from_slice(&angles(d)?);
        }
        if let Some(a) = o.animation {
            v[16] = opt(Some(a.graph));
            v[17] = i64::from(a.inherited);
            v[18] = i64::from(a.index);
            ensure!((0.0..32768.0).contains(&a.frame), "invalid animation frame");
            v[19] = (f64::from(a.frame) * 16.0).round() as i64;
        }
        if let Some(value) = o.travel {
            ensure!((0.0..=1.0).contains(&value), "invalid device travel");
            v[20] = 1 + (value * 255.).round() as i64;
        }
        v[21] = i64::from(o.depleted);
        v[22] = opt(o.player);
        v[23] = o.objective.map_or(0, |obj| obj as i64);
        q.name = o.name.clone();
        q.validate()?;
        Ok(q)
    }

    fn validate(&self) -> Result<()> {
        let v = &self.v;
        ensure!((0..=12).contains(&v[0]), "unknown guest object class");
        for lane in [1, 4, 16, 22] {
            ensure!(
                (0..=1 + i64::from(u32::MAX)).contains(&v[lane]),
                "invalid pose datum"
            );
        }
        for lane in [2, 12] {
            ensure!((0..=65536).contains(&v[lane]), "invalid placement or seat");
        }
        ensure!(
            (0..=256).contains(&v[3]) && (0..=256).contains(&v[20]),
            "invalid spawn or travel"
        );
        ensure!(
            v[5..8].iter().all(|n| i32::try_from(*n).is_ok()),
            "invalid pose coordinate"
        );
        for lane in [9, 11, 15] {
            ensure!((-16384..=16384).contains(&v[lane]), "invalid pose pitch");
        }
        ensure!(
            matches!(v[13], 0 | 1) && matches!(v[21], 0 | 1),
            "invalid pose flags"
        );
        ensure!(
            v[13] != 0 || v[14..16] == [0, 0],
            "aim absent with direction"
        );
        ensure!(
            v[16] != 0 || v[17..20] == [0, 0, 0],
            "animation absent with state"
        );
        ensure!(
            i16::try_from(v[17]).is_ok()
                && i16::try_from(v[18]).is_ok()
                && i32::try_from(v[19]).is_ok(),
            "invalid animation"
        );
        ensure!(matches!(v[23], 0 | 1 | 3 | 9), "invalid objective kind");
        ensure!(
            self.name
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= MAX_NAME),
            "invalid pose name"
        );
        Ok(())
    }

    fn unpack(&self, datum: u32) -> ObjectPose {
        let v = &self.v;
        ObjectPose {
            datum,
            kind: v[0] as u8,
            name: self.name.clone(),
            definition: (v[1] > 0).then(|| (v[1] - 1) as u32),
            placement: (v[2] > 0).then(|| (v[2] - 1) as u16),
            spawn: (v[3] > 0).then(|| (v[3] - 1) as u8),
            parent: (v[4] > 0).then(|| (v[4] - 1) as u32),
            pos: [v[5], v[6], v[7]].map(|n| n as f64 / CM_PER_UNIT),
            forward: direction(v[8], v[9]),
            up: direction(v[10], v[11]),
            seat: (v[12] > 0).then(|| (v[12] - 1) as u16),
            aim: (v[13] != 0).then(|| direction(v[14], v[15])),
            animation: (v[16] > 0).then(|| AnimPose {
                graph: (v[16] - 1) as u32,
                inherited: v[17] as i16,
                index: v[18] as i16,
                frame: v[19] as f32 / 16.0,
            }),
            travel: (v[20] > 0).then(|| (v[20] - 1) as f64 / 255.0),
            depleted: v[21] != 0,
            player: (v[22] > 0).then(|| (v[22] - 1) as u32),
            objective: match v[23] {
                1 => Some(ObjectiveKind::Flag),
                3 => Some(ObjectiveKind::Ball),
                9 => Some(ObjectiveKind::Bomb),
                _ => None,
            },
        }
    }

    fn write(&self, old: &Self, dst: &mut Vec<u8>) {
        let mut mask = if self.name != old.name { NAME_BIT } else { 0 };
        for i in 0..LANES {
            if self.v[i] != old.v[i] {
                mask |= 1 << i;
            }
        }
        var(mask, dst);
        for i in 0..LANES {
            if mask & (1 << i) != 0 {
                let delta = self.v[i] - old.v[i];
                signed(
                    if matches!(i, 8 | 10 | 14) {
                        i64::from(delta as i16)
                    } else {
                        delta
                    },
                    dst,
                );
            }
        }
        if mask & NAME_BIT != 0 {
            let s = self.name.as_deref().unwrap_or_default().as_bytes();
            var(s.len() as u64, dst);
            dst.extend_from_slice(s);
        }
    }

    fn update(&mut self, src: &mut Input<'_>) -> Result<()> {
        let mask = src.var()?;
        ensure!(mask & !LEGAL_MASK == 0, "unknown pose lane");
        for i in 0..LANES {
            if mask & (1 << i) != 0 {
                let delta = src.signed()?;
                let sum = self.v[i].checked_add(delta).context("pose overflow")?;
                self.v[i] = if matches!(i, 8 | 10 | 14) {
                    ensure!(i16::try_from(delta).is_ok(), "invalid yaw delta");
                    sum.rem_euclid(65536)
                } else {
                    sum
                };
            }
        }
        if mask & NAME_BIT != 0 {
            let n = src.count(MAX_NAME)?;
            let s = std::str::from_utf8(src.take(n)?)?;
            self.name = (!s.is_empty()).then(|| s.to_owned());
        }
        self.validate()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Hill {
    index: u8,
    corners: Vec<[i32; 3]>,
}
impl Hill {
    fn quantise(h: &HillPose) -> Result<Self> {
        ensure!(h.corners.len() <= MAX_CORNERS, "too many hill corners");
        Ok(Self {
            index: h.index,
            corners: h
                .corners
                .iter()
                .copied()
                .map(centimetres)
                .collect::<Result<_>>()?,
        })
    }
}

#[derive(Default)]
struct Dictionary {
    objects: BTreeMap<u32, Pose>,
    hill: Option<Hill>,
}

impl Dictionary {
    fn from_scene(scene: &Scene, prior: &Self) -> Result<(Self, Dropped)> {
        ensure!(
            scene
                .objects
                .len()
                .saturating_add(scene.missing.objects.len())
                <= CAPACITY,
            "too many guest objects"
        );
        let mut dropped = Dropped::default();
        // None marks an allocated but unreadable body. Distinguish this from
        // an absent slot so another object can still move or despawn.
        let mut rows: Vec<_> = scene
            .objects
            .iter()
            .map(|o| (o.datum, Some(o)))
            .chain(scene.missing.objects.iter().map(|&id| (id, None)))
            .collect();
        rows.sort_unstable_by_key(|row| row.0);
        ensure!(
            rows.last().is_none_or(|row| row.0 != u32::MAX)
                && !rows.windows(2).any(|pair| pair[0].0 == pair[1].0),
            "invalid census datum"
        );
        let mut objects = BTreeMap::new();
        for (id, row) in rows {
            let old = prior.objects.get(&id);
            let pose = if let Some(mut q) = row.and_then(|o| Pose::quantise(o).ok()) {
                if scene.missing.objectives {
                    q.v[23] = old.map_or(0, |p| p.v[23]);
                }
                Some(q)
            } else {
                dropped.objects += 1;
                old.cloned()
            };
            if let Some(pose) = pose {
                objects.insert(id, pose);
            }
        }
        dropped.objectives = scene.missing.objectives;
        let hill = match (
            scene.missing.hill,
            scene.hill.as_ref().map(Hill::quantise).transpose(),
        ) {
            (false, Ok(hill)) => hill,
            _ => {
                dropped.hill = true;
                prior.hill.clone()
            }
        };
        Ok((Self { objects, hill }, dropped))
    }

    fn changes(&self, old: &Self, baseline: bool) -> Option<Vec<u8>> {
        let empty = Pose::default();
        let mut changes = Vec::new();
        for (&id, obj) in &self.objects {
            match old.objects.get(&id) {
                None => changes.push((id, 0, Some((obj, &empty)))),
                Some(prior) if prior != obj => changes.push((id, 1, Some((obj, prior)))),
                _ => {}
            }
        }
        for &id in old.objects.keys() {
            if !self.objects.contains_key(&id) {
                changes.push((id, 2, None));
            }
        }
        if !baseline && changes.is_empty() && self.hill == old.hill {
            return None;
        }
        changes.sort_unstable_by_key(|c| c.0);
        let mut out = vec![if baseline { 0 } else { 1 }];
        var(changes.len() as u64, &mut out);
        let mut prev = 0;
        for (id, kind, diff) in changes {
            var(u64::from(id - prev) * 4 + kind, &mut out);
            if let Some((obj, prior)) = diff {
                obj.write(prior, &mut out);
            }
            prev = id;
        }
        if self.hill == old.hill {
            out.push(0);
        } else if let Some(h) = &self.hill {
            out.extend([2, h.index]);
            var(h.corners.len() as u64, &mut out);
            for corner in &h.corners {
                for &n in corner {
                    signed(i64::from(n), &mut out);
                }
            }
        } else {
            out.push(1);
        }
        Some(out)
    }
}

/// Rejected rows in a complete object-header census. An invalid census
/// itself returns an error and leaves the dictionary untouched.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Dropped {
    pub objects: u64,
    pub hill: bool,
    pub objectives: bool,
}

/// Scene-only deltas and additional renderer fields use the same outer tag.
/// A baseline starts a fresh supplemental v8 predictor for each HTTP range.
#[derive(Default)]
pub struct Encoder {
    scene: Dictionary,
    extras: Option<Frame>,
    packer: Option<pack::Encoder>,
}

impl Encoder {
    /// The first changed supplemental sample in each time chunk is a v8 key.
    /// Sleeping objects and unchanged player appearance still take no records.
    pub fn begin_chunk(&mut self) {
        self.packer = None;
    }

    pub fn push(&mut self, scene: &Scene) -> Result<Option<Vec<u8>>> {
        self.push_observed(scene).map(|(record, _)| record)
    }

    pub fn push_observed(&mut self, scene: &Scene) -> Result<(Option<Vec<u8>>, Dropped)> {
        let (next, drops) = Dictionary::from_scene(scene, &self.scene)?;
        let record = next.changes(&self.scene, false);
        self.scene = next;
        Ok((record, drops))
    }

    pub fn baseline(&mut self) -> Result<Vec<u8>> {
        self.baseline_observed().map(|(bytes, _)| bytes)
    }

    /// An unrepresentable supplemental key must not suppress the required
    /// object checkpoint. The next valid supplemental frame starts with a key.
    pub fn baseline_observed(&mut self) -> Result<(Vec<u8>, bool)> {
        let mut out = self
            .scene
            .changes(&Dictionary::default(), true)
            .expect("baseline is present");
        let mut packer = pack::Encoder::with_version(0, u64::MAX, 8)?;
        let packed = self.extras.as_ref().map(|f| packer.encode(f)).transpose();
        let dropped = packed.is_err();
        if dropped {
            self.extras = None;
        }
        let bytes = packed
            .ok()
            .flatten()
            .map(|(bytes, _)| bytes)
            .unwrap_or_default();
        var(bytes.len() as u64, &mut out);
        out.extend_from_slice(&bytes);
        self.packer = Some(packer);
        Ok((out, dropped))
    }

    /// Retain the named animation/weapon/stance, vitality and timestamped
    /// actions used by the existing web renderer. The object dictionary owns
    /// known transforms. A fallen player's last known location remains useful
    /// after its retail object has been destroyed.
    pub fn frame(&mut self, source: &Frame) -> Result<Option<Vec<u8>>> {
        self.frame_observed(source).map(|(record, _)| record)
    }

    pub fn frame_observed(&mut self, source: &Frame) -> Result<(Option<Vec<u8>>, u64)> {
        use live_proto::game;
        ensure!(
            source.players.len() <= game::MAX_PLAYERS
                && source.actors.len() <= game::MAX_FRAME_ACTORS,
            "invalid unit census"
        );
        let mut dropped = 0;
        let mut f = Frame {
            game_id: source.game_id.clone(),
            tick: source.tick,
            bsp: source.bsp,
            cinematic: source.cinematic,
            cut: source.cut,
            players: source.players.clone(),
            actors: source.actors.clone(),
            ..Frame::default()
        };
        let players: BTreeMap<_, _> = self
            .scene
            .objects
            .values()
            .filter(|p| p.v[22] != 0 && p.v[0] == 0)
            .map(|p| (((p.v[22] - 1) as u32) & 0xffff, p))
            .collect();

        // The packed telemetry schema has separate but structurally identical
        // fields on players and AI units.
        macro_rules! supplement {
            ($unit:ident, $pose:expr) => {{
                // The typed damage and weapon-fire events also drive browser FX.
                $unit.shots = None;
                $unit.fire = None;
                let pose: Option<&Pose> = $pose;
                if let Some(pose) = pose {
                    $unit.pos = [0.0; 3];
                    $unit.yaw = 0.0;
                    $unit.aim = None;
                    $unit.vehicle = None;
                    $unit.seat = None;
                    if pose.v[16] != 0 {
                        if let Some(a) = &mut $unit.anim {
                            a.frame = 0.;
                        }
                    }
                }
                fn quantise(x: f64, scale: f64) -> f64 {
                    (x * scale).round() / scale
                }
                $unit.vel = $unit.vel.map(|v| v.map(|n| quantise(n, pack::VEL_SCALE)));
                $unit.speed = $unit.speed.map(|n| quantise(n, pack::SPEED_SCALE));
                $unit.body = $unit.body.map(|n| quantise(n, 63.));
                $unit.shield = $unit.shield.map(|n| quantise(n, pack::SHIELD_SCALE));
                if let Some(a) = &mut $unit.anim {
                    a.frame = quantise(a.frame, pack::ANIM_SCALE);
                }
            }};
        }
        f.players.retain_mut(|p| {
            if game::validate_frame_player(p).is_err() || centimetres(p.pos).is_err() {
                dropped += 1;
                if let Some(old) = self
                    .extras
                    .as_ref()
                    .and_then(|f| f.players.iter().find(|old| old.index == p.index))
                {
                    p.clone_from(old);
                    return true;
                }
                return false;
            }
            supplement!(p, p.alive.then(|| players.get(&p.index).copied()).flatten());
            true
        });
        f.actors.retain_mut(|a| {
            if game::validate_frame_actor(a).is_err() || centimetres(a.pos).is_err() {
                dropped += 1;
                if let Some(old) = self
                    .extras
                    .as_ref()
                    .and_then(|f| f.actors.iter().find(|old| old.id == a.id))
                {
                    a.clone_from(old);
                    return true;
                }
                return false;
            }
            let pose = u32::from_str_radix(&a.id, 16)
                .ok()
                .and_then(|id| self.scene.objects.get(&id));
            supplement!(a, pose);
            true
        });
        let tick = f.tick;
        if let Some(prior) = &self.extras {
            f.tick = prior.tick;
            if f == *prior {
                return Ok((None, dropped));
            }
            f.tick = tick;
        }
        if self.packer.is_none() {
            self.packer = Some(pack::Encoder::with_version(0, u64::MAX, 8)?);
        }
        let (bytes, _) = self.packer.as_mut().unwrap().encode(&f)?;
        let mut out = Vec::with_capacity(bytes.len() + 1);
        out.push(2);
        out.extend(bytes);
        clear_transients(&mut f);
        self.extras = Some(f);
        Ok((Some(out), dropped))
    }
}

fn clear_transients(f: &mut Frame) {
    f.cut = None;
    for p in &mut f.players {
        p.actions = None;
        p.actions_ago = None;
    }
    for a in &mut f.actors {
        a.actions = None;
        a.actions_ago = None;
    }
}

/// Persistent pose dictionary for inspecting or converting a fetched TICK.
/// Begin with the baseline in that range. Every later delta applies once.
#[derive(Default)]
pub struct Decoder {
    scene: Dictionary,
    packer: pack::Decoder,
    extras: Option<Frame>,
}

impl Decoder {
    /// Decoded transforms in guest world units, sorted by salted datum.
    pub fn scene(&self) -> Scene {
        Scene {
            missing: Default::default(),
            objects: self
                .scene
                .objects
                .iter()
                .map(|(&id, o)| o.unpack(id))
                .collect(),
            hill: self.scene.hill.as_ref().map(|h| HillPose {
                index: h.index,
                corners: h
                    .corners
                    .iter()
                    .map(|c| c.map(|n| f64::from(n) / CM_PER_UNIT))
                    .collect(),
            }),
        }
    }

    pub fn extras(&self) -> Option<&Frame> {
        self.extras.as_ref()
    }

    pub fn apply(&mut self, raw: &[u8]) -> Result<()> {
        ensure!(raw.len() <= MAX_RECORD, "oversized film pose");
        let mut src = Input(raw);
        let opcode = src.byte()?;
        if opcode == 2 {
            check_header(src.0, false)?;
            self.extras = Some(self.packer.decode(src.0)?.frame);
            return Ok(());
        }
        ensure!(opcode < 2, "unknown film pose record");
        let baseline = opcode == 0;
        let max_ops = if baseline { CAPACITY } else { 2 * CAPACITY };
        let count = src.count(max_ops)?;
        let mut changes = Vec::with_capacity(count);
        let mut id = 0u32;
        let mut size = if baseline {
            0
        } else {
            self.scene.objects.len()
        };
        for index in 0..count {
            let h = src.var()?;
            let step = u32::try_from(h >> 2)?;
            ensure!(step != 0 || index == 0, "duplicate pose edit");
            id = id.checked_add(step).context("object datum overflow")?;
            ensure!(id != u32::MAX, "invalid null object datum");
            let old = (!baseline).then(|| self.scene.objects.get(&id)).flatten();
            let obj = match h & 3 {
                0 => {
                    ensure!(old.is_none(), "creating live object");
                    size += 1;
                    let mut o = Pose::default();
                    o.update(&mut src)?;
                    Some(o)
                }
                1 => {
                    let mut o = old.context("updating missing object")?.clone();
                    o.update(&mut src)?;
                    Some(o)
                }
                2 => {
                    ensure!(old.is_some(), "destroying missing object");
                    size -= 1;
                    None
                }
                _ => bail!("unknown object operation"),
            };
            changes.push((id, obj));
        }
        ensure!(size <= CAPACITY, "too many live film objects");
        let hill = match src.byte()? {
            0 => {
                if baseline {
                    None
                } else {
                    self.scene.hill.clone()
                }
            }
            1 => None,
            2 => {
                let index = src.byte()?;
                let len = src.count(MAX_CORNERS)?;
                let mut corners = Vec::with_capacity(len);
                for _ in 0..len {
                    let mut c = [0; 3];
                    for v in &mut c {
                        *v = i32::try_from(src.signed()?)?;
                    }
                    corners.push(c);
                }
                Some(Hill { index, corners })
            }
            _ => bail!("unknown hill edit"),
        };
        if baseline {
            let n = src.count(MAX_RECORD)?;
            let packed = src.take(n)?;
            ensure!(src.0.is_empty(), "trailing pose bytes");
            let mut packer = pack::Decoder::new();
            let frame = if packed.is_empty() {
                None
            } else {
                check_header(packed, true)?;
                let decoded = packer.decode(packed)?;
                ensure!(decoded.seq == 0, "invalid pose key sequence");
                Some(decoded.frame)
            };
            self.scene.objects.clear();
            self.extras = frame;
            self.packer = packer;
        }
        ensure!(src.0.is_empty(), "trailing pose bytes");
        self.scene.hill = hill;
        for (id, edit) in changes {
            if let Some(o) = edit {
                self.scene.objects.insert(id, o);
            } else {
                self.scene.objects.remove(&id);
            }
        }
        Ok(())
    }
}

fn check_header(bytes: &[u8], baseline: bool) -> Result<()> {
    let header = *bytes.first().context("empty supplemental pose frame")?;
    ensure!(
        matches!(header, 8 | 24) && (!baseline || header == 24),
        "invalid supplemental pose header"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::game::{FrameAnim, FramePlayer};

    fn item(id: u32) -> ObjectPose {
        ObjectPose {
            datum: id,
            kind: 2,
            definition: None,
            name: None,
            placement: None,
            spawn: None,
            parent: None,
            pos: [0.; 3],
            forward: [1., 0., 0.],
            up: [0., 0., 1.],
            seat: None,
            aim: None,
            animation: None,
            travel: None,
            depleted: false,
            player: None,
            objective: None,
        }
    }

    #[test]
    fn canonical_wire_and_sleeping_objects_cost_nothing() {
        let mut enc = Encoder::default();
        let mut s = Scene {
            missing: Default::default(),
            objects: vec![item(1)],
            hill: None,
        };
        let mut dec = Decoder::default();
        let created = enc.push(&s).unwrap().unwrap();
        assert_eq!(created, [1, 1, 4, 1, 4, 0]);
        dec.apply(&created).unwrap();
        assert_eq!(enc.baseline().unwrap(), [0, 1, 4, 1, 4, 0, 0]);
        assert!(enc.push(&s).unwrap().is_none());
        s.objects[0].pos[0] = 1. / CM_PER_UNIT;
        let moved = enc.push(&s).unwrap().unwrap();
        assert_eq!(moved, [1, 1, 5, 32, 2, 0]);
        dec.apply(&moved).unwrap();
        assert_eq!(dec.scene().objects[0].pos[0], s.objects[0].pos[0]);
        s.objects[0].pos[0] += 0.4 / CM_PER_UNIT;
        assert!(enc.push(&s).unwrap().is_none());
        s.objects.clear();
        let destroyed = enc.push(&s).unwrap().unwrap();
        assert_eq!(destroyed, [1, 1, 6, 0]);
        dec.apply(&destroyed).unwrap();
        assert!(dec.scene().objects.is_empty());
    }

    #[test]
    fn graph_pose_hill_and_active_objects_seek_without_prior_samples() {
        let mut enc = Encoder::default();
        let mut s = Scene {
            missing: Default::default(),
            objects: (0..=12)
                .map(|kind| {
                    let mut o = item(0xffff_f000 + u32::from(kind));
                    o.kind = kind;
                    o.name = Some(format!("object_{kind}"));
                    o.definition = Some(0xffff_fffe);
                    o.parent = Some(0xe001_0134);
                    o.pos = [0.1234567, -412.00933, 6.921];
                    o.forward = direction(65535, -1234);
                    o.aim = Some(direction(2, 123));
                    o.seat = Some(5);
                    o.animation = Some(AnimPose {
                        graph: 0x8000_0032,
                        inherited: -1,
                        index: 63,
                        frame: 61.02,
                    });
                    o.objective = Some(ObjectiveKind::Bomb);
                    o.travel = Some(0.425);
                    o
                })
                .collect(),
            hill: Some(HillPose {
                index: 7,
                corners: vec![[1.0, 13.25, -3.0], [7.4, 9.1, -3.0]],
            }),
        };
        enc.push(&s).unwrap();
        let mut dec = Decoder::default();
        dec.apply(&enc.baseline().unwrap()).unwrap();
        let copy = dec.scene();
        for (a, b) in s.objects.iter().zip(&copy.objects) {
            assert_eq!(
                (a.kind, a.datum, a.seat, a.parent, a.objective),
                (b.kind, b.datum, b.seat, b.parent, b.objective)
            );
            for (left, right) in a.pos.iter().zip(b.pos) {
                assert!((left - right).abs() * CM_PER_UNIT <= 0.500001);
            }
            assert!((a.animation.unwrap().frame - b.animation.unwrap().frame).abs() <= 1.0 / 32.0);
            assert!((a.travel.unwrap() - b.travel.unwrap()).abs() <= 0.5 / 255.0);
            assert_eq!(a.aim, b.aim);
        }
        assert_eq!(copy.hill.unwrap().index, 7);
        s.objects[0].aim = None;
        s.objects[0].animation = None;
        s.objects[0].forward = direction(1, -1234);
        s.hill = None;
        dec.apply(&enc.push(&s).unwrap().unwrap()).unwrap();
        let copy = dec.scene();
        assert!(copy.hill.is_none());
        assert_eq!(copy.objects[0].forward, s.objects[0].forward);
        assert!(copy.objects[0].animation.is_none());
        assert!(copy.objects[0].aim.is_none());
    }

    #[test]
    fn damaged_rows_hold_their_last_good_values_while_other_slots_change() {
        let initial = Scene {
            objects: vec![item(2), item(5), item(9), item(10)],
            hill: Some(HillPose {
                index: 1,
                corners: vec![[2., 0., 0.]],
            }),
            ..Default::default()
        };
        let mut enc = Encoder::default();
        let mut viewer = Decoder::default();
        viewer.apply(&enc.push(&initial).unwrap().unwrap()).unwrap();

        let mut next = initial.clone();
        next.objects[0].forward[0] = f64::NAN;
        next.objects[1].pos[0] = 20.;
        next.objects.truncate(2);
        next.objects.push(item(13));
        next.missing.objects.push(9); // inaccessible body, but still allocated
        next.hill.as_mut().unwrap().corners[0][1] = f64::INFINITY;
        let (update, drops) = enc.push_observed(&next).unwrap();
        assert_eq!(
            drops,
            Dropped {
                objects: 2,
                hill: true,
                objectives: false
            }
        );
        viewer.apply(&update.unwrap()).unwrap();
        let decoded = viewer.scene();
        assert_eq!(
            decoded.objects.iter().map(|o| o.datum).collect::<Vec<_>>(),
            [2, 5, 9, 13]
        );
        assert_eq!(decoded.objects[0].forward, initial.objects[0].forward);
        assert_eq!(decoded.objects[1].pos[0], 20.);
        assert_eq!(decoded.objects[2].pos, initial.objects[2].pos);
        assert_eq!(decoded.hill.as_ref().unwrap().index, 1);

        let mut recovered = next;
        recovered.missing.objects.clear();
        recovered.objects[0].forward = [0., 1., 0.];
        recovered.hill = None;
        let (update, drops) = enc.push_observed(&recovered).unwrap();
        assert_eq!(drops, Dropped::default());
        viewer.apply(&update.unwrap()).unwrap();
        let decoded = viewer.scene();
        assert_eq!(decoded.objects.len(), 3);
        assert!(decoded.objects[0].forward[1] > 0.9999);
        assert!(decoded.hill.is_none());
    }

    #[test]
    fn nonfinite_player_sample_preserves_only_that_player() {
        let mut enc = Encoder::default();
        let mut viewer = Decoder::default();
        let player = |index| FramePlayer {
            index,
            alive: true,
            ..Default::default()
        };
        let mut f = Frame {
            game_id: "fixture".into(),
            tick: 1,
            players: vec![player(0), player(2)],
            ..Default::default()
        };
        viewer.apply(&enc.frame(&f).unwrap().unwrap()).unwrap();
        f.tick += 2;
        f.players[0].yaw = f64::NAN;
        f.players[1].weapon = Some("battle_rifle".into());
        let (bytes, drops) = enc.frame_observed(&f).unwrap();
        assert_eq!(drops, 1);
        viewer.apply(&bytes.unwrap()).unwrap();
        let p = &viewer.extras().unwrap().players;
        assert_eq!(p[0].yaw, 0.);
        assert_eq!(p[1].weapon.as_deref(), Some("battle_rifle"));
        f.tick += 2;
        f.players[0].yaw = 1.;
        let (bytes, drops) = enc.frame_observed(&f).unwrap();
        assert_eq!(drops, 0);
        viewer.apply(&bytes.unwrap()).unwrap();
        assert!((viewer.extras().unwrap().players[0].yaw - 1.).abs() < 0.001);
    }

    #[test]
    fn position_and_angles_meet_quantisation_bounds() {
        for i in -1000..1000 {
            let a = f64::from(i) * 0.23567;
            let b = a.sin();
            let d = [a.cos() * b.cos(), a.sin() * b.cos(), b.sin()];
            let [yaw, pitch] = angles(d).unwrap();
            let reconstructed = direction(yaw, pitch);
            let dot: f64 = d.into_iter().zip(reconstructed).map(|(a, b)| a * b).sum();
            assert!(dot.clamp(-1., 1.).acos() <= 2.0_f64.sqrt() * TAU / 131072.0 + 1e-9);
            let cm = centimetres([a, b, -a]).unwrap();
            for (actual, n) in [a, b, -a].into_iter().zip(cm) {
                assert!((actual * CM_PER_UNIT - f64::from(n)).abs() <= 0.500001);
            }
        }
    }

    #[test]
    fn baseline_extras_are_self_contained_and_one_shots_do_not_repeat_on_seek() {
        let mut enc = Encoder::default();
        let f = Frame {
            tick: 192,
            game_id: "g".into(),
            players: vec![FramePlayer {
                index: 1,
                alive: true,
                pos: [1., 2., 3.],
                shots: Some(1),
                fire: Some(1),
                actions: Some(vec!["jump".into()]),
                anim: Some(FrameAnim {
                    name: "idle".into(),
                    frame: 4.,
                }),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut dec = Decoder::default();
        dec.apply(&enc.frame(&f).unwrap().unwrap()).unwrap();
        let player = &dec.extras().unwrap().players[0];
        assert!(player.shots.is_none() && player.fire.is_none());
        assert_eq!(player.actions.as_ref().unwrap(), &["jump"]);
        let mut fresh = Decoder::default();
        fresh.apply(&enc.baseline().unwrap()).unwrap();
        assert!(fresh.extras().unwrap().players[0].actions.is_none());
        let mut next = f;
        clear_transients(&mut next);
        next.tick += 2;
        assert!(enc.frame(&next).unwrap().is_none());
        next.players[0].weapon = Some("battle_rifle".into());
        fresh.apply(&enc.frame(&next).unwrap().unwrap()).unwrap();
        assert_eq!(
            fresh.extras().unwrap().players[0].weapon.as_deref(),
            Some("battle_rifle")
        );
    }

    #[test]
    fn invalid_and_truncated_edits_leave_previous_scene_intact() {
        let mut dec = Decoder::default();
        let mut enc = Encoder::default();
        let scene = Scene {
            missing: Default::default(),
            objects: vec![item(0x8000_0001)],
            hill: None,
        };
        enc.push(&scene).unwrap();
        let baseline = enc.baseline().unwrap();
        dec.apply(&baseline).unwrap();
        let unchanged = dec.scene();
        for n in 0..baseline.len() {
            assert!(dec.apply(&baseline[..n]).is_err());
            assert_eq!(dec.scene(), unchanged);
        }
        for invalid in [
            &[1, 1, 6, 0][..],
            &[1, 0x80, 0, 0],
            &[3],
            &[1, 1, 0, 0x80, 0x80, 0x80, 0x08, 0, 0],
            &[2, 0],
            &[1, 0, 2, 1, 0x81, 0x02],
        ] {
            assert!(dec.apply(invalid).is_err());
            assert_eq!(dec.scene(), unchanged);
        }
    }
}
