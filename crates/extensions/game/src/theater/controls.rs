use super::*;
use agent::Access;
use agent_proto::{RpcError, code, theater as api};
use serde::de::DeserializeOwned;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

pub(super) fn params<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(if v.is_null() {
        serde_json::json!({})
    } else {
        v
    })
    .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}
impl State {

    fn camera_request(&mut self, request: api::Camera) -> Result<(), RpcError> {
        use api::CameraMode::*;
        let Mode::Replay(_, replay) = &self.mode else {
            unreachable!()
        };
        let unavailable = |msg| RpcError::new(code::UNAVAILABLE, msg);
        if request.mode != Default && replay.render_every == 0 {
            return Err(unavailable("playback has no rendered view"));
        }
        if request.seat.is_some_and(|s| s >= 16)
            || (request.mode == Default && (request.seat.is_some() || request.player.is_some()))
        {
            return Err(RpcError::new(code::INVALID_PARAMS, "invalid camera target"));
        }
        let roster = self.engine.iter().flat_map(|engine| {
            engine.track.descriptor.roster.iter().map(|p| (p.seat, p.name.as_str()))
                .chain(engine.track.summary.players.iter().map(|p| (p.seat, p.name.as_str())))
        });
        let mut roster_seats = 0u16;
        let mut named_seats = BTreeSet::new();
        for (seat, name) in roster.filter(|(seat, _)| *seat < 16) {
            roster_seats |= 1 << seat;
            if request.player.as_deref() == Some(name) {
                named_seats.insert(seat);
            }
        }
        let seat = if request.player.is_some() && roster_seats != 0 {
            match request.seat {
                Some(seat) if named_seats.contains(&seat) => Some(seat),
                Some(_) => return Err(unavailable("player and seat do not match the recorded roster")),
                None if named_seats.len() == 1 => named_seats.first().copied(),
                None => return Err(unavailable("select a recorded player seat")),
            }
        } else {
            request.seat
        };
        if self.header.meta.source == Source::Host
            && self.engine.is_some()
            && seat.is_some_and(|seat| {
                roster_seats & (1 << seat) == 0
                    && !replay.camera.has_seat(seat)
                    && !super::forge_film::is_monitor(seat)
            })
        {
            return Err(unavailable("that seat is absent from the film roster and current view"));
        }
        replay.camera.control(request, seat)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
        Ok(())
    }
}

