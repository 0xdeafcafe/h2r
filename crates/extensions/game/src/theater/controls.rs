use super::*;
use agent::Access;
use agent_proto::{RpcError, code, theater as api};
use serde::de::DeserializeOwned;

fn params<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
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
        let Some(engine) = &self.engine else {
            if request.seat.is_some() {
                return Err(unavailable("legacy camera requires a player name"));
            }
            self.presentation
                .camera
                .control(request)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
            return Ok(());
        };
        let local = self.header.meta.source == Source::Local;
        if local
            && request.mode != Default
            && !(self.header.version >= 3 && request.mode == FirstPerson)
        {
            return Err(unavailable(
                "local playback provides the recorded retail viewports",
            ));
        }
        let seat = match request.player.as_deref() {
            Some(name) => {
                let seats: BTreeSet<_> = engine
                    .track
                    .descriptor
                    .roster
                    .iter()
                    .filter(|p| p.name == name)
                    .map(|p| p.seat)
                    .chain(
                        engine
                            .track
                            .summary
                            .players
                            .iter()
                            .filter(|p| p.name == name)
                            .map(|p| p.seat),
                    )
                    .collect();
                match request.seat {
                    Some(seat) if seats.contains(&seat) => Some(seat),
                    Some(_) => {
                        return Err(unavailable(
                            "player and seat do not match the recorded roster",
                        ));
                    }
                    None if seats.len() == 1 => seats.first().copied(),
                    None => return Err(unavailable("select a recorded player seat")),
                }
            }
            None => request.seat,
        };
        if local && request.mode == FirstPerson && seat.is_none() {
            return Err(unavailable("select a local player viewport"));
        }
        replay.camera.control(request, seat).map_err(unavailable)?;
        replay.camera.save(&mut self.presentation);
        Ok(())
    }
}

impl Session {
    pub fn add_methods(self: &Arc<Self>, a: &agent::Agent) {
        a.set_lobby_tape(self.clone());
        if !self.replaying() {
            return;
        }
        let unpaced = matches!(&self.lock().mode, Mode::Replay(_, replay) if replay.unpaced);
        for name in api::METHODS {
            if unpaced && name != api::STATE {
                continue;
            }
            let session = self.clone();
            a.add_method(
                name,
                if name == api::STATE {
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
                            s.paused = false;
                        }
                        api::PAUSE | api::STATE => {
                            if !(p.is_null() || p.as_object().is_some_and(|o| o.is_empty())) {
                                return Err(RpcError::new(
                                    code::INVALID_PARAMS,
                                    "expected empty parameters",
                                ));
                            }
                            if name == api::PAUSE {
                                s.paused = true;
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
                            s.speed = p.speed;
                        }
                        api::SEEK => {
                            let p = params::<api::Seek>(p)?;
                            if p.tick > s.header.meta.length_ticks {
                                return Err(RpcError::new(
                                    code::INVALID_PARAMS,
                                    "seek beyond film",
                                ));
                            }
                            if let Some(marks) = p.clip_marks {
                                let [start, end] = marks;
                                if start.is_some_and(|tick| tick > s.header.meta.length_ticks)
                                    || end.is_some_and(|tick| {
                                        tick > s.header.meta.length_ticks.saturating_add(1)
                                            || start.is_none_or(|start| tick <= start)
                                    })
                                {
                                    return Err(RpcError::new(
                                        code::INVALID_PARAMS,
                                        "invalid clip marks",
                                    ));
                                }
                                s.presentation.clip_marks = marks;
                            }
                            s.restart = Some(p.tick);
                        }
                        api::CAMERA => {
                            let p = params::<api::Camera>(p)?;
                            s.camera_request(p)?;
                        }
                        _ => unreachable!(),
                    }
                    let Mode::Replay(_, replay) = &s.mode else {
                        unreachable!()
                    };
                    let seat = replay.camera.requested_seat();
                    Ok(serde_json::json!(api::State {
                        id: s.header.meta.id.clone(),
                        tick: s.tick,
                        length_ticks: s.header.meta.length_ticks,
                        paused: s.paused,
                        speed: s.speed,
                        seeking: s.seeking.is_some() || s.restart.is_some(),
                        seek: s.seek_info(replay),
                        camera_seat: seat,
                        camera: s.presentation.camera.mode,
                        camera_player: s.presentation.camera.player.clone(),
                        kills: s.presentation.kills.iter().copied().collect(),
                    }))
                },
            );
        }
    }
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

pub fn playable_entry(entry: &film::LibraryEntry, stamp: &str) -> bool {
    (entry.version >= 2 || entry.meta.source == Source::Host)
        && image_match(
            entry.version,
            &entry.meta.image_stamp,
            &entry.runtime_stamp,
            entry.guest_image.as_ref(),
            stamp,
            &entry.xbe_sha256,
        )
        .is_ok()
}

pub(super) struct LibraryLaunch {
    dir: PathBuf,
    disc: PathBuf,
    out: PathBuf,
    data: PathBuf,
    image_stamps: Vec<String>,
}

impl LibraryLaunch {
    pub(super) fn entry(&self, id: &str) -> Result<film::LibraryEntry, RpcError> {
        let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
        let entry = if let Some(game) = id.strip_prefix("live:") {
            let path = crate::theater_ui::adapter::archive_path(&self.dir, game)
                .map_err(unavailable)?;
            crate::theater_ui::adapter::archive_entry(&path, game)
        } else {
            film::Library::open(&self.dir).and_then(|lib| lib.get(id))
        }
        .map_err(|e| unavailable(e.to_string()))?;
        if !self
            .image_stamps
            .iter()
            .any(|stamp| playable_entry(&entry, stamp))
        {
            return Err(unavailable(
                "Recording needs a compatible game build and an engine film".into(),
            ));
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
    image_stamps: Vec<String>,
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
        image_stamps,
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
    for name in api::METHODS {
        let player = player.clone();
        let launch = launch.clone();
        let jobs = jobs.clone();
        a.add_method(
            name,
            if name == api::STATE {
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
                client
                    .set_timeout(Some(std::time::Duration::from_secs(2)))
                    .map_err(|e| unavailable(e.to_string()))?;
                client.call(name, p).map_err(|e| match e {
                    agent_proto::CallError::Rpc(e) => e,
                    other => unavailable(other.to_string()),
                })
            },
        );
    }
}
