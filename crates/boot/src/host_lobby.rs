//! The light host is a network peer with no signed-in controller. The agent
//! submits work at a boundary, the next tick makes the retail calls, and the
//! method answers when the title acknowledges the transition.

use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use agent::{Access, Agent, GameCtx};
use agent_proto::host_lobby::OpenParams;
use agent_proto::{RpcError, code, method};
use feature::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy, points};
use live_proto::hopper::variant::VariantRecord;
use serde_json::{Value, json};

use crate::map_select::Catalogue;
use crate::new_maps::{Disc, IsoDisc};

mod retail;
pub(crate) use retail::{Machine as PlayerlessLobby, SimStart};
mod variants;
use self::variants::Variants;
#[cfg(test)]
mod dispatch_tests;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Variant {
    Stock(sim::config::GameType),
    Named(Box<VariantRecord>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Config {
    map: u32,
    variant: Variant,
    max_players: u8,
    live: bool,
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

impl Config {
    fn parse(
        params: OpenParams,
        maps: &Catalogue,
        variants: &Variants,
        has_live: bool,
    ) -> Result<Self, RpcError> {
        if !(1..=16).contains(&params.max_players) {
            return Err(invalid("max_players must be 1..=16"));
        }
        let name = params.map.trim();
        if name.is_empty() || name.len() > 260 {
            return Err(invalid("map must name a multiplayer map"));
        }
        let mut matches = maps.all().filter(|m| {
            m.short_name().eq_ignore_ascii_case(name)
                || m.path.eq_ignore_ascii_case(name)
                || m.name.eq_ignore_ascii_case(name)
        });
        let map = matches
            .next()
            .ok_or_else(|| invalid(format!("unknown multiplayer map {name:?}")))?
            .id;
        if matches.next().is_some() {
            return Err(invalid(
                "ambiguous multiplayer map name; use its scenario path",
            ));
        }
        let variant = variants.resolve(params.variant.as_deref())?;
        if params.live && !has_live {
            return Err(unavailable("live requires a configured Live service"));
        }
        Ok(Self {
            map,
            variant,
            max_players: params.max_players,
            live: params.live,
        })
    }

    fn class(&self) -> u32 {
        if self.live { 2 } else { 1 }
    }
}

enum Command {
    Open(Config),
    Start,
    Close,
}

type Answer = Result<Value, RpcError>;

struct Job {
    cmd: Command,
    reply: mpsc::Sender<Answer>,
    ticks: u64,
    until: Instant,
    waiting_for: &'static str,
}

#[derive(Default, PartialEq, Eq)]
enum TickStage {
    #[default]
    Idle,
    GuestCalls,
    RetailTick,
}

#[derive(Default)]
struct Queue {
    job: Option<Job>,
    readers: Vec<mpsc::Sender<Answer>>,
    machine: retail::Machine,
    stage: TickStage,
}

type Shared = Arc<Mutex<Queue>>;

fn submit(q: &Shared, ctx: &mut GameCtx<'_>, cmd: Option<Command>) -> Answer {
    let (reply, answer) = mpsc::channel();
    let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(cmd) = cmd {
        if q.job.is_some() || q.stage == TickStage::GuestCalls {
            return Err(RpcError::new(code::BUSY, "lobby transition in progress"));
        }
        q.job = Some(Job {
            cmd,
            reply,
            ticks: 0,
            until: Instant::now() + Duration::from_secs(55),
            waiting_for: "waiting for the first retail game tick",
        });
    } else {
        q.readers.push(reply);
    }
    ctx.defer(
        agent::DEFER_TICKS_MAX,
        Box::new(move |_| match answer.try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err(unavailable("lobby driver stopped"))),
        }),
    );
    Ok(Value::Null)
}

fn empty(params: Value) -> Result<(), RpcError> {
    if params.is_null() || params.as_object().is_some_and(|p| p.is_empty()) {
        return Ok(());
    }
    Err(invalid("expected an empty parameter object"))
}

fn methods(agent: &Agent, shared: &Shared, maps: Catalogue, variants: Variants, has_live: bool) {
    let q = shared.clone();
    agent.add_game_method(method::HOST_LOBBY_OPEN, Access::Act, move |p, ctx| {
        if !p.is_object() {
            return Err(invalid("host.lobby.open expects named parameters"));
        }
        let params = serde_json::from_value(p).map_err(|e| invalid(e.to_string()))?;
        submit(
            &q,
            ctx,
            Some(Command::Open(Config::parse(
                params, &maps, &variants, has_live,
            )?)),
        )
    });
    let q = shared.clone();
    agent.add_game_method(method::HOST_LOBBY_STATE, Access::Read, move |p, ctx| {
        empty(p)?;
        {
            let q = q.lock().unwrap_or_else(|e| e.into_inner());
            if q.stage != TickStage::GuestCalls {
                let mut observed = None;
                ctx.reader
                    .with_guest(&mut |m| observed = Some(q.machine.observe(m)));
                if let Some(result) = observed {
                    let (state, blocker) = result?;
                    // Only an eligible lobby needs retail's start check.
                    if blocker.is_some() {
                        return Ok(json!(state));
                    }
                }
            }
        }
        submit(&q, ctx, None)
    });
    let q = shared.clone();
    agent.add_game_method(method::HOST_LOBBY_START, Access::Act, move |p, ctx| {
        empty(p)?;
        submit(&q, ctx, Some(Command::Start))
    });
    let q = shared.clone();
    agent.add_game_method(method::HOST_LOBBY_CLOSE, Access::Act, move |p, ctx| {
        empty(p)?;
        submit(&q, ctx, Some(Command::Close))
    });
}

struct Lobby {
    q: Shared,
    live: Option<Arc<live_hle::Hle>>,
    machine_start: bool,
}

pub(crate) fn exclude_local_profiles(b: &mut Builder) {
    // PROFILE_SIGN_IN is void, stdcall (slot, block, file).
    b.raw()
        .before(&points::PROFILE_SIGN_IN, "no dedicated local seats", |c| {
            Ok(Flow::Return(c.ret(12)?))
        });
}

impl Feature for Lobby {
    fn manifest(&self) -> Manifest {
        Manifest::new("host-lobby", "Playerless dedicated network lobby")
            .doc("docs/live/dedicated-host.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::forbid_while(|| {
            Some("dedicated network host".into())
        }));
        exclude_local_profiles(b);
        let scratch = b.guest_alloc(
            crate::level_boot::VARIANT_SIZE,
            runtime::guest::Permissions::RW,
        )?;
        let (shared, live, machine_start) = (self.q.clone(), self.live.clone(), self.machine_start);
        b.raw()
            .before(&points::GAME_TICK, "drive dedicated lobby", move |c| {
                let mut q = shared.lock().unwrap_or_else(|e| e.into_inner());
                if q.stage != TickStage::Idle || (q.job.is_none() && q.readers.is_empty()) {
                    return Ok(Flow::Continue);
                }
                q.stage = TickStage::GuestCalls;
                let (mut machine, mut job, readers) = (
                    std::mem::take(&mut q.machine),
                    q.job.take(),
                    std::mem::take(&mut q.readers),
                );
                drop(q);
                let (shared, live) = (shared.clone(), live.clone());
                c.answer_async(move |cx| {
                    Box::pin(async move {
                        if let Some(j) = &mut job {
                            let out = if j.until <= Instant::now() {
                                Some(Err(RpcError::new(code::TIMEOUT, j.waiting_for)))
                            } else {
                                if j.ticks == 0 && matches!(&j.cmd, Command::Close) {
                                    machine.retry_close();
                                }
                                j.ticks += 1;
                                let advertised = machine.advertisement(&j.cmd, live.as_deref());
                                match machine
                                    .advance(
                                        &cx,
                                        &j.cmd,
                                        scratch,
                                        advertised.as_deref(),
                                        machine_start,
                                    )
                                    .await
                                {
                                    Ok(retail::Progress::Done(v)) => Some(Ok(v)),
                                    Ok(retail::Progress::Wait(why))
                                        if j.ticks >= agent::DEFER_TICKS_MAX - 1 =>
                                    {
                                        Some(Err(RpcError::new(code::TIMEOUT, why)))
                                    }
                                    Ok(retail::Progress::Wait(why)) => {
                                        j.waiting_for = why;
                                        None
                                    }
                                    Err(e) => Some(Err(e)),
                                }
                            };
                            if let Some(out) = out {
                                let _ = j.reply.send(out);
                                job = None;
                            }
                        }
                        if !readers.is_empty() {
                            let out = machine.state(&cx).await.map(|s| json!(s));
                            for reply in readers {
                                let _ = reply.send(out.clone());
                            }
                        }
                        // Publish progress before retail runs: a load can stop ticking.
                        // Its agent boundary may read state or enqueue the next command.
                        // Keep the hook guarded until the outer tick returns.
                        {
                            let mut q = shared.lock().unwrap_or_else(|e| e.into_inner());
                            q.machine = machine;
                            q.job = job;
                            q.stage = TickStage::RetailTick;
                        }
                        let result = cx
                            .call(engine::entry::GuestCall::new(points::GAME_TICK.base))
                            .await;
                        shared.lock().unwrap_or_else(|e| e.into_inner()).stage = TickStage::Idle;
                        Ok(engine::entry::Ret::new(result?.eax, 0))
                    })
                })
            })
            .first()
            .after_feature(agent::sim::calls::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE);
        Ok(())
    }
}

pub(crate) fn install(
    features: &mut feature::Registry,
    agent: &Agent,
    host_light: bool,
    opts: &crate::RunOpts,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<()> {
    if !host_light || std::env::var("H2_HOST_LOBBY").as_deref() != Ok("1") {
        return Ok(());
    }
    let q = Shared::default();
    let lobby = Lobby {
        q: q.clone(),
        live: live.clone(),
        machine_start: std::env::var("H2_HOST_LOBBY_MACHINE_START").as_deref() != Ok("0"),
    };
    if features.add(&lobby).map_err(anyhow::Error::msg)? {
        let mut disc = IsoDisc::new(&opts.disc);
        let slots = disc.slots()?;
        let variants = Variants::load(&mut disc, &opts.data_root)?;
        let content = vfs::Config::new(&opts.data_root, crate::new_maps::HALO2_TITLE)
            .title_data_dir()
            .join(vfs::CONTENT_DIR);
        methods(
            agent,
            &q,
            crate::map_select::read(&slots, &content),
            variants,
            live.is_some_and(|h| h.config().server_url.is_some()),
        );
    }
    Ok(())
}
