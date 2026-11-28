//! Controller routes use bot movement on the game thread, with the resulting
//! reports delivered by the same XAPI tap as input.pad.

mod legs;
mod player;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent::{Access, RoutePad, RoutePorts};
use agent_proto::route::{Leg, LegStatus, PortParams, RouteParams, State, Status};
use agent_proto::{Boundary, PORTS, Phase, RpcError, code, method};
use bots::cache::{self, MapData, Stamp};
use guest_layout::GuestMemory;
use input::host::ConvertConfig;
use serde_json::Value;

use self::legs::{LegRun, Step};
use self::player::{Biped, Region};

const COUNT: usize = PORTS as usize;

pub(super) fn install(
    registry: &mut hooks::Registry,
    agent: &agent::Agent,
    ports: RoutePorts,
    data_root: &Path,
) -> anyhow::Result<()> {
    let feature = Routes::default();
    if !registry
        .add(&feature)
        .map_err(|e| anyhow::anyhow!("agent routes: {e}"))?
    {
        return Ok(());
    }
    let routes = feature.0;
    let dir = cache::dir(data_root);
    let shared = routes.clone();
    agent.add_game_method(method::INPUT_ROUTE, Access::Act, move |v, ctx| {
        let params: RouteParams = parse(v)?;
        let index = port(params.player)?;
        if params.legs.is_empty() || params.timeout_ticks == 0 {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "legs and timeout_ticks must be nonzero",
            ));
        }
        for (i, leg) in params.legs.iter().enumerate() {
            leg.check()
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("leg {i}: {e}")))?;
        }
        if ctx.at.phase != Phase::Gameplay || ctx.at.boundary != Boundary::Tick {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "routes require active gameplay",
            ));
        }
        let (region, stamp, biped, tick) = ctx
            .read_guest(|m| {
                Some((
                    Region::read(m)?,
                    player::map_stamp(m)?,
                    Biped::read(m, params.player),
                    bots::guest::game_tick(m)?,
                ))
            })?
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no active map or BSP"))?;

        let mut routes = shared.lock().unwrap_or_else(|e| e.into_inner());
        let nav = routes.mesh(&dir, &region, stamp)?;
        let biped = biped.ok_or_else(|| {
            RpcError::new(
                code::UNAVAILABLE,
                "controller port has no living player on foot",
            )
        })?;
        let pad = ports.acquire(params.player)?;
        let mut track = Track {
            run: Run::new(params),
            drive: Some(Drive {
                nav,
                region,
                pad,
                unit: biped.unit,
                tick,
                motion: bots::mind::RoutePad::default(),
            }),
        };
        track.advance(biped, tick);
        routes.tracks[index] = Some(track);
        status(&mut routes.tracks[index], index)
    });

    let shared = routes.clone();
    agent.add_method(method::INPUT_ROUTE_STATUS, Access::Read, move |v| {
        let index = port(parse::<PortParams>(v)?.player)?;
        status(
            &mut shared.lock().unwrap_or_else(|e| e.into_inner()).tracks[index],
            index,
        )
    });
    agent.add_method(method::INPUT_ROUTE_STOP, Access::Act, move |v| {
        let index = port(parse::<PortParams>(v)?.player)?;
        let mut routes = routes.lock().unwrap_or_else(|e| e.into_inner());
        let track = &mut routes.tracks[index];
        if let Some(track) = track {
            track.check_pad();
            track.stop(State::Stopped, "stopped");
        }
        status(track, index)
    });
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() {
        serde_json::json!({})
    } else {
        v
    };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn port(player: u32) -> Result<usize, RpcError> {
    if player < PORTS {
        Ok(player as usize)
    } else {
        Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("player must be 0 to {}", PORTS - 1),
        ))
    }
}

fn status(track: &mut Option<Track>, index: usize) -> Result<Value, RpcError> {
    let idle = Status {
        player: index as u32,
        state: State::Idle,
        ticks: 0,
        leg: None,
        legs: vec![],
    };
    let view = match track {
        Some(track) => {
            track.check_pad();
            &track.run.status
        }
        None => &idle,
    };
    Ok(serde_json::json!(view))
}

#[derive(Default)]
struct Routes(Arc<Mutex<Controllers>>);

#[derive(Default)]
struct Controllers {
    tracks: [Option<Track>; COUNT],
    mesh: Option<(Region, Stamp, Arc<MapData>)>,
}

impl Controllers {
    fn mesh(
        &mut self,
        dir: &Path,
        region: &Region,
        stamp: Stamp,
    ) -> Result<Arc<MapData>, RpcError> {
        if let Some((key, cached_stamp, nav)) = &self.mesh
            && key == region
            && *cached_stamp == stamp
        {
            return Ok(nav.clone());
        }
        // The same stamped BSP cache the bots use, even in games with no bots.
        let name = region.name();
        let path = dir.join(cache::file_name(&name, usize::from(region.bsp)));
        let mesh = load_mesh(path, &name, region.bsp, stamp)?;
        self.mesh = Some((region.clone(), stamp, mesh.clone()));
        Ok(mesh)
    }
}

