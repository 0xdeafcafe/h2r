//! A ship's flights on the host: the keyed path the host flies a vehicle
//! along with no AI pilot (docs/game/rules-world-api.md, "Vehicles and
//! drop ships"). The pose each tick is `rules::vehicles::Flight`'s, the
//! same integer math the fake runs, so a machine that replays the entry
//! and a film put the ship at the same place on the same tick.
//!
//! [`Flights`] holds no engine state: [`Flights::step`] says where each
//! ship is on a tick, and the caller puts it there at the tick boundary.

use std::collections::BTreeMap;

use ::rules::vehicles::{Flight, ONE};

use super::super::objects::{from_fixed, to_fixed};

/// The most ships in the air at once.
pub const MAX_FLIGHTS: usize = 4;

/// Where a ship is on a tick, in world units, and the unit tangent it faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub object: u32,
    pub at: [f32; 3],
    pub forward: [f32; 3],
}

/// What a tick's step did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stepped {
    /// Each flying ship's place, in object order.
    pub steps: Vec<Step>,
    /// The ships whose flight ended on this tick, at the path's last point.
    pub arrived: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flights {
    flights: BTreeMap<u32, Flight>,
}

impl Flights {
    /// Starts `object` along `path` on `tick`, taking `ticks`. A second
    /// flight of a ship replaces the first, which ends with no arrival.
    pub fn begin(&mut self, object: u32, path: &[[f32; 3]], tick: u32, ticks: u32) -> anyhow::Result<()> {
        let points = path.iter().map(|p| p.map(to_fixed)).collect();
        let flight = Flight::new(points, tick, ticks)?;
        if !self.flights.contains_key(&object) && self.flights.len() >= MAX_FLIGHTS {
            anyhow::bail!("{MAX_FLIGHTS} ships are already in the air");
        }
        self.flights.insert(object, flight);
        Ok(())
    }

    /// Whether `object` has a flight in progress.
    pub fn flying(&self, object: u32) -> bool {
        self.flights.contains_key(&object)
    }

    /// Ends the ship's flight where it is, with no arrival.
    pub fn stop(&mut self, object: u32) {
        self.flights.remove(&object);
    }

    pub fn is_empty(&self) -> bool {
        self.flights.is_empty()
    }

    /// Every flight's place on `tick`. A flight that has run its ticks is
    /// stepped to the last point once and ends.
    pub fn step(&mut self, tick: u32) -> Stepped {
        let mut out = Stepped::default();
        for (&object, flight) in &self.flights {
            let pose = flight.at(tick);
            out.steps.push(Step {
                object,
                at: pose.position.map(from_fixed),
                forward: pose.facing.map(|f| f as f32 / ONE as f32),
            });
            if flight.done(tick) {
                out.arrived.push(object);
            }
        }
        for object in &out.arrived {
            self.flights.remove(object);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIP: u32 = 0x8123_0004;

    fn path() -> Vec<[f32; 3]> {
        vec![[0.0, 0.0, 10.0], [100.0, 0.0, 10.0], [100.0, 50.0, 10.0]]
    }

    #[test]
    fn a_ship_flies_its_path_in_the_stated_ticks() {
        let mut f = Flights::default();
        f.begin(SHIP, &path(), 5, 30).unwrap();
        // Before and on the start tick it is at the first point.
        let s = f.step(5);
        assert_eq!(s.steps[0].at, [0.0, 0.0, 10.0]);
        assert!(s.arrived.is_empty());
        // The path is 150 long: 5 a tick, so the corner (100) is tick 25.
        let s = f.step(15);
        assert_eq!(s.steps[0].at, [50.0, 0.0, 10.0]);
        assert_eq!(s.steps[0].forward, [1.0, 0.0, 0.0]);
        let s = f.step(25);
        assert_eq!(s.steps[0].at, [100.0, 0.0, 10.0]);
        // A corner belongs to the second segment.
        assert_eq!(s.steps[0].forward, [0.0, 1.0, 0.0]);
        assert!(s.arrived.is_empty() && f.flying(SHIP));
        // It is at the last point on tick start + 30, reports the arrival
        // once and flies no more.
        let s = f.step(35);
        assert_eq!(s.steps[0].at, [100.0, 50.0, 10.0]);
        assert_eq!(s.arrived, [SHIP]);
        assert!(!f.flying(SHIP));
        assert_eq!(f.step(36), Stepped::default());
    }

    #[test]
    fn a_flight_with_no_path_no_ticks_or_a_fifth_ship_is_refused() {
        let mut f = Flights::default();
        let why = |r: anyhow::Result<()>| r.unwrap_err().to_string();
        assert!(why(f.begin(SHIP, &path()[..1], 0, 10)).contains("at least 2 points"));
        assert!(why(f.begin(SHIP, &path(), 0, 0)).contains("at least 1 tick"));
        assert!(f.is_empty());
        for i in 0..MAX_FLIGHTS as u32 {
            f.begin(SHIP + i, &path(), 0, 10).unwrap();
        }
        assert!(why(f.begin(SHIP + 99, &path(), 0, 10)).contains("in the air"));
        // A ship already flying may be given a new flight.
        f.begin(SHIP, &path(), 3, 10).unwrap();
        assert_eq!(f.step(3).steps.len(), MAX_FLIGHTS);
    }

    #[test]
    fn stop_ends_a_flight_with_no_arrival() {
        let mut f = Flights::default();
        f.begin(SHIP, &path(), 0, 10).unwrap();
        f.stop(SHIP);
        assert_eq!(f.step(10), Stepped::default());
    }
}
