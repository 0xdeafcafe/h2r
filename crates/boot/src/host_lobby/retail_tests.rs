use super::*;
use engine::entry::{EntryError, EntryFuture, Regs, Ret, testing};
use engine::testing::FakeGuest;

const S: u32 = 0x8100_0000;
const GAME: u32 = 0x8200_0000;
const SCRATCH: u32 = 0x8300_0000;
const WORLD: u32 = 0x8400_0000;
const MAP: u32 = 50;

fn menu() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(S, 0x7900)
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
        variant: Variant::Stock(sim::config::GameType::Slayer),
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
        LIVE_MENU_PREPARE => {
            assert_eq!(f.args, [1]);
            0
        }
        CREATE_BROWSER_SQUAD | CREATE_SQUAD => {
            let class = if f.target == CREATE_BROWSER_SQUAD {
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
        SET_VARIANT => {
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
        0x5a220 => {
            assert_eq!(f.args, [2]);
            m.set_u32(S + CLASS, 2).unwrap();
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
            assert!(matches!(
                machine
                    .advance(&cx, &next, SCRATCH, None, true)
                    .await
                    .map_err(rpc)?,
                Progress::Wait(_)
            ));
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
    assert_eq!(phase(3, 4, false, mp, None, false).unwrap(), Phase::Loading);
    assert!(phase(3, 4, false, mp, Some(5), false).is_err());
    assert!(phase(7, 4, false, mp, Some(4), false).is_err());
    assert_eq!(
        phase(4, 5, false, None, None, false).unwrap(),
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
    let run = testing::run(&test, menu(), Regs::default(), reply);
    assert_eq!(run.result.unwrap(), Ret::new(1, 0));
    assert_eq!(run.calls[0].target, LIVE_MENU_PREPARE);
    assert_eq!(run.calls[1].target, CREATE_SQUAD);
    assert_eq!(
        run.calls
            .iter()
            .filter(|c| c.target == LEAVE_SESSIONS)
            .count(),
        1
    );
}
