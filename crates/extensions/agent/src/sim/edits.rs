//! Host writes for the paired editor. Read options at dispatch, and resolve
//! objects and scenario bytes inside the guest-call hook where they are used.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use super::calls::{Arg, GameStep, RespawnTarget};
use super::*;
use agent_proto::sim::REFUSED;

/// Check the active DVD slot and start a worker to open and validate the play copy.
pub type MapRebind = Box<
    dyn Fn(&str, &str, &Path, Arc<AtomicBool>) -> Result<Receiver<Result<PreparedMapReload, String>>, String>
        + Send,
>;

/// The deferred RPC owns the cancellation signal for the worker and the VFS.
#[derive(Default)]
struct PreparationGuard(Arc<AtomicBool>);

impl Drop for PreparationGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// A validated file and the host operation that retires its old cache entry.
pub struct PreparedMapReload {
    pub(super) checksum: u32,
    reserve: Box<dyn FnOnce() -> Result<ReservedMapReload, String> + Send>,
}

impl std::fmt::Debug for PreparedMapReload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedMapReload")
            .field("checksum", &self.checksum)
            .finish_non_exhaustive()
    }
}

impl PreparedMapReload {
    pub fn new(
        checksum: u32,
        reserve: impl FnOnce() -> Result<ReservedMapReload, String> + Send + 'static,
    ) -> Self {
        Self {
            checksum,
            reserve: Box::new(reserve),
        }
    }

    pub(super) fn reserve(self) -> Result<ReservedMapReload, String> {
        (self.reserve)()
    }
}

/// The DVD source is reserved and the old resident paths have been retired.
/// Publishing cannot fail. Cancellation restores the paths before releasing
/// the reservation, and can be retried if guest memory is unavailable.
pub struct ReservedMapReload {
    pub(super) publish: Box<dyn FnOnce() + Send>,
    pub(super) restore: Box<dyn FnMut() -> Result<(), String> + Send>,
    pub(super) retirement_error: Option<String>,
}

impl ReservedMapReload {
    pub fn new(
        publish: impl FnOnce() + Send + 'static,
        restore: impl FnMut() -> Result<(), String> + Send + 'static,
    ) -> Self {
        Self {
            publish: Box::new(publish),
            restore: Box::new(restore),
            retirement_error: None,
        }
    }

    /// A partial retirement still returns its journal so cancellation can
    /// repair the keys that were written.
    pub fn with_retirement(mut self, result: Result<(), String>) -> Self {
        self.retirement_error = result.err();
        self
    }
}

impl SimHandle {
    /// An authorized editor write adopts the game on the game thread. Passive
    /// discovery and editor presentation leave the boot dispatch alone.
    pub fn on_first_edit(
        &self,
        agent: &Agent,
        install: impl Fn(&Agent) -> Result<(), String> + Send + 'static,
    ) {
        let agent = agent.weak_handle();
        let Some(queue) = self.calls() else { return };
        self.lock().adopt_editor = Some(Box::new(move || {
            let agent = agent().ok_or("the game agent has shut down")?;
            install(&agent)?;
            agent.editor_adopted(queue.clone());
            Ok(())
        }));
    }

    /// Where this host can bind an editor play copy on the next map load.
    pub fn set_map_rebind(&self, prepare: MapRebind) {
        self.lock().map_rebind = Some(prepare);
    }

    /// A film or input replay can explain why this host cannot reload.
    pub fn install_reload_refusal(&self, agent: &Agent, reason: &'static str) {
        if self.calls().is_none() {
            return;
        }
        agent.add_game_method(method::MAP_RELOAD, Access::Act, move |_, _| {
            Err(refused(reason))
        });
    }

    /// The host installs these only with both tick features and outside playback.
    pub fn install_world_edits(&self, agent: &Agent) {
        for &name in method::EDITS {
            let sim = self.clone();
            let input = agent.edit_input();
            agent.add_game_method(name, Access::Act, move |v, ctx| {
                sim.lock().edit(name, v, ctx, &input)
            });
        }
    }
}

pub(super) fn refused(why: impl Into<String>) -> RpcError {
    let why = why.into();
    let mut err = RpcError::new(REFUSED, why.clone());
    err.data = Some(json!({ "why": why }));
    err
}

fn object_id(s: &str) -> Result<ObjectId, RpcError> {
    ObjectId::parse(s)
        .filter(|id| id.kind != Kind::Player)
        .ok_or_else(|| invalid("id must be <kind>.<8 hex digits> from world.object"))
}

pub(super) fn valid_axes(forward: [f32; 3], up: [f32; 3]) -> bool {
    let dot = |a: [f32; 3], b: [f32; 3]| a.into_iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
    (dot(forward, forward) - 1.0).abs() < 0.001
        && (dot(up, up) - 1.0).abs() < 0.001
        && dot(forward, up).abs() < 0.001
}

