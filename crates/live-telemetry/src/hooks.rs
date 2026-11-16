use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use runtime::dispatch::Dispatch;
use runtime::exec::OriginalHandler;
use runtime::guest::Permissions;
use runtime::{CpuAbi, ExecCtx, Runtime, TransferAbi, abi};

use crate::config::{TelemetryConfig, TelemetrySinkConfig};
use crate::layout;
use crate::memory;
use crate::server::ServerSink;
use crate::session::Session;
use crate::sink::{JsonlSink, QueuedSink};
use crate::tags::{MapLocator, TagNames};

/// Weapons with uncollected shots are few (only those fired since the last
/// completed tick); past this many the counts are dropped, not grown.
const MAX_FIRING_WEAPONS: usize = 256;

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
    /// Shots by weapon object datum since the last sample, from the weapon
    /// fire hook.
    shots: Mutex<BTreeMap<u32, u32>>,
    /// Finds the loaded map's file for tag names; set by the boot once its
    /// VFS exists.
    maps: Mutex<Option<MapLocator>>,
}

impl Telemetry {
    fn new(session: Session, sink: QueuedSink) -> Self {
        Self {
            session: Mutex::new(session),
            sink,
            disabled: AtomicBool::new(false),
            last_tick: Mutex::new(None),
            shots: Mutex::new(BTreeMap::new()),
            maps: Mutex::new(None),
        }
    }

    /// Lets each match look up its map's biped and weapon tag names (frame
    /// `species` and `weapon`). Without it those fields are never sent.
    pub fn set_map_locator(&self, locator: MapLocator) {
        *self.maps.lock().unwrap() = Some(locator);
    }

    /// The loaded map's names, if the host has its file and the file's
    /// scenario datum is the guest's.
    fn tag_names(&self, guest: &runtime::Guest, map: &str) -> Option<TagNames> {
        if !live_proto::game::valid_id(map) {
            return None;
        }
        let file = {
            let maps = self.maps.lock().unwrap();
            (maps.as_ref()?)(map)?
        };
        let names = match TagNames::load(&file) {
            Ok(names) => names,
            Err(error) => {
                eprintln!("live telemetry: no tag names for {map}: {error}");
                return None;
            }
        };
        let scenario = memory::tag_index_scenario(guest).ok()?;
        if !names.is_for(scenario) {
            eprintln!(
                "live telemetry: {} is not the loaded {map}",
                file.path.display()
            );
            return None;
        }
        Some(names)
    }

    /// The weapon fire hook: one shot of the weapon object `weapon`.
    fn fired(&self, weapon: u32) {
        if self.disabled() {
            return;
        }
        let mut shots = self.shots.lock().unwrap();
        if shots.len() >= MAX_FIRING_WEAPONS && !shots.contains_key(&weapon) {
            shots.clear();
        }
        let count = shots.entry(weapon).or_insert(0);
        *count = count.saturating_add(1);
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
        let names = if info.is_match() {
            self.tag_names(guest, &info.map)
        } else {
            None
        };
        {
            let mut session = self.session.lock().unwrap();
            session.start_with_teams(&info, players, teams);
            if let Some(names) = names {
                session.set_tag_names(names);
            }
        }
        self.shots.lock().unwrap().clear();
        *self.last_tick.lock().unwrap() = Some(info.tick);
        self.publish()
    }

