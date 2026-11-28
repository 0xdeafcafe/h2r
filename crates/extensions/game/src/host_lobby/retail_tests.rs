use super::*;
use guest_layout::entry::{EntryError, EntryFuture, Regs, Ret, testing};
use guest_layout::testing::FakeGuest;

const S: u32 = 0x8100_0000;
const GAME: u32 = 0x8200_0000;
const SCRATCH: u32 = 0x8300_0000;
const WORLD: u32 = 0x8400_0000;
const TARGET: u32 = 0x8600_0000;
const MAP: u32 = 50;

fn menu() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(S, 0x7900)
        .map(TARGET, 0x7900)
        .put_u32(lobby::TARGET_SESSION, TARGET)
        .map(GAME, 0x2000)
        .map(SCRATCH, level_boot::VARIANT_SIZE)
        .map(WORLD, 0x1000)
        .put_u32(points::GAME_GLOBALS.base, GAME)
        .put_u32(points::SIM_GLOBALS.base, WORLD)
        .put_u8(GAME + 0x1120, 1)
        .put_u32(GAME + telemetry::layout::OPTIONS, level_boot::MODE_MAINMENU)
        .put_u8(lobby::NETWORKING, 1)
        .put_u8(lobby::LOBBY_ON, 1)
        .put_u32(lobby::SESSION, S);
    for port in 0..4 {
        m.put_u32(bots::guest::slot(port), 0).put_u32(
            bots::guest::slot(port) + bots::guest::SLOT_LOCAL_USER,
            u32::MAX,
        );
    }
    m
}

fn config() -> Config {
    Config {
        map: MAP,
        variant: Variant::Stock(sim_bench::config::GameType::Slayer),
        max_players: 2,
        live: false,
    }
}

fn peer(m: &mut dyn GuestMemory, slot: u32, peer: u32, name: &str) -> Result<(), MemFault> {
    let p = S + PLAYERS + slot * PLAYER_SIZE;
    m.set_u32(p + P_PEER, peer)?;
    let name: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    m.write(p + P_EFFECTIVE, &name)?;
    let mask = m.u32(S + PLAYER_MASK)? | (1 << slot);
    m.set_u32(S + PLAYER_COUNT, mask.count_ones())?;
    m.set_u32(S + PLAYER_MASK, mask)
}

fn rpc(e: RpcError) -> EntryError {
    EntryError::IsolatedCall(e.to_string())
}

fn reply(f: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
    let eax = match f.target {
        t if t == points::UI_LEAVE_SESSIONS.base => {
            assert_eq!(f.args, [1]);
            0
        }
        t if t == points::BROWSER_CREATE_SQUAD.base || t == CREATE_SQUAD => {
            let class = if f.target == points::BROWSER_CREATE_SQUAD.base {
                assert_eq!(f.args, [0, 0]);
                1
            } else {
                assert_eq!(f.args, [2, 0, 0]);
                2
            };
            m.set_u32(S + STATE, 5).unwrap();
            m.set_u32(S + SESSION_NETWORK_CLASS, class).unwrap();
            m.set_u32(S + PEER_COUNT, 1).unwrap();
            m.set_u32(S + CLASS, 1).unwrap();
            m.set_u32(S + PROTOCOL, 2).unwrap();
            m.set_u32(S + MAX_PLAYERS, 16).unwrap();
            m.set_u32(lobby::SESSION_TYPE, 1).unwrap();
            1
        }
        t if t == points::LOBBY_MODE_SET.base => {
            assert_eq!(f.args, [2]);
            0
        }
        t if t == points::LOBBY_SET_MAP.base => {
            assert_eq!(f.args, [u32::MAX]);
            m.set_u32(S + coop_probe::layout::MAP, f.esi.unwrap())
                .unwrap();
            1
        }
        level_boot::VARIANT_DEFAULTS => {
            assert_eq!(f.args, [SCRATCH, 0]);
            0
        }
        t if t == SET_VARIANT.base => {
            assert_eq!(m.u32(SCRATCH + variant::MAX_PLAYERS as u32), Ok(2));
            let raw = m.bytes(SCRATCH, level_boot::VARIANT_SIZE as usize).unwrap();
            m.write(S + VARIANT, &raw).unwrap();
            1
        }
        t if t == points::LOBBY_START_STATE.base => lobby::READY,
        CONTROLLER_COUNTDOWN => {
            assert_eq!(f.eax, Some(0));
            0
        }
        MACHINE_LOAD_REQUEST => {
            // 0x6e4d7's convention: session in ESI, stage in EDI, plain ret.
            assert!(f.args.is_empty(), "no stack arguments");
            assert_eq!((f.esi, f.edi), (Some(S), Some(STAGE_LOAD)));
            m.set_u32(S + CLASS, STAGE_LOAD).unwrap();
            0
        }
        LEAVE_SESSIONS => {
            assert!(f.args.is_empty());
            m.set_u32(S + STATE, 0).unwrap();
            m.set_u32(S + PLAYER_COUNT, 0).unwrap();
            m.set_u32(S + PLAYER_MASK, 0).unwrap();
            0
        }
        level_boot::REQUEST_MAINMENU => {
            m.set_u32(GAME + telemetry::layout::OPTIONS, level_boot::MODE_MAINMENU)
                .unwrap();
            0
        }
        other => panic!("unexpected guest call {other:#x}"),
    };
    Regs {
        eax,
        ..Default::default()
    }
}

