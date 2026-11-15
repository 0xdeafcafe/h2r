use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use runtime::dispatch::Dispatch;
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, Runtime, TransferAbi, abi};

use crate::config::{TelemetryConfig, TelemetrySinkConfig};
use crate::layout;
use crate::memory;
use crate::server::ServerSink;
use crate::session::Session;
use crate::sink::{JsonlSink, QueuedSink};

const SHADOW_START: u32 = 0xa0ff_5000;
const SHADOW_TICK: u32 = SHADOW_START + 4;
const SHADOW_END: u32 = SHADOW_START + 8;
const SHADOW_BYTES: u32 = 0x1000;

#[derive(Clone, Copy)]
enum PostHook {
    Start,
    Tick,
    End,
}

pub struct Telemetry {
    session: Mutex<Session>,
    sink: QueuedSink,
    disabled: AtomicBool,
    last_tick: Mutex<Option<u64>>,
}

impl Telemetry {
    fn new(session: Session, sink: QueuedSink) -> Self {
        Self {
            session: Mutex::new(session),
            sink,
            disabled: AtomicBool::new(false),
            last_tick: Mutex::new(None),
        }
    }

    pub fn disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    fn disable(&self, reason: impl AsRef<str>) {
        if !self.disabled.swap(true, Ordering::AcqRel) {
            self.sink.log_error(reason.as_ref());
        }
    }

    fn protect(&self, f: impl FnOnce() -> Result<(), String>) {
        if self.disabled() {
            return;
        }
        if let Some(error) = self.sink.worker_error() {
            self.disable(error);
            return;
        }
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => self.disable(error),
            Err(_) => self.disable("panic in observer"),
        }
    }

    fn publish(&self) -> Result<(), String> {
        let output = self.session.lock().unwrap().take_output();
        for output in output {
            match output {
                crate::event::Output::Event(event) => self.sink.event(event)?,
                crate::event::Output::Frame(frame) => self.sink.frame(frame),
            }
        }
        Ok(())
    }

    fn start(&self, guest: &runtime::Guest) -> Result<(), String> {
        let info = memory::game_info(guest).map_err(|e| format!("read game options: {e:?}"))?;
        let players = if info.is_match() {
            memory::players(guest).map_err(|e| format!("read players: {e:?}"))?
        } else {
            Vec::new()
        };
        let teams = if info.is_match() {
            memory::team_scores(guest, &players).map_err(|e| format!("read teams: {e:?}"))?
        } else {
            Vec::new()
        };
        self.session
            .lock()
            .unwrap()
            .start_with_teams(&info, players, teams);
        *self.last_tick.lock().unwrap() = Some(info.tick);
        self.publish()
    }

    fn sample(&self, guest: &runtime::Guest) -> Result<(), String> {
        let snapshot = memory::snapshot(guest).map_err(|e| format!("read tick snapshot: {e:?}"))?;
        {
            let mut last = self.last_tick.lock().unwrap();
            if *last == Some(snapshot.tick) {
                return Ok(());
            }
            *last = Some(snapshot.tick);
        }
        self.session.lock().unwrap().sample(&snapshot);
        self.publish()
    }

    fn end(&self, guest: &runtime::Guest) -> Result<(), String> {
        let globals = guest
            .u32(layout::GAME_GLOBALS_PTR)
            .map_err(|e| format!("read game globals: {e:?}"))?;
        if globals == 0
            || guest
                .u8(globals + layout::GAME_ENDED)
                .map_err(|e| format!("read end latch: {e:?}"))?
                == 0
        {
            return Ok(());
        }
        let snapshot =
            memory::snapshot(guest).map_err(|e| format!("read final snapshot: {e:?}"))?;
        self.session.lock().unwrap().end(&snapshot);
        self.publish()
    }

    fn kill(
        &self,
        guest: &runtime::Guest,
        attacker: u32,
        victim: u32,
        betrayal: bool,
    ) -> Result<(), String> {
        let tick = memory::game_tick(guest).map_err(|e| format!("read kill tick: {e:?}"))?;
        let players = memory::players(guest).map_err(|e| format!("read kill players: {e:?}"))?;
        let victim_index = victim & 0xffff;
        let unit = players
            .iter()
            .find(|p| p.player.index == victim_index)
            .map_or(u32::MAX, |p| p.unit);
        let object = memory::object(guest, unit).map_err(|e| format!("read kill object: {e:?}"))?;
        self.session.lock().unwrap().kill(
            tick,
            (attacker != u32::MAX).then_some(attacker & 0xffff),
            victim_index,
            betrayal,
            object,
        );
        self.publish()
    }

    fn post(&self, kind: PostHook, cpu: &CpuAbi, ctx: &mut ExecCtx) {
        self.protect(|| {
            let guest = ctx.memory();
            match kind {
                PostHook::Start if cpu.eax & 0xff != 0 => self.start(&guest),
                PostHook::Start => Ok(()),
                PostHook::Tick => self.sample(&guest),
                PostHook::End => self.end(&guest),
            }
        });
    }
}

