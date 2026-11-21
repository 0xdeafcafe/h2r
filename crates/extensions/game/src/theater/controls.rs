use super::*;
use agent::Access;
use agent_proto::{RpcError, code, theater as api};
use serde::de::DeserializeOwned;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

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
        let session = self.clone();
        a.add_method(api::MEASUREMENTS, Access::Read, move |p| {
            let _: Empty = params(p)?;
            let s = session.lock();
            let run = s.measurements.as_ref().ok_or_else(|| {
                RpcError::new(code::UNAVAILABLE, "start with H2_THEATER_MEASURE=1")
            })?;
            Ok(serde_json::json!(run.snapshot()))
        });
        if !self.replaying() {
            return;
        }
        for name in api::METHODS.into_iter().filter(|&name| name != api::MEASUREMENTS) {
            let session = self.clone();
            a.add_method(
                name,
                if name == api::STATE || name == api::KEYFRAMES {
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
                            let _: Empty = params(p)?;
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
                            let source = s
                                .engine
                                .as_ref()
                                .map_or(api::SeekSource::Boot, |engine| {
                                    engine.seek_source(p.tick)
                                });
                            s.seek = Some(SeekProgress {
                                target: p.tick,
                                from_tick: s.tick,
                                source,
                                warm: false,
                                restore_ms: None,
                            });
                            s.restart = Some(p.tick);
                        }
                        api::CAMERA => {
                            let p = params::<api::Camera>(p)?;
                            s.camera_request(p)?;
                        }
                        api::KEYFRAMES => {
                            let _: Empty = params(p)?;
                            return Ok(serde_json::json!(api::Keyframes {
                                film: s
                                    .engine
                                    .as_ref()
                                    .map(|engine| engine.track.keyframe_ticks())
                                    .unwrap_or_default(),
                                sidecar: Vec::new(),
                                ring: Vec::new(),
                                capture_points: s.capture_points.clone(),
                                ring_bytes: 0,
                                ring_cap: 0,
                                sidecar_bytes: 0,
                            }));
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
                        camera_seat: seat,
                        camera: s.presentation.camera.mode,
                        camera_player: s.presentation.camera.player.clone(),
                        kills: s.presentation.kills.iter().copied().collect(),
                        seek: s.seek.as_ref().map(|seek| api::SeekInfo {
                            target: seek.target,
                            from_tick: seek.from_tick,
                            source: seek.source,
                            warm: seek.warm,
                            remaining_ticks: seek.target.saturating_sub(s.tick),
                            restore_ms: seek.restore_ms,
                        }),
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
    let image_stamps = Arc::new(image_stamps);
    for name in api::METHODS.into_iter().filter(|&name| name != api::MEASUREMENTS) {
        let player = player.clone();
        let image_stamps = image_stamps.clone();
        let (dir, disc, out, data) = (dir.clone(), disc.clone(), out.clone(), data.clone());
        a.add_method(
            name,
            if name == api::STATE || name == api::KEYFRAMES {
                Access::Read
            } else {
                Access::Act
            },
            move |p| {
                let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
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
                        let entry = if let Some(game) = id.strip_prefix("live:") {
                            let path = crate::theater_ui::adapter::archive_path(&dir, game)
                                .map_err(unavailable)?;
                            crate::theater_ui::adapter::archive_entry(&path, game)
                        } else {
                            film::Library::open(&dir).and_then(|lib| lib.get(&id))
                        }
                        .map_err(|e| unavailable(e.to_string()))?;
                        if !image_stamps
                            .iter()
                            .any(|stamp| playable_entry(&entry, stamp))
                        {
                            return Err(unavailable(
                                "Recording needs a compatible game build and an engine film".into(),
                            ));
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
                        let exe =
                            std::env::current_exe().map_err(|e| unavailable(e.to_string()))?;
                        if exe.file_stem().is_none_or(|s| s != "h2") {
                            return Err(unavailable(
                                "launch host films with h2 theater play".into(),
                            ));
                        }
                        let mut command = std::process::Command::new(exe);
                        command
                            .env("H2_FILM_DIR", &dir)
                            .env("H2_THEATER_FILM_ID", &id)
                            .env_remove("H2_THEATER_RANGE");
                        if let Some([start, end]) = request.range {
                            command.env("H2_THEATER_RANGE", format!("{start}:{end}"));
                        }
                        let child = command
                            .args(["theater", "play"])
                            .arg(path)
                            .arg("--data")
                            .arg(&data)
                            .arg("--disc")
                            .arg(&disc)
                            .arg("--out")
                            .arg(&out)
                            .arg("--socket")
                            .arg(&socket)
                            .env_remove("H2_THEATER_WORK")
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