fn load_mesh(path: PathBuf, map: &str, bsp: u16, stamp: Stamp) -> Result<Arc<MapData>, RpcError> {
    let unavailable = || {
        RpcError::new(
            code::UNAVAILABLE,
            format!("no matching nav mesh for {map} BSP {bsp}; run `h2 bots nav {map}`"),
        )
    };
    let bytes = std::fs::read(path).map_err(|_| unavailable())?;
    if cache::read_header(&bytes).and_then(|(_, stamp)| stamp) != Some(stamp) {
        return Err(unavailable());
    }
    let (nav, _) = MapData::from_bytes(&bytes).ok_or_else(unavailable)?;
    if nav.nav.polys.is_empty() {
        return Err(unavailable());
    }
    Ok(Arc::new(nav))
}

impl hooks::Feature for Routes {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("agent-routes", "Agent API controller routes")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Routes are live controller commands. Agent snapshot restores
        // release their pads, just like a checkpoint restore.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let state = self.0.clone();
        b.frame().on_tick("advance controller routes", move |o| {
            let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
            if !state
                .tracks
                .iter()
                .flatten()
                .any(|track| track.drive.is_some())
            {
                return;
            }
            let g = o.guest();
            let region = Region::read(&*g);
            let tick = bots::guest::game_tick(&*g);
            for track in state.tracks.iter_mut().flatten() {
                track.tick(&*g, region.as_ref(), tick);
            }
        });
        let state = self.0.clone();
        b.game()
            .on_start("release controller routes on game start", move |_| {
                let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                for track in state.tracks.iter_mut().flatten() {
                    track.stop(State::Stopped, "map_changed");
                }
            });
        Ok(())
    }
}

struct Track {
    run: Run,
    drive: Option<Drive>,
}

struct Drive {
    nav: Arc<MapData>,
    region: Region,
    unit: u32,
    tick: u32,
    pad: RoutePad,
    motion: bots::mind::RoutePad,
}

impl Track {
    fn stop(&mut self, state: State, reason: &'static str) {
        self.run.stop(state, reason);
        self.drive = None;
    }

    fn check_pad(&mut self) {
        if let Some(reason) = self
            .drive
            .as_ref()
            .and_then(|drive| drive.pad.interruption())
        {
            self.stop(State::Stopped, reason);
        }
    }

    fn tick(&mut self, m: &dyn GuestMemory, region: Option<&Region>, tick: Option<u32>) {
        self.check_pad();
        let Some(drive) = &mut self.drive else { return };
        let Some(tick) = tick.filter(|&tick| tick >= drive.tick && region == Some(&drive.region))
        else {
            self.stop(State::Stopped, "map_changed");
            return;
        };
        if tick == drive.tick {
            return;
        }
        let Some(biped) = Biped::read(m, self.run.status.player).filter(|b| b.unit == drive.unit)
        else {
            self.stop(State::Failed, "player_unavailable");
            return;
        };
        self.advance(biped, tick);
    }

    fn advance(&mut self, biped: Biped, tick: u32) {
        let Some(drive) = &mut self.drive else { return };
        drive.tick = tick;
        if let Some(motion) = self.run.tick(&drive.nav, biped, tick) {
            let pad = drive.motion.report(biped.aim, tick, motion);
            drive
                .pad
                .set(pad.to_gamepad(&ConvertConfig::default(), false));
        } else {
            self.drive = None;
        }
    }
}

struct Run {
    legs: Vec<Leg>,
    timeout: u32,
    status: Status,
    leg: LegRun,
    pending: bool,
}

impl Run {
    fn new(params: RouteParams) -> Self {
        let first = params.legs[0];
        Self {
            status: Status {
                player: params.player,
                state: State::Running,
                ticks: 0,
                leg: Some(0),
                legs: vec![LegStatus::default(); params.legs.len()],
            },
            legs: params.legs,
            timeout: params.timeout_ticks,
            leg: LegRun::new(first),
            pending: false,
        }
    }

    fn stop(&mut self, state: State, reason: &'static str) {
        let Some(index) = self.status.leg.take() else {
            return;
        };
        self.status.state = state;
        self.status.legs[index].ok = Some(false);
        self.status.legs[index].reason = Some(reason);
        for leg in &mut self.status.legs[index + 1..] {
            leg.ok = Some(false);
            leg.reason = Some("not_run");
        }
    }

    fn tick(&mut self, nav: &MapData, biped: Biped, tick: u32) -> Option<bots::mind::RouteMotion> {
        if self.pending {
            let index = self.status.leg?;
            self.status.ticks += 1;
            self.status.legs[index].ticks += 1;
            self.pending = false;
        }
        while let Some(index) = self.status.leg {
            match self
                .leg
                .step(nav, biped, tick, &mut self.status.legs[index])
            {
                Err(reason) => self.stop(State::Failed, reason),
                Ok(Step::Done) => {
                    self.status.legs[index].ok = Some(true);
                    if let Some(&next) = self.legs.get(index + 1) {
                        self.status.leg = Some(index + 1);
                        self.leg = LegRun::new(next);
                    } else {
                        self.status.leg = None;
                        self.status.state = State::Completed;
                    }
                }
                Ok(Step::Pad(_)) if self.status.ticks == self.timeout => {
                    self.stop(State::Failed, "timeout");
                }
                Ok(Step::Pad(motion)) => {
                    self.pending = true;
                    return Some(motion);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