impl Session {
    pub fn add_methods(self: &Arc<Self>, a: &agent::Agent) {
        a.set_lobby_tape(self.clone());
        let session = self.clone();
        a.add_method(api::MEASUREMENTS, Access::Read, move |p| {
            let _: Empty = params(p)?;
            let s = session.lock();
            let run = s.measurements.as_ref().ok_or_else(|| {
                RpcError::new(code::UNAVAILABLE, "start with H2_THEATER_MEASURE=1")
            })?;
            Ok(serde_json::json!(run.snapshot()))
        });
        let session = self.clone();
        a.add_method(api::STATUS, Access::Read, move |p| {
            let _: Empty = params(p)?;
            Ok(serde_json::json!(session.status()))
        });
        if !self.replaying() {
            return;
        }
        let unpaced = matches!(&self.lock().mode, Mode::Replay(_, replay) if replay.unpaced);
        if !unpaced {
            self.add_verify_method(a);
            self.add_media_methods(a);
        }
        for name in api::TRANSPORT_METHODS {
            if unpaced && name != api::STATE {
                continue;
            }
            let session = self.clone();
            a.add_method(
                name,
                if matches!(name, api::STATE | api::KEYFRAMES) {
                    Access::Read
                } else {
                    Access::Act
                },
                move |p| {
                    let mut s = session.lock();
                    match name {
                        api::PLAY => {
                            let p = params::<api::Play>(p)?;
                            if p.range.is_some() {
                                return Err(RpcError::new(
                                    code::INVALID_PARAMS,
                                    "range is only for opening a film",
                                ));
                            }
                            if p.id.is_some_and(|id| id != s.header.meta.id) {
                                return Err(RpcError::new(code::GONE, "film is not loaded"));
                            }
                            s.set_transport_pause(false);
                        }
                        api::PAUSE | api::STATE => {
                            let _: Empty = params(p)?;
                            if name == api::PAUSE {
                                s.set_transport_pause(true);
                            }
                        }
                        api::SPEED => {
                            let p = params::<api::Speed>(p)?;
                            if !(0.25..=4.).contains(&p.speed) {
                                return Err(RpcError::new(
                                    code::INVALID_PARAMS,
                                    "speed must be 0.25 to 4",
                                ));
                            }
                            s.set_speed(p.speed);
                        }
                        api::SEEK => {
                            s.seek(params(p)?)?;
                        }
                        api::STEP => {
                            s.step(params(p)?)?;
                        }
                        api::HUD => {
                            let p = params::<api::Hud>(p)?;
                            let Mode::Replay(_, replay) = &s.mode else { unreachable!() };
                            if let Some(visible) = p.game {
                                replay.camera.set_hud(visible);
                                s.presentation.hud = visible;
                            }
                            if let Some(visible) = p.bar {
                                s.presentation.bar = visible;
                            }
                            if p.game.is_some() {
                                s.request_camera_redraw();
                            }
                        }
                        api::CAMERA => {
                            let p = params::<api::Camera>(p)?;
                            s.camera_request(p)?;
                            s.request_camera_redraw();
                        }
                        api::KEYFRAMES => {
                            let _: Empty = params(p)?;
                            return Ok(serde_json::json!(api::Keyframes {
                                film: s
                                    .engine
                                    .as_ref()
                                    .map(|engine| engine.track.keyframe_ticks())
                                    .unwrap_or_default(),
                                sidecar: s.engine.as_ref()
                                    .map(|engine| engine.track.sidecar_ticks())
                                    .unwrap_or_default(),
                                ring: s.ring_status.ticks.clone(),
                                capture_points: s.capture_points.clone(),
                                ring_bytes: s.ring_status.bytes as u64,
                                ring_cap: s.ring_status.cap as u64,
                                sidecar_bytes: s.engine.as_ref()
                                    .map_or(0, |engine| engine.track.sidecar_bytes()),
                            }));
                        }
                        _ => unreachable!(),
                    }
                    let Mode::Replay(_, replay) = &s.mode else {
                        unreachable!()
                    };
                    let camera = replay.camera.state();
                    Ok(serde_json::json!(api::State {
                        id: s.header.meta.id.clone(),
                        tick: s.visible_tick(),
                        length_ticks: s.length(),
                        paused: s.visible_pause(),
                        speed: s.speed,
                        speed_reached: if s.camera_redraw.is_some() && s.visible_pause() { Some(0.0) } else { s.speed_reached() },
                        seeking: s.camera_redraw.is_none() && s.jump_in_progress(),
                        compatibility: api::CompatibilityState::Playable,
                        camera_seat: camera.seat,
                        camera: camera.mode,
                        camera_player: camera.player,
                        camera_pose: camera.pose,
                        camera_fallback: camera.fallback,
                        monitor_looks: monitor_looks(),
                        hud: s.presentation.hud,
                        bar: s.presentation.bar,
                        kills: s.presentation.kills.iter().copied().collect(),
                        seek: if s.camera_redraw.is_none() { s.seek_info() } else { None },
                    }))
                },
            );
        }
    }
}

/// The recorded monitor looks this machine cannot draw, from what playback
/// has read, as `theater.state` reports them (`monitor_look_source`).
fn monitor_looks() -> Vec<api::MonitorLookSource> {
    super::compatibility::monitor_look_sources(&super::forge_film::unseen(
        &crate::forge_glue::can_draw(),
    ))
}

/// Installed modules accepted by exact film playback, in preference order.
pub fn playback_images(xbe: &[u8], out: &std::path::Path) -> Vec<(PathBuf, String)> {
    [crate::tier::clang_dylib(out), crate::tier::fast_dylib(out)]
        .into_iter()
        .filter_map(|path| {
            let stamp = if let Some(slim) = crate::current_slim_stamp(xbe, &path) {
                slim
            } else {
                match crate::tier::module_tier(xbe, &path)? {
                    crate::tier::Tier::Clang => crate::build_stamp(xbe),
                    crate::tier::Tier::Fast => crate::tier::fast_stamp(xbe),
                    crate::tier::Tier::Host => return None,
                }
            };
            Some((path, stamp))
        })
        .collect()
}

