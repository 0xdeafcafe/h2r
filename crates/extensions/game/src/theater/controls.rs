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
        let roster = self.play.iter().flat_map(|play| {
            play.start().game.descriptor.roster.iter().map(|p| (p.seat, p.name.as_str()))
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
            && self.play.is_some()
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
        if !self.replaying {
            return;
        }
        self.add_verify_method(a);
        self.add_media_methods(a);
        for name in api::TRANSPORT_METHODS {
            let session = self.clone();
            a.add_method(
                name,
                if name == api::STATE { Access::Read } else { Access::Act },
                move |p| session.transport(name, p),
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
        self.playable(library_entry(&self.dir, id)?)
    }

    /// The film at `path`, if this build plays it.
    fn entry_at(&self, path: &std::path::Path) -> Result<film::LibraryEntry, RpcError> {
        let entry = film::LibraryEntry::read(path)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("{}: {e}", path.display())))?;
        self.playable(entry)
    }

    fn playable(&self, entry: film::LibraryEntry) -> Result<film::LibraryEntry, RpcError> {
        let compatibility = self.installation.compatibility(&entry);
        if compatibility.state == api::CompatibilityState::Unavailable {
            return Err(RpcError::new(code::UNAVAILABLE, compatibility.reasons.join(" ")));
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

/// `theater.watch`: check the film, queue its options as the next game and
/// arm it once the game's calls have queued them
/// (docs/game/theater-design.md, "Starting a film in the running game").
fn watch_film(
    film: &Arc<Session>,
    launch: &LibraryLaunch,
    p: Value,
    ctx: &mut agent::GameCtx<'_>,
) -> Result<Value, RpcError> {
    use agent::sim::calls::{self, Call};
    let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
    let invalid = |e: &str| RpcError::new(code::INVALID_PARAMS, e);
    let request: api::Watch = params(p)?;
    request.check().map_err(invalid)?;
    if request.seat.is_some_and(|n| n >= 16) {
        return Err(invalid("invalid seat"));
    }
    // A co-op or networked session runs one game for every machine.
    let mut sim_mode = None;
    if !ctx
        .reader
        .with_guest(&mut |g| sim_mode = crate::cheats::sim_mode(&*g))
    {
        return Err(unavailable("guest memory unavailable".into()));
    }
    if let Some(why) = crate::game_load::session_refusal(sim_mode) {
        return Err(unavailable(why.into()));
    }
    let entry = match (&request.id, &request.path) {
        (Some(id), _) => {
            valid_film_id(id)?;
            launch.entry(id)?
        }
        (_, Some(path)) => launch.entry_at(std::path::Path::new(path))?,
        _ => return Err(invalid("name the film by id or by path")),
    };
    if let Some(tick) = request.tick
        && u64::from(tick) > entry.meta.length_ticks
    {
        return Err(invalid("invalid starting tick"));
    }
    let planned = film
        .plan_watch(&entry.path, request.view())
        .map_err(|e| unavailable(format!("{e:#}")))?;
    let queue = film.calls().ok_or_else(|| {
        unavailable(
            "the game's own functions aren't reachable here: the agent-calls feature isn't \
             installed"
                .into(),
        )
    })?;
    let ticket = calls::submit(&queue, Call::QueueGame { steps: planned.steps() })
        .map_err(unavailable)?;
    let (id, length) = (planned.id().to_owned(), planned.length_ticks());
    eprintln!("theater.watch: queueing film {id}");
    let film = film.clone();
    let mut planned = Some(planned);
    ctx.defer(
        30,
        Box::new(move |_: &mut agent::GameCtx<'_>| {
            let out = ticket.outcome()?;
            Some(out.map(|v| {
                let queued = v["queued"].as_bool().unwrap_or(false);
                if let Some(planned) = planned.take().filter(|_| queued) {
                    film.arm_watch(planned);
                }
                serde_json::json!(api::Watching { id: id.clone(), length_ticks: length, queued })
            }))
        }),
    );
    Ok(Value::Null)
}

fn valid_film_id(id: &str) -> Result<(), RpcError> {
    if id.is_empty()
        || id
            .strip_prefix("live:")
            .is_some_and(|game| !live_proto::game::valid_id(game))
    {
        return Err(RpcError::new(code::INVALID_PARAMS, "invalid film ID"));
    }
    Ok(())
}

/// A player's game that records its games watches a film in itself
/// (`theater.watch`, [`Session::plan_watch`]), and answers the transport
/// there. Any other game launches a film player process for a film
/// (`theater.play`), and transport requests follow that process.
/// `watch` is the game's own session, if it can watch.
pub fn library_methods(
    a: &agent::Agent,
    dir: PathBuf,
    disc: PathBuf,
    out: PathBuf,
    data: PathBuf,
    installation: Installation,
    watch: Option<Arc<Session>>,
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
    let library = launch.clone();
    a.add_method(api::LIST, Access::Read, move |p| {
        let _: Empty = params(p)?;
        let entries = film::Library::open(&library.dir)
            .and_then(|library| library.list())
            .map_err(|error| RpcError::new(code::UNAVAILABLE, error.to_string()))?;
        Ok(Value::Array(entries.into_iter().map(|entry| serde_json::json!({
            "id": entry.meta.id,
            "map": entry.meta.map,
            "variant": entry.meta.variant,
            "started_ms": entry.meta.started_ms,
            "length_ticks": entry.meta.length_ticks,
            "saved": entry.saved,
        })).collect()))
    });
    if let Some(film) = watch.clone().filter(|film| film.can_watch()) {
        let launch = launch.clone();
        let watching = film.clone();
        a.add_game_method(api::WATCH, Access::Act, move |p, ctx| {
            watch_film(&watching, &launch, p, ctx)
        });
        let leaving = film;
        a.add_method(api::END, Access::Act, move |p| {
            let _: Empty = params(p)?;
            leaving.end_film()?;
            Ok(serde_json::json!({"leaving": true}))
        });
    }
    let compat = launch.clone();
    a.add_method(api::COMPAT, Access::Read, move |p| {
        let request = params::<api::Compat>(p)?;
        let entry = library_entry(&compat.dir, &request.id)?;
        Ok(serde_json::json!(compat.installation.compatibility(&entry)))
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
    for name in api::METHODS.into_iter().chain([api::EXIT]).filter(|&name| !matches!(name, api::MEASUREMENTS | api::VERIFY | api::STATUS)) {
        let player = player.clone();
        let launch = launch.clone();
        let watch = watch.clone();
        a.add_method(
            name,
            if matches!(name, api::STATE | api::CLIP_STATUS) {
                Access::Read
            } else {
                Access::Act
            },
            move |p| {
                let unavailable = |e: String| RpcError::new(code::UNAVAILABLE, e);
                // A film playing in this game answers its own transport.
                if let Some(film) = watch.as_ref().filter(|film| film.watching()) {
                    if name == api::EXIT {
                        film.end_film()?;
                        return Ok(serde_json::json!({"leaving": true}));
                    }
                    if api::TRANSPORT_METHODS.contains(&name) {
                        return film.transport(name, p);
                    }
                }
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
                let method = if name == api::EXIT {
                    agent_proto::host_quit::method::QUIT
                } else {
                    name
                };
                client.call(method, p).map_err(|e| match e {
                    agent_proto::CallError::Rpc(e) => e,
                    other => unavailable(other.to_string()),
                })
            },
        );
    }
}

impl Session {
    /// A transport request (`theater.play`, `.pause`, `.state`, ...) of the
    /// film playing in this session: a film player process answers it from
    /// its own session; a game that watches a film answers it from the
    /// state that holds the film ([`watch`]).
    pub(super) fn transport(&self, name: &str, p: Value) -> Result<Value, RpcError> {
        let mut s = self.lock();
        if !matches!(s.mode, Mode::Replay(..)) {
            return Err(RpcError::new(code::UNAVAILABLE, "no film is playing"));
        }
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
                s.set_paused(false);
            }
            api::PAUSE | api::STATE => {
                let _: Empty = params(p)?;
                if name == api::PAUSE {
                    s.set_paused(true);
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
                if let Some(target) = s.restart {
                    // A start-route seek queues the film's options again;
                    // the game loads the map and plays on to the target.
                    drop(s);
                    self.requeue(target)?;
                    s = self.lock();
                }
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
        let camera = replay.camera.state();
        Ok(serde_json::json!(api::State {
            id: s.header.meta.id.clone(),
            tick: s.tick,
            length_ticks: s.length(),
            paused: s.paused,
            speed: s.speed,
            speed_reached: s.speed_reached(),
            seeking: s.jump_in_progress(),
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
            seek: s.seek_info(),
        }))
    }
}