#[test]
fn browser_create_waits_for_network_allocations_and_pregame_acknowledgement() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let mut machine = Machine::default();
            let config = config();
            for (squad, target, reason) in [
                (0, TARGET, "waiting for the squad allocation"),
                (S, 0, "waiting for the target session allocation"),
            ] {
                cx.write(|m| -> Result<(), MemFault> {
                    m.set_u32(lobby::SESSION, squad)?;
                    m.set_u32(lobby::TARGET_SESSION, target)
                })?;
                assert_eq!(
                    machine
                        .open(&cx, &config, SCRATCH, None, &[])
                        .await
                        .map_err(rpc)?,
                    SimOpen::Wait(reason)
                );
                assert!(machine.owned_squad().is_none());
            }
            cx.write(|m| m.set_u32(lobby::TARGET_SESSION, TARGET))?;
            assert_eq!(
                machine
                    .open(&cx, &config, SCRATCH, None, &[])
                    .await
                    .map_err(rpc)?,
                SimOpen::SquadCreated
            );

            // Retail owns the lifecycle. A successful browser call alone
            // does not permit the map/variant setters or a match start.
            cx.write(|m| m.set_u32(S + STATE, 3))?;
            assert_eq!(
                machine.open(&cx, &config, SCRATCH, None, &[]).await.map_err(rpc)?,
                SimOpen::Wait("waiting for the retail squad to reach host state 5")
            );
            cx.write(|m| m.set_u32(S + STATE, session::STATE_LIVE))?;
            assert_eq!(
                machine
                    .open(&cx, &config, SCRATCH, None, &[])
                    .await
                    .map_err(rpc)?,
                SimOpen::Wait("waiting for the retail pregame life cycle")
            );
            assert!(cx.read(|m| machine.owned_squad().unwrap().playerless_host(m)));
            cx.write(|m| m.set_u32(lobby::SESSION_TYPE, 1))?;
            assert_eq!(
                machine
                    .open(&cx, &config, SCRATCH, None, &[])
                    .await
                    .map_err(rpc)?,
                SimOpen::SessionStarted
            );
            assert_eq!(cx.read(|m| m.u32(S + PLAYER_COUNT))?, 0);
            Ok(Ret::new(1, 0))
        })
    };
    let run = testing::run(&test, menu(), Regs::default(), |call, m| {
        let result = reply(call, m);
        if call.target == points::BROWSER_CREATE_SQUAD.base {
            m.set_u32(lobby::SESSION_TYPE, 0).unwrap();
        }
        result
    });
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert_eq!(
        run.calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        [
            points::BROWSER_CREATE_SQUAD.base,
            points::LOBBY_MODE_SET.base
        ]
    );
}