fn library_entry(dir: &std::path::Path, id: &str) -> Result<film::LibraryEntry, RpcError> {
    let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
    if let Some(game) = id.strip_prefix("live:") {
        let path = crate::theater_ui::adapter::archive_path(dir, game).map_err(unavailable)?;
        crate::theater_ui::adapter::archive_entry(&path, game)
    } else {
        if !dir.is_dir() {
            return Err(unavailable("unknown film ID".into()));
        }
        film::Library::open(dir).and_then(|lib| lib.get(id))
    }
    .map_err(|e| unavailable(e.to_string()))
}

pub(super) struct LibraryLaunch {
    dir: PathBuf,
    disc: PathBuf,
    out: PathBuf,
    data: PathBuf,
    installation: Installation,
}

impl LibraryLaunch {
    pub(super) fn entry(&self, id: &str) -> Result<film::LibraryEntry, RpcError> {
        let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
        let entry = library_entry(&self.dir, id)?;
        let compatibility = self.installation.compatibility(&entry);
        if compatibility.state == api::CompatibilityState::Unavailable {
            return Err(unavailable(compatibility.reasons.join(" ")));
        }
        Ok(entry)
    }

    pub(super) fn command(
        &self,
        mode: &str,
        id: &str,
        path: &std::path::Path,
        socket: &std::path::Path,
    ) -> Result<std::process::Command, RpcError> {
        let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
        let exe = std::env::current_exe().map_err(|e| unavailable(e.to_string()))?;
        let cli = exe.file_stem().is_some_and(|s| s == "h2");
        let bundled_cli = cfg!(target_os = "macos") && exe.ends_with("Contents/MacOS/Halo2");
        if !cli && !bundled_cli {
            return Err(unavailable(format!(
                "launch host films with h2 theater {mode}"
            )));
        }
        let mut command = std::process::Command::new(exe);
        command
            .args(["theater", mode])
            .arg(path)
            .arg("--data")
            .arg(&self.data)
            .arg("--disc")
            .arg(&self.disc)
            .arg("--out")
            .arg(&self.out)
            .arg("--socket")
            .arg(socket)
            .env("H2_FILM_DIR", &self.dir)
            .env("H2_THEATER_FILM_ID", id)
            .env_remove("H2_THEATER_RANGE")
            .env_remove("H2_THEATER_VIEW")
            .env_remove("H2_THEATER_WORK");
        Ok(command)
    }
}

fn valid_prepare_id(id: &str) -> Result<(), RpcError> {
    if id.is_empty()
        || id
            .strip_prefix("live:")
            .is_some_and(|game| !live_proto::game::valid_id(game))
    {
        return Err(RpcError::new(code::INVALID_PARAMS, "invalid film ID"));
    }
    Ok(())
}