/// Install the configured observer. `Ok(None)` means disabled; installation
/// errors never replace an existing override.
pub fn install(
    runtime: &Runtime,
    config: &TelemetryConfig,
) -> Result<Option<Arc<Telemetry>>, String> {
    if !config.enabled {
        return Ok(None);
    }
    let sink = match &config.sink {
        TelemetrySinkConfig::File(path) => QueuedSink::new(JsonlSink::create(path)?, 256, 2),
        TelemetrySinkConfig::Server { base_url, account } => {
            QueuedSink::new(ServerSink::new(base_url.clone(), account.clone())?, 256, 2)
        }
    };
    let telemetry = Arc::new(Telemetry::new(
        Session::new("h2recomp", Some("h2recomp".into())),
        sink,
    ));
    for address in [
        layout::MATCH_START,
        layout::GAME_TICK,
        layout::GAME_END,
        layout::PLAYER_KILLED,
    ] {
        if runtime.resolve_original(address).is_none() {
            return Err(format!("telemetry original {address:#x} is not registered"));
        }
    }
    runtime
        .memory()
        .map_zeroed(SHADOW_START, SHADOW_BYTES, Permissions::RX)
        .map_err(|e| format!("map telemetry shadow entries: {e:?}"))?;
    runtime.edit_dispatch(|dispatch| {
        for address in [
            layout::MATCH_START,
            layout::GAME_TICK,
            layout::GAME_END,
            layout::PLAYER_KILLED,
            SHADOW_START,
            SHADOW_TICK,
            SHADOW_END,
        ] {
            if dispatch.override_name(address).is_some() {
                return Err(format!(
                    "telemetry override {address:#x} is already installed"
                ));
            }
        }
        install_post(
            dispatch,
            runtime,
            layout::MATCH_START,
            SHADOW_START,
            PostHook::Start,
            Arc::clone(&telemetry),
        );
        install_post(
            dispatch,
            runtime,
            layout::GAME_TICK,
            SHADOW_TICK,
            PostHook::Tick,
            Arc::clone(&telemetry),
        );
        install_post(
            dispatch,
            runtime,
            layout::GAME_END,
            SHADOW_END,
            PostHook::End,
            Arc::clone(&telemetry),
        );

        let original = runtime.resolve_original(layout::PLAYER_KILLED).unwrap();
        let observer = Arc::clone(&telemetry);
        dispatch.register_override(
            layout::PLAYER_KILLED,
            "live telemetry kill",
            move |cpu, ctx, _| {
                let mut observation = || {
                    let guest = ctx.memory();
                    let attacker = guest.u32(cpu.esp.wrapping_add(4))?;
                    let victim = guest.u32(cpu.esp.wrapping_add(8))?;
                    let betrayal = guest.u32(cpu.esp.wrapping_add(0x0c))? != 0;
                    let _packed_damage = guest.u32(cpu.esp.wrapping_add(0x10))?;
                    Ok::<_, runtime::GuestFault>((attacker, victim, betrayal))
                };
                if let Ok((attacker, victim, betrayal)) = observation() {
                    observer.protect(|| {
                        let guest = ctx.memory();
                        observer.kill(&guest, attacker, victim, betrayal)
                    });
                }
                Ok(original.invoke(cpu, ctx, layout::PLAYER_KILLED))
            },
        );
        Ok(())
    })?;
    Ok(Some(telemetry))
}