#[test]
fn the_menu_guard_only_keeps_the_driver_owned_playerless_host() {
    let id = SquadId {
        addr: S,
        class: 1,
        incarnation: 9,
    };
    let mut m = menu();
    m.put_u32(S + INCARNATION, 9)
        .put_u32(S + STATE, session::STATE_LIVE)
        .put_u32(S + SESSION_NETWORK_CLASS, 1)
        .put_u32(S + PEER_COUNT, 5);
    assert!(id.playerless_host(&m));
    assert_eq!(id.is_current(&m), Some(true));
    assert_eq!(id.is_current(&FakeGuest::new()), None);
    let mut replacement = m.clone();
    replacement.put_u32(S + INCARNATION, 10);
    assert_eq!(id.is_current(&replacement), Some(false));
    for (field, value) in [
        (lobby::SESSION, TARGET),
        (S + INCARNATION, 10),
        (S + STATE, 0),
        (S + SESSION_NETWORK_CLASS, 2),
        (S + HOST_PEER, 1),
        (bots::guest::slot(3), bots::guest::SLOT_PROFILE),
    ] {
        let mut other = m.clone();
        other.put_u32(field, value);
        assert!(!id.playerless_host(&other));
    }
}

#[test]
fn pregame_screen_and_title_reset_guards_return_to_their_callers() {
    use crate::host_lobby::{keep_on_title_reset, sync_playerless_screen};
    use hooks::{Call, Flow};
    use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runtime, TransferAbi, guest::Permissions};

    const STACK: u32 = 0x0060_0000;
    let id = SquadId {
        addr: S,
        incarnation: 9,
        class: 1,
    };
    let mut g = Guest::new();
    for (addr, size) in [
        (STACK, 0x1000),
        (S, 0x8000),
        (lobby::SESSION & !0xfff, 0x1000),
        (bots::guest::SLOTS & !0xfff, 0x4000),
    ] {
        g.map_zeroed(addr, size, Permissions::RW).unwrap();
    }
    g.set_u8(lobby::NETWORKING, 1).unwrap();
    g.set_u32(lobby::SESSION, S).unwrap();
    g.set_u32(S + STATE, session::STATE_LIVE).unwrap();
    g.set_u32(S + INCARNATION, id.incarnation).unwrap();
    g.set_u32(S + SESSION_NETWORK_CLASS, id.class).unwrap();
    let rt = Runtime::new(g, Dispatch::default());
    let mut ctx = ExecCtx::new(rt.clone(), 1);

    for (point, cycle, caller, owner, popped) in [
        (&points::UI_SYNC_NETWORK_SCREEN, 0, 123, Some(id), 0),
        (&points::UI_SYNC_NETWORK_SCREEN, 1, 123, Some(id), 4),
        (&points::UI_SYNC_NETWORK_SCREEN, 2, 123, Some(id), 4),
        (&points::UI_SYNC_NETWORK_SCREEN, 3, 123, Some(id), 0),
        (&points::UI_SYNC_NETWORK_SCREEN, 1, 123, None, 0),
        (
            &points::UI_LEAVE_SESSIONS,
            1,
            0x0019_06d8,
            Some(id),
            8,
        ),
        (
            &points::UI_LEAVE_SESSIONS,
            1,
            0x0019_06d9,
            Some(id),
            0,
        ),
        (
            &points::UI_LEAVE_SESSIONS,
            1,
            0x0019_06d8,
            None,
            0,
        ),
    ] {
        rt.memory().set_u32(lobby::SESSION_TYPE, cycle).unwrap();
        rt.memory().set_u32(STACK, caller).unwrap();
        let mut cpu = CpuAbi {
            esp: STACK,
            ..Default::default()
        };
        let mut c = Call {
            cpu: &mut cpu,
            ctx: &mut ctx,
            addr: point.base,
            point,
        };
        let flow = if point.base == points::UI_LEAVE_SESSIONS.base {
            keep_on_title_reset(&mut c, owner)
        } else {
            sync_playerless_screen(&mut c, owner)
        }
        .unwrap();
        match flow {
            Flow::Continue => assert_eq!(popped, 0),
            Flow::Return(t) => {
                assert_ne!(popped, 0);
                assert_eq!(t, TransferAbi::ret(caller));
            }
        }
        assert_eq!(cpu.esp, STACK + popped);
    }
}