    fn sample(&self, guest: &runtime::Guest) -> Result<(), String> {
        let mut snapshot =
            memory::snapshot(guest).map_err(|e| format!("read tick snapshot: {e:?}"))?;
        {
            let mut last = self.last_tick.lock().unwrap();
            if *last == Some(snapshot.tick) {
                return Ok(());
            }
            *last = Some(snapshot.tick);
        }
        snapshot.shots = std::mem::take(&mut *self.shots.lock().unwrap());
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
    install_at(runtime, config, Hooks::RETAIL)
}

/// The guest entries telemetry wraps.
#[derive(Clone, Copy)]
struct Hooks {
    start: u32,
    tick: u32,
    end: u32,
    kill: u32,
    /// Optional: without it frames carry no `shots`.
    fire: u32,
}

impl Hooks {
    const RETAIL: Self = Self {
        start: layout::MATCH_START,
        tick: layout::GAME_TICK,
        end: layout::GAME_END,
        kill: layout::PLAYER_KILLED,
        fire: layout::WEAPON_FIRE,
    };
}

fn install_at(
    runtime: &Runtime,
    config: &TelemetryConfig,
    hooks: Hooks,
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
        Session::new("h2recomp", Some("h2recomp".into())).with_signed_in(signed_in(config)),
        sink,
    ));
    // Resolve every original before edit_dispatch: resolve_original takes the
    // dispatch read lock, and calling it under the write lock deadlocks boot.
    let original = |address: u32| {
        runtime
            .resolve_original(address)
            .ok_or_else(|| format!("telemetry original {address:#x} is not registered"))
    };
    let start = (original(hooks.start)?, original(hooks.start)?);
    let tick = (original(hooks.tick)?, original(hooks.tick)?);
    let end = (original(hooks.end)?, original(hooks.end)?);
    let kill_original = original(hooks.kill)?;
    // The fire hook only adds `shots`: if its entry is missing the rest of
    // telemetry still installs.
    let fire_original = runtime.resolve_original(hooks.fire);
    if fire_original.is_none() {
        eprintln!(
            "live telemetry: weapon fire {:#x} is not registered; frames carry no shots",
            hooks.fire
        );
    }
    runtime
        .memory()
        .map_zeroed(SHADOW_START, SHADOW_BYTES, Permissions::RX)
        .map_err(|e| format!("map telemetry shadow entries: {e:?}"))?;
    runtime.edit_dispatch(|dispatch| {
        for address in [
            hooks.start,
            hooks.tick,
            hooks.end,
            hooks.kill,
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
            start,
            hooks.start,
            SHADOW_START,
            PostHook::Start,
            Arc::clone(&telemetry),
        );
        install_post(
            dispatch,
            tick,
            hooks.tick,
            SHADOW_TICK,
            PostHook::Tick,
            Arc::clone(&telemetry),
        );
        install_post(
            dispatch,
            end,
            hooks.end,
            SHADOW_END,
            PostHook::End,
            Arc::clone(&telemetry),
        );

        if let Some(original) = fire_original {
            if dispatch.override_name(hooks.fire).is_some() {
                eprintln!(
                    "live telemetry: weapon fire {:#x} is already overridden; frames carry no shots",
                    hooks.fire
                );
            } else {
                let observer = Arc::clone(&telemetry);
                let address = hooks.fire;
                dispatch.register_override(
                    address,
                    "live telemetry weapon fire",
                    move |cpu, ctx, _| {
                        // stdcall (weapon object datum, barrel index): read
                        // the first argument, then always run the original.
                        let weapon = ctx.memory().u32(cpu.esp.wrapping_add(4));
                        if let Ok(weapon) = weapon {
                            observer.fired(weapon);
                        }
                        Ok(original.invoke(cpu, ctx, address))
                    },
                );
            }
        }

        let original = kill_original;
        let observer = Arc::clone(&telemetry);
        dispatch.register_override(hooks.kill, "live telemetry kill", move |cpu, ctx, _| {
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
            Ok(original.invoke(cpu, ctx, hooks.kill))
        });
        Ok(())
    })?;
    Ok(Some(telemetry))
}

/// The Live account the game is signed in with: the one whose key signs the
/// events. Offline capture has none.
fn signed_in(config: &TelemetryConfig) -> Option<live_proto::Xuid> {
    match &config.sink {
        TelemetrySinkConfig::Server { account, .. } => live_proto::Xuid::parse(&account.xuid).ok(),
        TelemetrySinkConfig::File(_) => None,
    }
}