fn install_post(
    dispatch: &mut Dispatch,
    runtime: &Runtime,
    address: u32,
    shadow: u32,
    kind: PostHook,
    telemetry: Arc<Telemetry>,
) {
    let original_shadow = runtime
        .resolve_original(address)
        .expect("generated original checked");
    dispatch.register_override(shadow, "live telemetry original", move |cpu, ctx, _| {
        Ok(original_shadow.invoke(cpu, ctx, address))
    });
    let original_fallback = runtime
        .resolve_original(address)
        .expect("generated original checked");
    dispatch.register_override(address, "live telemetry observer", move |cpu, ctx, _| {
        let return_pc_result = {
            let memory = ctx.memory();
            memory.u32(cpu.esp)
        };
        let return_pc = match return_pc_result {
            Ok(pc) => pc,
            Err(_) => return Ok(original_fallback.invoke(cpu, ctx, address)),
        };
        cpu.esp = cpu.esp.wrapping_add(4);
        let observer = Arc::clone(&telemetry);
        let transfer = {
            let runtime = Arc::clone(ctx.runtime());
            let mut memory = runtime.memory();
            ctx.call_guest(cpu, &mut memory, shadow, move |cpu, ctx| {
                observer.post(kind, cpu, ctx);
                Ok(TransferAbi::ret(return_pc))
            })
        };
        match transfer {
            Ok(transfer) if transfer.kind != abi::FAULT => Ok(transfer),
            _ => {
                cpu.esp = cpu.esp.wrapping_sub(4);
                Ok(original_fallback.invoke(cpu, ctx, address))
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{GameInfo, PlayerState};
    use crate::memory::fixture;

    #[test]
    fn observer_failure_disables_without_panicking() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-failure-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            QueuedSink::new(JsonlSink::create(&path).unwrap(), 2, 1),
        );
        let guest = runtime::Guest::new();
        telemetry.protect(|| telemetry.start(&guest));
        assert!(telemetry.disabled());
        telemetry.protect(|| telemetry.start(&guest));
        drop(telemetry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn successful_start_publishes_current_roster() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-start-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            QueuedSink::new(JsonlSink::create(&path).unwrap(), 4, 1),
        );
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 4).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, u32::MAX);
        telemetry.protect(|| telemetry.start(&guest));
        assert!(!telemetry.disabled());
        assert!(telemetry.session.lock().unwrap().game_id().is_some());
        drop(telemetry);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("GameStarted"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn salted_kill_datums_become_player_slots() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-kill-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            QueuedSink::new(JsonlSink::create(&path).unwrap(), 8, 1),
        );
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 9).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, 0x90010000);
        fixture::put_player(&mut guest, 1, "B", 0, false, fixture::object_datum(1));
        fixture::put_object(&mut guest, 0, u32::MAX);
        fixture::put_object(&mut guest, 1, u32::MAX);
        let object = memory::object(&guest, fixture::object_datum(1))
            .unwrap()
            .unwrap();
        assert_eq!(object.pos, [1.0, 2.0, 3.0]);
        telemetry.protect(|| telemetry.start(&guest));
        telemetry.protect(|| telemetry.kill(&guest, 0x80010000, 0x80010001, false));
        telemetry.protect(|| telemetry.kill(&guest, 0x80010000, 0x80010000, false));
        drop(telemetry);
        let events: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|event: &serde_json::Value| event["type"] == "PlayerKilled")
            .collect();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["data"]["victim"], 1);
        assert_eq!(events[0]["data"]["kind"], "kill");
        assert_eq!(events[0]["data"]["pos"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(events[1]["data"]["victim"], 0);
        assert_eq!(events[1]["data"]["kind"], "suicide");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn disabled_config_is_a_noop() {
        assert!(!TelemetryConfig::default().enabled);
        let info = GameInfo {
            map: "lockout".into(),
            mode: "main_menu".into(),
            variant: None,
            team_play: false,
            score_limit: None,
            time_limit_s: None,
            tick_rate: 30,
            tick: 0,
        };
        let mut session = Session::new("test", None);
        session.start(&info, Vec::<PlayerState>::new());
        assert!(session.take_output().is_empty());
    }
}