#[test]
fn a_sim_waits_for_its_full_roster_and_keeps_variant_settings() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let config = sim_bench::config::parse_match(
                "map='midship'\nscore_limit=17\n[[bot]]\n[[bot]]\n",
                false,
            )
            .unwrap();
            let matched = sim_bench::config::resolve(config, &|_| unreachable!()).unwrap();
            let mut lobby = Machine::default();
            for progress in [
                SimOpen::SquadCreated,
                SimOpen::SessionStarted,
                SimOpen::Wait("checking replicated lobby parameters"),
                SimOpen::Ready,
            ] {
                assert_eq!(
                    lobby
                        .advance_sim_open(&cx, MAP, &matched, SCRATCH)
                        .await
                        .map_err(rpc)?,
                    progress,
                );
            }
            assert_eq!(
                cx.read(|m| m.u32(S + VARIANT + variant::SCORE_TO_WIN as u32))?,
                17
            );
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(S + PEER_COUNT, 2)?;
                peer(m, 5, 1, "First")?;
                m.set_u32(S + PEERS + PEER_MAP_STATUS, 3)?;
                m.set_u32(S + PEERS + PEER_SIZE + PEER_MAP_STATUS, 4)
            })?;
            assert_eq!(
                lobby.start_sim(&cx, 2).await.map_err(rpc)?,
                SimStart::Wait("1 of 2 seats have a player".into())
            );
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(S + PEER_COUNT, 3)?;
                peer(m, 9, 2, "Second")?;
                m.set_u32(S + PEERS + 2 * PEER_SIZE + PEER_MAP_STATUS, 4)
            })?;
            assert_eq!(
                lobby.start_sim(&cx, 2).await.map_err(rpc)?,
                SimStart::Started
            );
            Ok(Ret::new(1, 0))
        })
    };
    let run = testing::run(&test, menu(), Regs::default(), reply);
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert!(
        run.calls
            .iter()
            .all(|c| c.target != points::PROFILE_SIGN_IN.base)
    );
    assert_eq!(
        run.calls
            .iter()
            .filter(|c| c.target == MACHINE_LOAD_REQUEST)
            .count(),
        1
    );
}

/// The playerless lobby the sim drives: no local user on any port, the
/// host as peer 0 with no player, and one phantom peer with one player per
/// seat.
fn eligible(seats: usize) -> Eligibility {
    Eligibility {
        phase: Phase::Lobby,
        open: true,
        leader: 0,
        local: 0,
        map: MAP,
        want_map: MAP,
        protocol: 2,
        want_protocol: 2,
        max: seats as u32,
        want_max: seats as u32,
        players: seats,
        peer_maps: vec![3; seats + 1],
        variant: true,
    }
}

#[test]
fn a_playerless_lobby_with_only_phantoms_is_eligible_and_says_what_holds_it() {
    for seats in [1, 4, 15] {
        let e = eligible(seats);
        assert_eq!(e.blocker(), None, "{seats} phantoms");
        assert_eq!(sim_blocker(e.blocker(), seats, seats), None);
    }
    let e = eligible(4);
    let why = |f: &dyn Fn(&mut Eligibility)| {
        let mut e = e.clone();
        f(&mut e);
        e.blocker().expect("a blocker")
    };
    assert!(why(&|e| e.phase = Phase::Loading).contains("Loading"));
    assert!(why(&|e| e.open = false).contains("not open"));
    assert!(why(&|e| e.leader = 1).contains("does not lead"));
    assert!(why(&|e| e.map = MAP + 1).contains("map"));
    assert!(why(&|e| e.protocol = 1).contains("protocol"));
    assert!(why(&|e| e.max = 2).contains("player limit is 2 (want 4)"));
    assert!(why(&|e| e.players = 0).contains("no remote player"));
    assert!(why(&|e| e.players = 5).contains("exceed"));
    assert!(why(&|e| e.variant = false).contains("variant"));
    // The countdown finisher's map test: every peer, this host's own too.
    assert!(why(&|e| e.peer_maps[0] = 0).starts_with("peer 0 has map status 0"));
    assert!(why(&|e| e.peer_maps[3] = 2).starts_with("peer 3 has map status 2"));
    assert!(why(&|e| e.peer_maps[2] = 1).starts_with("peer 2 has map status 1"));
    assert_eq!(
        Eligibility {
            peer_maps: vec![4, 3, 4, 4, 3],
            ..e.clone()
        }
        .blocker(),
        None
    );
    // The sim needs every seat filled; the lobby's reason comes first.
    assert_eq!(
        sim_blocker(None, 3, 4),
        Some("3 of 4 seats have a player".into())
    );
    assert_eq!(
        sim_blocker(Some("x".into()), 3, 4),
        Some("x".into())
    );
}