/// The loaded scenario and the simulation and squad that own its objects.
/// The cache name/checksum are the same fields the public map stamp reads.
#[derive(PartialEq)]
struct EditIdentity {
    name: [u8; 32],
    checksum: u32,
    scenario: u32,
    simulation: u32,
    squad: u32,
}

impl EditIdentity {
    fn read(m: &dyn GuestMemory) -> Result<Self, RpcError> {
        use guest_layout::{game::cache_checksum, global};
        let fault = |e: engine::MemFault| {
            refused(format!("cannot identify the active map and session at {:#x}", e.addr))
        };
        let mut name = [0; 32];
        m.read(global::CACHE_MAP_NAME.base, &mut name).map_err(fault)?;
        let scenario = global::SCENARIO.pointer(m).map_err(fault)?
            .ok_or_else(|| refused("no loaded scenario"))?;
        Ok(Self {
            name,
            checksum: cache_checksum(m).map_err(fault)?,
            scenario,
            simulation: m.u32(global::SIM_GLOBALS.base).map_err(fault)?,
            squad: m.u32(global::SQUAD_SESSION.base).map_err(fault)?,
        })
    }
}

impl Sim {
    fn adopt_editor(&mut self) -> Result<(), RpcError> {
        if let Some(install) = &self.adopt_editor {
            install().map_err(refused)?;
            self.adopt_editor = None;
        }
        Ok(())
    }

    fn edit_check(
        &self,
        session: SessionView,
        params: &Value,
        ctx: &mut GameCtx<'_>,
    ) -> Result<calls::EditCheck, RpcError> {
        crate::hot::permit(session.clone(), params, ctx.epoch)?;
        let identity = ctx.with_guest(|m| EditIdentity::read(m))?;
        let name_len = identity.name.iter().position(|&b| b == 0).unwrap_or(identity.name.len());
        if !ctx.at.map.as_ref().is_some_and(|name| {
            name.as_bytes().eq_ignore_ascii_case(&identity.name[..name_len])
        }) {
            return Err(refused("the active map changed before the edit"));
        }
        let lifetime = ctx.edit_lifetime();
        let epoch = ctx.epoch;
        let live = ctx.live_session;
        let probe = Arc::clone(&self.probe);
        let params = params.clone();
        Ok(Box::new(move |m| {
            let current_epoch = lifetime.load(std::sync::atomic::Ordering::Acquire);
            crate::hot::check_epoch(&params, current_epoch)?;
            if current_epoch != epoch || EditIdentity::read(m)? != identity {
                return Err(refused("the active map or session changed before the edit"));
            }
            if live.is_some_and(|probe| probe(m)) {
                return Err(refused("external Live session"));
            }
            let current = read_session(&probe, m);
            if current.kind != session.kind
                || current.host != session.host
                || current.protocol != session.protocol
                || current.peers != session.peers
            {
                return Err(refused("the session or its peer count changed before the edit"));
            }
            crate::hot::permit(current, &params, current_epoch)
        }))
    }

    fn edit(&mut self, name: &str, v: Value, ctx: &mut GameCtx<'_>, input: &Mutex<crate::Input>) -> R {
        if let Some(probe) = ctx.live_session
            && ctx.read_guest(probe)?
        {
            return Err(refused("external Live session"));
        }
        let session = self.host_write(ctx)?;
        let check = self.edit_check(session, &v, ctx)?;
        let q = self.calls()?;
        if name == method::MAP_RELOAD {
            return self.reload_map(params(v)?, ctx, q, check, input);
        }
        let call = match name {
            method::OBJECT_SET => {
                let p: wire::ObjectSetParams = params(v)?;
                if p.pos.is_none() && p.forward.is_none() && p.up.is_none() {
                    return Err(invalid("give pos, forward or up"));
                }
                if p.pos.is_some_and(|p| !p.iter().all(|c| c.abs() < 1e6)) {
                    return Err(invalid("pos: three finite world coordinates"));
                }
                Call::ObjectSet {
                    id: object_id(&p.id)?,
                    pos: p.pos,
                    forward: p.forward,
                    up: p.up,
                }
            }
            method::OBJECT_DELETE => {
                let p: wire::ObjectDeleteParams = params(v)?;
                Call::ObjectDelete { id: object_id(&p.id)? }
            }
            method::OBJECT_RESPAWN => {
                let p: wire::ObjectRespawnParams = params(v)?;
                let target = match (p.id, p.placement) {
                    (Some(id), None) => RespawnTarget::Object(object_id(&id)?),
                    (None, Some(place)) => RespawnTarget::Placement(place),
                    _ => return Err(invalid("give exactly one of id or placement")),
                };
                Call::ObjectRespawn { target }
            }
            _ => unreachable!("only the registered edit methods"),
        };
        self.adopt_editor()?;
        let pending = calls::queue_edit(&q, call, check)?;
        ctx.defer(CALL_TICKS, Box::new(move |_| pending.outcome()));
        Ok(Value::Null)
    }

