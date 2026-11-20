//! Account-owned, map-local memories. The caller can serialize a snapshot
//! between sessions. Nothing here needs access to the object table.
use crate::brain::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Region {
    cell: [i32; 3],
    kills: u16,
    visits: u16,
    finds: u16,
}

/// Versioned wire data for one bot account. Positions are coarse map
/// regions. No living opponent, clock or item availability is persisted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    version: u8,
    maps: BTreeMap<String, Vec<Region>>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            maps: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Experience {
    history: Snapshot,
    map: String,
    last_visit: Option<Vec3>,
    last_shot: Option<(Vec3, u32, u32)>,
}

fn cell(p: Vec3) -> [i32; 3] {
    [
        (p[0] / 4.).floor() as i32,
        (p[1] / 4.).floor() as i32,
        (p[2] / 2.).floor() as i32,
    ]
}

impl Experience {
    pub fn snapshot(&self) -> Snapshot {
        self.history.clone()
    }

    pub fn restore(&mut self, history: Snapshot) -> Result<(), &'static str> {
        if history.version != 1
            || history.maps.len() > 128
            || history.maps.iter().any(|(name, regions)| {
                name.is_empty()
                    || name.len() > 128
                    || regions.len() > 256
                    || regions.iter().any(|r| r.finds > r.visits)
            })
        {
            return Err("unsupported or invalid bot map history");
        }
        self.history = history;
        self.new_life();
        Ok(())
    }

    pub(crate) fn enter(&mut self, name: &str) {
        if self.map != name {
            self.map = name.to_owned();
            self.new_life();
        }
    }

    pub(crate) fn new_life(&mut self) {
        self.last_visit = None;
        self.last_shot = None;
    }

    fn record(&mut self, at: Vec3) -> Option<&mut Region> {
        if self.map.is_empty() || self.map.len() > 128 || !at.iter().all(|f| f.is_finite()) {
            return None;
        }
        if !self.history.maps.contains_key(&self.map) && self.history.maps.len() == 128 {
            return None;
        }
        let regions = self.history.maps.entry(self.map.clone()).or_default();
        let cell = cell(at);
        let i = if let Some(i) = regions.iter().position(|r| r.cell == cell) {
            i
        } else if regions.len() < 256 {
            regions.push(Region {
                cell,
                kills: 0,
                visits: 0,
                finds: 0,
            });
            regions.len() - 1
        } else {
            return None;
        };
        regions.get_mut(i)
    }

    pub(crate) fn fired(&mut self, at: Vec3, target: u32, tick: u32) {
        self.last_shot = Some((at, target, tick));
    }
    pub(crate) fn own_kill(&mut self, victim: u32, tick: u32) {
        if let Some((at, target, since)) = self.last_shot
            && target == victim
            && tick.wrapping_sub(since) <= 30
        {
            self.last_shot = None;
            if let Some(r) = self.record(at) {
                r.kills = r.kills.saturating_add(1);
            }
        }
    }

    pub(crate) fn visit(&mut self, feet: Vec3, observation: Option<(Vec3, bool)>) {
        if self
            .last_visit
            .is_some_and(|at| crate::decisions::distance(feet, at) > 3.)
        {
            self.last_visit = None;
        }
        if let Some((at, present)) = observation
            && self.last_visit != Some(at)
        {
            self.last_visit = Some(at);
            if let Some(r) = self.record(at) {
                if r.visits >= 1024 {
                    r.visits /= 2;
                    r.finds /= 2;
                }
                r.visits += 1;
                r.finds += u16::from(present);
            }
        }
    }

    fn region(&self, at: Vec3) -> Option<&Region> {
        let cell = cell(at);
        self.history
            .maps
            .get(&self.map)?
            .iter()
            .find(|r| r.cell == cell)
    }

    pub(crate) fn success(&self, at: Vec3) -> f32 {
        self.region(at).map_or(0., |r| {
            (2. * f32::from(r.finds) - f32::from(r.visits)) / (4. + f32::from(r.visits))
        })
    }

    pub(crate) fn firing_position(&self, at: Vec3) -> f32 {
        self.region(at)
            .map_or(0., |r| f32::from(r.kills) / (4. + f32::from(r.kills)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_keep_map_specific_outcomes_and_discard_in_flight_evidence() {
        let mut m = Experience::default();
        let at = [12., 2., 0.];
        m.enter("h1_bloodgulch");
        m.fired(at, 2, 10);
        m.own_kill(3, 12);
        assert_eq!(m.firing_position(at), 0.);
        m.own_kill(2, 15);
        for _ in 0..40 {
            m.visit(at, Some((at, true)));
        }
        assert_eq!(m.success(at), 0.2);
        m.enter("lockout");
        assert_eq!(m.firing_position(at), 0.);
        let mut restored = Experience::default();
        let bytes = serde_json::to_vec(&m.snapshot()).unwrap();
        restored
            .restore(serde_json::from_slice(&bytes).unwrap())
            .unwrap();
        restored.enter("h1_bloodgulch");
        assert_eq!(restored.success(at), 0.2);
        assert_eq!(restored.firing_position(at), 0.2);
        restored.visit(at, Some((at, false)));
        assert_eq!(restored.success(at), 0.);
    }
}