#[test]
fn an_all_phantom_sim_starts_with_zero_local_users_and_four_phantoms() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let config = sim_bench::config::parse_match(
                "map='lockout'\nteam_play=true\n\
                 [[bot]]\nkind='phantom'\nteam=0\n[[bot]]\nkind='phantom'\nteam=1\n\
                 [[bot]]\nkind='phantom'\nteam=0\n[[bot]]\nkind='phantom'\nteam=1\n",
                false,
            )
            .unwrap();
            let matched = sim_bench::config::resolve(config, &|_| unreachable!()).unwrap();
            let mut lobby = Machine::default();
            while !lobby
                .open_sim(&cx, MAP, &matched, SCRATCH)
                .await
                .map_err(rpc)?
            {}
            assert_eq!(cx.read(|m| m.u32(S + MAX_PLAYERS))?, 4);
            // The host's own peer is loaded; four phantom peers join, each
            // with one player and this host's properties.
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(S + PEER_COUNT, 5)?;
                m.set_u32(S + PEERS + PEER_MAP_STATUS, 2)?;
                for (i, name) in ["Red 1", "Blue 1", "Red 2", "Blue 2"].iter().enumerate() {
                    let p = i as u32 + 1;
                    peer(m, i as u32, p, name)?;
                    m.set_u32(S + PEERS + p * PEER_SIZE + PEER_MAP_STATUS, 2)?;
                }
                Ok(())
            })?;
            // Still loading the map: retail's start check would say 9 (a
            // leader's loading lobby), but the load request must wait as the
            // countdown finisher does.
            assert!(matches!(
                lobby.start_sim(&cx, 4).await.map_err(rpc)?,
                SimStart::Wait(why) if why.starts_with("peer 0 has map status 2")
            ));
            cx.write(|m| -> Result<(), MemFault> {
                for p in 0..5 {
                    m.set_u32(S + PEERS + p * PEER_SIZE + PEER_MAP_STATUS, 3)?;
                }
                Ok(())
            })?;
            assert_eq!(
                lobby.start_sim(&cx, 4).await.map_err(rpc)?,
                SimStart::Started
            );
            assert_eq!(cx.read(|m| m.u32(S + CLASS))?, STAGE_LOAD);
            Ok(Ret::new(1, 0))
        })
    };
    let run = testing::run(&test, menu(), Regs::default(), |f, m| {
        if f.target == SET_VARIANT.base {
            // Four seats: the limit open writes before the variant setter.
            assert_eq!(m.u32(SCRATCH + variant::MAX_PLAYERS as u32), Ok(4));
            let raw = m.bytes(SCRATCH, level_boot::VARIANT_SIZE as usize).unwrap();
            m.write(S + VARIANT, &raw).unwrap();
            return Regs {
                eax: 1,
                ..Default::default()
            };
        }
        reply(f, m)
    });
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert!(
        run.calls
            .iter()
            .all(|c| c.target != points::PROFILE_SIGN_IN.base),
        "no local seat is ever signed in"
    );
    let order: Vec<u32> = run
        .calls
        .iter()
        .map(|c| c.target)
        .filter(|t| {
            [
                points::LOBBY_START_STATE.base,
                CONTROLLER_COUNTDOWN,
                MACHINE_LOAD_REQUEST,
            ]
            .contains(t)
        })
        .collect();
    assert_eq!(
        order,
        [
            points::LOBBY_START_STATE.base,
            CONTROLLER_COUNTDOWN,
            MACHINE_LOAD_REQUEST
        ],
        "one start check, the refused controller countdown, then the load request"
    );
}