/// In the original game, Watch selects a local or downloaded film and
/// launches its isolated player. Transport requests follow that player.
pub fn library_methods(
    a: &agent::Agent,
    dir: PathBuf,
    disc: PathBuf,
    out: PathBuf,
    data: PathBuf,
    installation: Installation,
) {
    struct PlayerProcess {
        child: std::process::Child,
        socket: PathBuf,
        id: String,
    }
    let player = Arc::new(Mutex::new(None::<PlayerProcess>));
    let launch = Arc::new(LibraryLaunch {
        dir,
        disc,
        out,
        data,
        installation,
    });
    let compat = launch.clone();
    a.add_method(api::COMPAT, Access::Read, move |p| {
        let request = params::<api::Compat>(p)?;
        let entry = library_entry(&compat.dir, &request.id)?;
        Ok(serde_json::json!(compat.installation.compatibility(&entry)))
    });
    let jobs = super::prepare::Jobs::default();
    a.add_method(api::PREPARE, Access::Act, {
        let launch = launch.clone();
        let jobs = jobs.clone();
        move |p| {
            let request = params::<api::Prepare>(p)?;
            valid_prepare_id(&request.id)?;
            Ok(serde_json::json!(jobs.start(request.id, launch.clone())?))
        }
    });
    let active = player.clone();
    a.add_connection_read(api::VERIFY, move |p| {
        let unavailable = |e| RpcError::new(code::UNAVAILABLE, e);
        let socket = active.lock().unwrap_or_else(|e| e.into_inner())
            .as_ref().map(|player| player.socket.clone())
            .ok_or_else(|| unavailable("no film loaded".to_owned()))?;
        let mut client = agent_proto::Client::connect(&socket)
            .map_err(|e| unavailable(format!("film player unavailable: {e}")))?;
        client.call(api::VERIFY, p).map_err(|e| match e {
            agent_proto::CallError::Rpc(e) => e,
            other => unavailable(other.to_string()),
        })
    });
    for name in api::METHODS.into_iter().filter(|&name| !matches!(name, api::MEASUREMENTS | api::VERIFY | api::STATUS)) {
        let player = player.clone();
        let launch = launch.clone();
        let jobs = jobs.clone();
        a.add_method(
            name,
            if matches!(name, api::STATE | api::KEYFRAMES | api::CLIP_STATUS) {
                Access::Read
            } else {
                Access::Act
            },
            move |p| {
                let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
                let p = if name == api::STATE {
                    if let Some(id) = params::<api::StateParams>(p)?.id {
                        valid_prepare_id(&id)?;
                        return Ok(serde_json::json!(jobs.state(&id)));
                    }
                    serde_json::json!({})
                } else {
                    p
                };
                let mut player = player.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(active) = player.as_mut()
                    && active
                        .child
                        .try_wait()
                        .map_err(|e| unavailable(e.to_string()))?
                        .is_some()
                {
                    *player = None;
                }
                if name == api::PLAY {
                    let request = params::<api::Play>(p.clone())?;
                    if let Some(id) = request.id {
                        if player.is_some() {
                            return Err(unavailable("A film is already playing".into()));
                        }
                        let entry = launch.entry(&id)?;
                        if let Some(view) = request.view {
                            if view.seat.is_some_and(|n| n >= 16)
                                || view.tick.is_some_and(|tick| {
                                    let t = u64::from(tick);
                                    t > entry.meta.length_ticks
                                        || request.range.is_some_and(|[a, b]| !(a..b).contains(&t))
                                })
                            {
                                return Err(RpcError::new(
                                    code::INVALID_PARAMS,
                                    "invalid starting tick or seat",
                                ));
                            }
                        }
                        let meta = entry.meta;
                        let path = entry.path;
                        if request.range.is_some_and(|[start, end]| {
                            start >= end || end > meta.length_ticks.saturating_add(1)
                        }) {
                            return Err(RpcError::new(code::INVALID_PARAMS, "invalid clip range"));
                        }
                        let socket = std::env::temp_dir()
                            .join(format!("h2t-{}.sock", live_proto::sign::random_id()));
                        let mut command = launch.command("play", &id, &path, &socket)?;
                        if let Some(view) = request.view {
                            command.env(
                                "H2_THEATER_VIEW",
                                serde_json::to_string(&view)
                                    .map_err(|e| unavailable(e.to_string()))?,
                            );
                        }
                        if let Some([start, end]) = request.range {
                            command.env("H2_THEATER_RANGE", format!("{start}:{end}"));
                        }
                        let child = command
                            .spawn()
                            .map_err(|e| unavailable(e.to_string()))?;
                        *player = Some(PlayerProcess {
                            child,
                            socket,
                            id: id.clone(),
                        });
                        return Ok(serde_json::json!({"id":id,"starting":true}));
                    }
                }
                let Some(active) = player.as_ref() else {
                    return if name == api::STATE {
                        Ok(serde_json::json!({"id":null,"active":false}))
                    } else {
                        Err(unavailable("no film loaded".into()))
                    };
                };
                let mut client = agent_proto::Client::connect(&active.socket)
                    .map_err(|_| unavailable(format!("film {} is starting", active.id)))?;
                let timeout = if name == api::SCREENSHOT { 61 } else { 2 };
                client
                    .set_timeout(Some(std::time::Duration::from_secs(timeout)))
                    .map_err(|e| unavailable(e.to_string()))?;
                client.call(name, p).map_err(|e| match e {
                    agent_proto::CallError::Rpc(e) => e,
                    other => unavailable(other.to_string()),
                })
            },
        );
    }
}