    fn reload_map(
        &mut self,
        p: wire::MapReloadParams,
        ctx: &mut GameCtx<'_>,
        q: calls::Shared,
        check: calls::EditCheck,
        input: &Mutex<crate::Input>,
    ) -> R {
        let name = ctx
            .at
            .map
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| refused("the running map has no name"))?;
        let options = ctx.with_guest(|m| active_options(m))?;
        let mut pending = {
            let mut input = crate::tap::lock(input);
            input.allow_reload(&q).map_err(refused)?;
            calls::hold_reload(&q, check)?
        };
        let mut result = wire::MapReloadResult {
            queued: true,
            map: name,
            activation: pending.activation(),
            file: p.file,
            checksum: None,
        };
        if let Some(file) = &result.file {
            let path = Path::new(file);
            if !path.is_absolute() {
                return Err(refused("file must have an absolute path to a .map file"));
            }
            let map = &result.map;
            let filename = path.file_name().and_then(|s| s.to_str());
            if !filename.is_some_and(|f| f.eq_ignore_ascii_case(&format!("{map}.map"))) {
                return Err(refused(format!("file must be named {map}.map for the active map")));
            }
            // NUL-terminated scenario path (level_boot::OPT_PATH and OPT_PATH_LEN).
            let field = &options[0x1c..0x1c + 0x104];
            let end = field.iter().position(|b| *b == 0)
                .ok_or_else(|| refused("the active scenario path is unterminated"))?;
            let scenario = std::str::from_utf8(&field[..end]).ok().filter(|s| !s.is_empty())
                .ok_or_else(|| refused("the active scenario path is empty or invalid"))?;
            if self.map_rebind.is_none() {
                return Err(refused("this host has no writable editor map mount"));
            }
            self.adopt_editor()?;
            let prepare = self.map_rebind.as_ref().expect("checked writable mount");
            let guard = PreparationGuard::default();
            let worker = prepare(map, scenario, path, Arc::clone(&guard.0)).map_err(refused)?;
            let mut preparing = Some((worker, options));
            ctx.defer_file(Box::new(move |_| {
                // The claim remains live through queue admission. Dropping this
                // poll on a timeout or disconnect also revokes a running copy.
                let _guard = &guard;
                if let Some((worker, _)) = &preparing {
                    let binding = match worker.try_recv() {
                        Ok(Ok(binding)) => binding,
                        Ok(Err(why)) => return Some(Err(refused(why))),
                        Err(TryRecvError::Empty) => return None,
                        Err(TryRecvError::Disconnected) => {
                            return Some(Err(refused("the map preparation worker stopped")));
                        }
                    };
                    result.checksum = Some(binding.checksum);
                    let (_, options) = preparing.take().expect("preparing reload");
                    pending.schedule_reload(options, Some(binding));
                }
                pending.outcome().map(|out| out.map(|_| json!(result)))
            }));
        } else {
            self.adopt_editor()?;
            pending.schedule_reload(options, None);
            ctx.defer(
                CALL_TICKS,
                Box::new(move |_| pending.outcome().map(|out| out.map(|_| json!(result)))),
            );
        }
        Ok(Value::Null)
    }
}

/// GAME_ACTIVATE publishes the active options at globals +8. OPTIONS_QUEUE
/// takes a complete options block in EAX (docs/runtime/level-boot.md).
pub(super) fn active_options(m: &dyn GuestMemory) -> Result<Vec<u8>, RpcError> {
    use guest_layout::game::{GameGlobals, MODE_CAMPAIGN, MODE_MULTIPLAYER};
    let fault = |e: engine::MemFault| {
        refused(format!("cannot read active game options at {:#x}", e.addr))
    };
    let globals = GameGlobals::current(m)
        .map_err(fault)?
        .ok_or_else(|| refused("no game globals"))?;
    let options = globals.options();
    if globals.map_active(m).map_err(fault)? == 0
        || !matches!(
            options.mode(m).map_err(fault)?,
            MODE_CAMPAIGN | MODE_MULTIPLAYER
        )
    {
        return Err(refused("no active campaign or multiplayer map"));
    }
    m.bytes(options.addr, calls::OPTIONS_SIZE as usize)
        .map_err(fault)
}

pub(super) fn replay_options(bytes: Vec<u8>) -> Call {
    Call::QueueGame {
        steps: vec![
            GameStep::Write { offset: 0, bytes },
            GameStep::Call {
                target: engine::points::OPTIONS_QUEUE.base,
                args: vec![],
                eax: Some(Arg::Options(0)),
                esi: None,
                report: None,
            },
        ],
    }
}