#[test]
fn a_refused_retail_start_check_is_a_wait_with_its_reason() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let config = sim_bench::config::parse_match(
                "map='lockout'\n[[bot]]\nkind='phantom'\n[[bot]]\nkind='phantom'\n",
                false,
            )
            .unwrap();
            let matched = sim_bench::config::resolve(config, &|_| unreachable!()).unwrap();
            let mut lobby = Machine::default();
            while !lobby
                .open_sim(&cx, MAP, &matched, SCRATCH)
                .await
                .map_err(rpc)?
            {}
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(S + PEER_COUNT, 3)?;
                peer(m, 0, 1, "A")?;
                peer(m, 1, 2, "B")?;
                for p in 0..3 {
                    m.set_u32(S + PEERS + p * PEER_SIZE + PEER_MAP_STATUS, 4)?;
                }
                Ok(())
            })?;
            assert!(matches!(
                lobby.start_sim(&cx, 2).await.map_err(rpc)?,
                SimStart::Wait(why) if why.starts_with("retail start state 7, blocked by the teams")
            ));
            Ok(Ret::new(1, 0))
        })
    };
    // The facts behind the reason read this console's own map status.
    let mut guest = menu();
    guest.map(lobby::MAP_FAILED_HERE, 4);
    let run = testing::run(&test, guest, Regs::default(), |f, m| {
        if f.target == points::LOBBY_START_STATE.base {
            return Regs {
                eax: 7,
                ..Default::default()
            };
        }
        reply(f, m)
    });
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert!(run.calls.iter().all(|c| c.target != MACHINE_LOAD_REQUEST));
}

#[test]
fn playerless_open_join_start_postgame_and_close_follow_retail() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let mut machine = Machine::default();
            let open = Command::Open(config());
            assert_eq!(
                machine.state(&cx).await.map_err(rpc)?,
                StateResult::default()
            );
            assert!(
                machine
                    .advance(&cx, &Command::Start, SCRATCH, None, true)
                    .await
                    .is_err()
            );
            let mut opened = false;
            for _ in 0..4 {
                if let Progress::Done(result) = machine
                    .advance(&cx, &open, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?
                {
                    assert_eq!(result, json!({}));
                    opened = true;
                    break;
                }
            }
            assert!(opened);
            let state = machine.state(&cx).await.map_err(rpc)?;
            assert_eq!(
                state,
                StateResult {
                    phase: Phase::Lobby,
                    ..Default::default()
                }
            );
            assert!(matches!(
                machine
                    .advance(&cx, &open, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            assert!(
                machine
                    .advance(
                        &cx,
                        &Command::Open(Config {
                            map: 17,
                            ..config()
                        }),
                        SCRATCH,
                        None,
                        true
                    )
                    .await
                    .is_err()
            );
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(S + PEER_COUNT, 2)?;
                peer(m, 5, 1, "Remote")?;
                peer(m, 9, 1, "Guest")?;
                m.set_u32(S + PEERS + PEER_MAP_STATUS, 3)
            })?;
            assert!(!machine.state(&cx).await.map_err(rpc)?.ready);
            cx.write(|m| m.set_u32(S + PEERS + PEER_SIZE + PEER_MAP_STATUS, 4))?;
            let s = machine.state(&cx).await.map_err(rpc)?;
            assert!(s.ready);
            assert_eq!(
                s.players
                    .iter()
                    .map(|p| (p.slot, p.peer, p.name.as_str()))
                    .collect::<Vec<_>>(),
                [(5, 1, "Remote"), (9, 1, "Guest")]
            );
            let user_index = bots::guest::slot(0) + bots::guest::SLOT_LOCAL_USER;
            cx.write(|m| m.set_u32(user_index, 0))?;
            assert!(
                machine
                    .advance(&cx, &Command::Start, SCRATCH, None, true)
                    .await
                    .is_err()
            );
            cx.write(|m| m.set_u32(user_index, u32::MAX))?;
            let refusal = machine
                .advance(&cx, &Command::Start, SCRATCH, None, false)
                .await;
            assert!(matches!(refusal, Err(e) if e.message.contains("profile")));
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Start, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            assert_eq!(machine.state(&cx).await.map_err(rpc)?.phase, Phase::Loading);
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(lobby::SESSION_TYPE, 3)?;
                m.set_u32(
                    GAME + telemetry::layout::OPTIONS,
                    level_boot::MODE_MULTIPLAYER,
                )?;
                m.set_u32(WORLD + SIM_MODE, session::WORLD_DIST_SERVER)
            })?;
            cx.write(|m| m.set_u32(WORLD + SIM_STATE, 3))?;
            assert_eq!(machine.state(&cx).await.map_err(rpc)?.phase, Phase::Loading);
            cx.write(|m| m.set_u32(WORLD + SIM_STATE, 5))?;
            assert_eq!(machine.state(&cx).await.map_err(rpc)?.phase, Phase::Playing);
            cx.write(|m| m.set_u8(GAME + telemetry::layout::GAME_ENDED, 1))?;
            assert_eq!(
                machine.state(&cx).await.map_err(rpc)?.phase,
                Phase::Postgame
            );
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Close, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Wait(_)
            ));
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Close, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            assert_eq!(
                machine.state(&cx).await.map_err(rpc)?,
                StateResult::default()
            );
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Close, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            let next = Command::Open(Config {
                map: MAP + 1,
                ..config()
            });
            for _ in 0..3 {
                assert!(matches!(
                    machine
                        .advance(&cx, &next, SCRATCH, None, true)
                        .await
                        .map_err(rpc)?,
                    Progress::Wait(_)
                ));
            }
            assert!(matches!(
                machine
                    .advance(&cx, &next, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            assert_eq!(
                machine.state(&cx).await.map_err(rpc)?,
                StateResult {
                    phase: Phase::Lobby,
                    ..Default::default()
                }
            );
            Ok(Ret::new(1, 0))
        })
    };
    let run = testing::run(&test, menu(), Regs::default(), reply);
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert_eq!(
        run.calls
            .iter()
            .filter(|c| c.target == points::LOBBY_SET_MAP.base)
            .count(),
        2
    );
    assert!(
        run.calls
            .iter()
            .all(|c| c.target != points::PROFILE_SIGN_IN.base)
    );
}