fn install_post(
    dispatch: &mut Dispatch,
    (original_shadow, original_fallback): (OriginalHandler, OriginalHandler),
    address: u32,
    shadow: u32,
    kind: PostHook,
    telemetry: Arc<Telemetry>,
) {
    dispatch.register_override(shadow, "live telemetry original", move |cpu, ctx, _| {
        Ok(original_shadow.invoke(cpu, ctx, address))
    });
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
    fn fired_shots_reach_the_holders_frame() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-shots-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            QueuedSink::new(JsonlSink::create(&path).unwrap(), 8, 8),
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
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 1).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, fixture::object_datum(1));
        fixture::put_object(&mut guest, 1, u32::MAX);
        fixture::put_unit(&mut guest, 1, 0xe100_0010, [1.0, 0.0, 0.0], 0);
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        telemetry.protect(|| telemetry.start(&guest));
        // Shots before the match's first sample are the previous match's.
        telemetry.fired(fixture::object_datum(5));
        telemetry.protect(|| telemetry.start(&guest));
        telemetry.fired(fixture::object_datum(5));
        telemetry.fired(fixture::object_datum(5));
        telemetry.fired(fixture::object_datum(9)); // nobody's weapon
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 2).unwrap();
        telemetry.protect(|| telemetry.sample(&guest));
        assert!(telemetry.shots.lock().unwrap().is_empty(), "drained");
        telemetry.fired(fixture::object_datum(5));
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 4).unwrap();
        telemetry.protect(|| telemetry.sample(&guest));
        assert!(!telemetry.disabled());
        drop(telemetry);
        let frames: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|row: &serde_json::Value| row["type"] == "GameFrame")
            .collect();
        // The sink may coalesce the two frames; their shots then add up.
        let shots: u64 = frames
            .iter()
            .map(|f| f["data"]["players"][0]["shots"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(shots, 3, "{frames:?}");
        assert_eq!(
            frames[0]["data"]["players"][0]["aim"],
            serde_json::json!([1.0, 0.0, 0.0])
        );
        assert!(
            frames[0]["data"]["players"][0].get("weapon").is_none(),
            "no names without a map file"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn fire_counts_are_bounded() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-bounded-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            QueuedSink::new(JsonlSink::create(&path).unwrap(), 2, 1),
        );
        for weapon in 0..(MAX_FIRING_WEAPONS as u32 + 10) {
            telemetry.fired(weapon);
        }
        assert!(telemetry.shots.lock().unwrap().len() <= MAX_FIRING_WEAPONS);
        drop(telemetry);
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

    /// Regression: install used to resolve originals under the dispatch write
    /// lock and hung boot forever whenever Live was on.
    #[test]
    fn install_on_a_real_runtime_returns() {
        use runtime::guest::Guest;
        use runtime::{Dispatch, RegisteredModule};
        let mut dispatch = Dispatch::default();
        // SAFETY: the fixture's tables, handlers and probes follow its ABI.
        let registered = unsafe {
            dispatch.register_generated_module(
                &runtime_test_probe::h2gen_fixture_module,
                runtime_test_probe::h2gen_fixture_module_abi_version,
            )
        };
        assert!(matches!(registered, Ok(RegisteredModule { .. })));
        let runtime = Runtime::new(Guest::new(), dispatch);
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-install-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let config = TelemetryConfig {
            enabled: true,
            sink: TelemetrySinkConfig::File(path.clone()),
        };
        // The fixture module's four entries stand in for the retail hooks.
        let hooks = Hooks {
            start: 0x40_1000,
            tick: 0x40_2000,
            end: 0x40_3000,
            kill: 0x40_4000,
            fire: 0x40_3005,
        };
        let (done, wait) = std::sync::mpsc::channel();
        let rt = Arc::clone(&runtime);
        std::thread::spawn(move || {
            let result = install_at(&rt, &config, hooks).map(|t| t.is_some());
            let names = rt.edit_dispatch(|d| {
                [hooks.start, hooks.kill, hooks.fire, SHADOW_START]
                    .map(|a| d.override_name(a).map(str::to_owned))
            });
            let _ = done.send((result, names));
        });
        let (result, names) = wait
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("install deadlocked");
        assert_eq!(result, Ok(true));
        assert_eq!(
            names,
            [
                Some("live telemetry observer".into()),
                Some("live telemetry kill".into()),
                Some("live telemetry weapon fire".into()),
                Some("live telemetry original".into()),
            ]
        );
        let _ = std::fs::remove_file(path);
    }

    /// The weapon fire hook is optional: an image without that entry still
    /// gets the rest of telemetry.
    #[test]
    fn install_without_the_fire_entry_still_installs() {
        use runtime::guest::Guest;
        use runtime::{Dispatch, RegisteredModule};
        let mut dispatch = Dispatch::default();
        // SAFETY: the fixture's tables, handlers and probes follow its ABI.
        let registered = unsafe {
            dispatch.register_generated_module(
                &runtime_test_probe::h2gen_fixture_module,
                runtime_test_probe::h2gen_fixture_module_abi_version,
            )
        };
        assert!(matches!(registered, Ok(RegisteredModule { .. })));
        let runtime = Runtime::new(Guest::new(), dispatch);
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-nofire-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let config = TelemetryConfig {
            enabled: true,
            sink: TelemetrySinkConfig::File(path.clone()),
        };
        let hooks = Hooks {
            start: 0x40_1000,
            tick: 0x40_2000,
            end: 0x40_3000,
            kill: 0x40_4000,
            fire: 0x40_5000,
        };
        let (done, wait) = std::sync::mpsc::channel();
        let rt = Arc::clone(&runtime);
        std::thread::spawn(move || {
            let result = install_at(&rt, &config, hooks).map(|t| t.is_some());
            let _ = done.send(result);
        });
        let result = wait
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("install deadlocked");
        assert_eq!(result, Ok(true));
        let _ = std::fs::remove_file(path);
    }
}
