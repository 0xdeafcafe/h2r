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
impl Session {
    pub(crate) fn add_methods(self: &Arc<Self>, a: &agent::Agent) {
        a.set_lobby_tape(self.clone());
        if !self.replaying() {
            return;
        }
        for name in api::METHODS {
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
                                        tick > s.header.meta.length_ticks
                                            || start.is_none_or(|start| tick <= start)
                                    })
                                {
                                    return Err(RpcError::new(code::INVALID_PARAMS, "invalid clip marks"));
                                }
                                s.presentation.clip_marks = marks;
                            }
                            s.restart = Some(p.tick);
                        }
                        api::CAMERA => {
                            let p = params::<api::Camera>(p)?;
                            if p.mode == api::CameraMode::FirstPerson
                                && s.header.meta.source != Source::Host
                            {
                                return Err(RpcError::new(
                                    code::UNAVAILABLE,
                                    "first person requires an exact host film",
                                ));
                            }
                            if p.mode != api::CameraMode::Default
                                && matches!(&s.mode, Mode::Replay(_, opts) if opts.render_every == 0)
                            {
                                return Err(RpcError::new(
                                    code::UNAVAILABLE,
                                    "playback has no rendered view",
                                ));
                            }
                            s.presentation.camera.control(p)
                                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?;
                        }
                        _ => unreachable!(),
                    }
                    Ok(serde_json::json!(api::State {
                        id: s.header.meta.id.clone(),
                        tick: s.tick,
                        length_ticks: s.header.meta.length_ticks,
                        paused: s.paused,
                        speed: s.speed,
                        seeking: s.seeking.is_some() || s.restart.is_some(),
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

/// In the original game, Watch selects a local or downloaded film and
/// launches its isolated player. Transport requests follow that player.
pub(crate) fn library_methods(
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
    for name in api::METHODS {
        let player = player.clone();
        let image_stamps = image_stamps.clone();
        let (dir, disc, out, data) = (dir.clone(), disc.clone(), out.clone(), data.clone());
        a.add_method(
            name,
            if name == api::STATE {
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
                        let (path, meta) = if let Some(game) = id.strip_prefix("live:") {
                            let path = crate::theater_ui::adapter::archive_path(&dir, game)
                                .map_err(unavailable)?;
                            let header = film::Reader::open(&path)
                                .map_err(|e| unavailable(e.to_string()))?
                                .header;
                            (path, header.meta)
                        } else {
                            let entry = film::Library::open(&dir)
                                .and_then(|lib| lib.get(&id))
                                .map_err(|e| unavailable(e.to_string()))?;
                            (entry.path, entry.meta)
                        };
                        if meta.source != film::Source::Host {
                            return Err(unavailable(
                                "Exact playback needs a host recording".into(),
                            ));
                        }
                        if !image_stamps.iter().any(|stamp| meta.playable_here(stamp)) {
                            return Err(unavailable(
                                "Recording needs a different game build".into(),
                            ));
                        }
                        if request
                            .range
                            .is_some_and(|[start, end]| start >= end || end > meta.length_ticks)
                        {
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