#[test]
fn a_local_seat_migration_or_broken_membership_refuses_start() {
    let owner = Owned {
        addr: S,
        incarnation: 0,
        config: config(),
        setup: Setup::Open,
        applied_variant: None,
        live_id: None,
        prior_live_id: None,
        closing: false,
    };
    let mut m = menu();
    m.put_u32(S + STATE, 5)
        .put_u32(S + PEER_COUNT, 2)
        .put_u32(S + SESSION_NETWORK_CLASS, 1);
    let s = Squad::read(&m, &owner).unwrap();
    peer(&mut m, 7, 0, "Local").unwrap();
    assert!(s.roster(&m).is_err());
    m.put_u32(S + PLAYERS + 7 * PLAYER_SIZE + P_PEER, 1);
    m.put_u32(S + PLAYER_MASK, 1 << 16);
    assert!(s.roster(&m).is_err());
    m.put_u32(S + HOST_PEER, 1);
    assert!(Squad::read(&m, &owner).is_err());
    m.put_u32(bots::guest::slot(3), bots::guest::SLOT_PROFILE);
    assert!(profiles(&m).is_err());
}

#[test]
fn stale_world_or_session_numbers_do_not_report_playing() {
    let mp = Some(level_boot::MODE_MULTIPLAYER);
    assert_eq!(phase(3, 4, false, mp, None, session::WORLD_DIST_SERVER, false).unwrap(), Phase::Loading);
    assert!(phase(3, 4, false, mp, Some(5), session::WORLD_DIST_SERVER, false).is_err());
    assert!(phase(7, 4, false, mp, Some(4), session::WORLD_DIST_SERVER, false).is_err());
    assert_eq!(
        phase(4, 5, false, None, None, session::WORLD_DIST_SERVER, false).unwrap(),
        Phase::Postgame
    );
}

#[test]
fn live_open_waits_for_a_new_advertisement_and_close_for_withdrawal() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let mut machine = Machine::default();
            let config = Config {
                live: true,
                ..config()
            };
            let open = Command::Open(config);
            let older = Some("0000000000000001");
            let ours = Some("0000000000000002");
            for _ in 0..4 {
                assert!(matches!(
                    machine
                        .advance(&cx, &open, SCRATCH, older, true)
                        .await
                        .map_err(rpc)?,
                    Progress::Wait(_)
                ));
            }
            assert!(matches!(
                machine
                    .advance(&cx, &open, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Wait(_)
            ));
            let result = machine
                .advance(&cx, &open, SCRATCH, ours, true)
                .await
                .map_err(rpc)?;
            assert!(
                matches!(result, Progress::Done(v) if v == json!({"session_id": "0000000000000002"}))
            );
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Close, SCRATCH, ours, true)
                    .await
                    .map_err(rpc)?,
                Progress::Wait(_)
            ));
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Close, SCRATCH, ours, true)
                    .await
                    .map_err(rpc)?,
                Progress::Wait(_)
            ));
            assert!(matches!(
                machine
                    .advance(&cx, &Command::Close, SCRATCH, older, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            Ok(Ret::new(1, 0))
        })
    };
    // The Live menu is allowed to allocate a fresh squad in Create.
    let offline = 0x8500_0000;
    let mut initial = menu();
    initial
        .map(offline, 0x7900)
        .put_u32(lobby::SESSION, offline);
    let run = testing::run(&test, initial, Regs::default(), |f, m| {
        if f.target == CREATE_SQUAD {
            m.set_u32(lobby::SESSION, S).unwrap();
        }
        reply(f, m)
    });
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert_eq!(run.calls[0].target, points::UI_LEAVE_SESSIONS.base);
    assert_eq!(run.calls[1].target, CREATE_SQUAD);
    assert_eq!(
        run.calls
            .iter()
            .filter(|c| c.target == LEAVE_SESSIONS)
            .count(),
        1
    );
}

#[test]
fn open_tracks_the_network_squad_when_create_replaces_the_menu_squad() {
    let mut memory = menu();
    let offline = 0x8500_0000;
    memory.map(offline, 0x7900).put_u32(lobby::SESSION, offline);
    let test = move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let mut machine = Machine::default();
            let open = Command::Open(config());
            for _ in 0..3 {
                assert!(matches!(
                    machine
                        .advance(&cx, &open, SCRATCH, None, true)
                        .await
                        .map_err(rpc)?,
                    Progress::Wait(_)
                ));
            }
            assert!(matches!(
                machine
                    .advance(&cx, &open, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Done(_)
            ));
            assert_eq!(cx.read(|m| m.u32(offline + MAX_PLAYERS))?, 0);
            assert_eq!(cx.read(|m| m.u32(S + MAX_PLAYERS))?, 2);
            Ok(Ret::new(1, 0))
        })
    };
    let run = testing::run(&test, memory, Regs::default(), |call, m| {
        if call.target == points::BROWSER_CREATE_SQUAD.base {
            m.set_u32(lobby::SESSION, S).unwrap();
        }
        reply(call, m)
    });
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
}

#[test]
fn synchronous_open_keeps_the_fresh_squad_variant_free_and_can_request_start() {
    let test = |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let mut machine = Machine::default();
            let config = Config { variant: Variant::Absent, ..config() };
            for expected in [
                SimOpen::SquadCreated,
                SimOpen::SessionStarted,
                SimOpen::Wait("checking replicated lobby parameters"),
                SimOpen::Ready,
            ] {
                assert_eq!(machine.open(&cx, &config, SCRATCH, None, &[]).await.map_err(rpc)?, expected);
            }
            cx.write(|m| -> Result<(), MemFault> {
                m.set_u32(S + PEER_COUNT, 2)?;
                m.set_u32(S + PEERS + PEER_MAP_STATUS, 3)?;
                m.set_u32(S + PEERS + PEER_SIZE + PEER_MAP_STATUS, 3)?;
                peer(m, 1, 1, "FixtureClient")
            })?;
            assert!(machine.state(&cx).await.map_err(rpc)?.ready);
            assert!(matches!(machine.start(&cx, true).await.map_err(rpc)?, Progress::Done(_)));
            Ok(Ret::new(1, 0))
        })
    };
    let run = testing::run(&test, menu(), Regs::default(), |call, m| {
        let result = reply(call, m);
        if call.target == points::BROWSER_CREATE_SQUAD.base {
            m.set_u32(S + PROTOCOL, 1).unwrap();
            m.set_u16(S + coop_probe::layout::DIFFICULTY, coop_probe::NO_DIFFICULTY).unwrap();
        }
        result
    });
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert!(!run.calls.iter().any(|call| call.target == SET_VARIANT.base));
    assert!(run.calls.iter().any(|call| call.target == MACHINE_LOAD_REQUEST));
    assert_eq!(
        phase(3, 4, false, Some(level_boot::MODE_MULTIPLAYER), Some(WORLD_SYNC_SERVER),
              WORLD_SYNC_SERVER, false).unwrap(),
        Phase::Playing
    );
}
