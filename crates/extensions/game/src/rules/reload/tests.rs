//! The reload decision, its refusals and the restart's options block, as
//! pure functions.

use super::*;

const SLAYER: i32 = 2;

fn slayer() -> Ir {
    ::rules::compile(::rules::builtin::SLAYER).unwrap()
}

fn edited(from: &str, to: &str) -> Ir {
    let src = ::rules::builtin::SLAYER.replacen(from, to, 1);
    assert_ne!(src, ::rules::builtin::SLAYER, "the edit {from:?} did not apply");
    ::rules::compile(&src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

/// Slayer with a player variable: another state shape.
fn with_streak() -> Ir {
    let src = format!("{}\nplayer var streak: int = 0\n", ::rules::builtin::SLAYER);
    ::rules::compile(&src).unwrap()
}

fn host(seat: Seat) -> Situation {
    Situation {
        seat,
        multiplayer: true,
        matchmade: false,
        playback: false,
        engine: Some(SLAYER),
        restart: None,
    }
}

#[test]
fn custom_system_link_host_and_local_games_take_edits() {
    for seat in [Seat::Alone, Seat::SystemLinkHost, Seat::LiveHost] {
        assert_eq!(refusal(&host(seat)), None, "{seat:?}");
    }
}

#[test]
fn theater_matchmade_guests_and_no_game_are_refused() {
    let cases = [
        (Situation { playback: true, ..host(Seat::Alone) }, "Theater"),
        (Situation { matchmade: true, ..host(Seat::LiveHost) }, "matchmade"),
        (host(Seat::SystemLinkGuest), "guest"),
        (host(Seat::LiveGuest), "guest"),
        (Situation { multiplayer: false, ..host(Seat::Alone) }, "no multiplayer game"),
    ];
    for (s, want) in cases {
        let why = refusal(&s).unwrap_or_else(|| panic!("{s:?} is refused"));
        assert!(why.contains(want), "{want}: {why}");
        match decide(&s, Some(&slayer()), &slayer(), ReloadMode::Auto) {
            Decision::Refuse(d) => assert_eq!(d, why),
            other => panic!("{s:?}: {other:?}"),
        }
    }
}

#[test]
fn the_same_shape_goes_in_place() {
    let s = host(Seat::Alone);
    assert_eq!(decide(&s, Some(&slayer()), &slayer(), ReloadMode::Auto), Decision::InPlace);
    let handler = edited("score(killer, kill_points)", "score(killer, kill_points * 2)");
    assert_eq!(decide(&s, Some(&slayer()), &handler, ReloadMode::Auto), Decision::InPlace);
    assert_eq!(decide(&s, Some(&slayer()), &handler, ReloadMode::InPlace), Decision::InPlace);
    // A Live custom game's host swaps in place too.
    assert_eq!(
        decide(&host(Seat::LiveHost), Some(&slayer()), &handler, ReloadMode::Auto),
        Decision::InPlace
    );
}

#[test]
fn another_shape_restarts_with_its_reasons() {
    let s = host(Seat::SystemLinkHost);
    match decide(&s, Some(&slayer()), &with_streak(), ReloadMode::Auto) {
        Decision::Restart { reasons } => {
            assert_eq!(reasons, ["the script has 1 player variables, not 0"]);
        }
        other => panic!("{other:?}"),
    }
    // Asked for: a restart even for the same shape, with no reasons.
    assert_eq!(
        decide(&s, Some(&slayer()), &slayer(), ReloadMode::Restart),
        Decision::Restart { reasons: vec![] }
    );
    // No script runs yet: the restart starts one.
    assert_eq!(
        decide(&s, None, &slayer(), ReloadMode::Auto),
        Decision::Restart { reasons: vec!["no script runs in this game".into()] }
    );
}

#[test]
fn in_place_only_refuses_another_shape() {
    match decide(&host(Seat::Alone), Some(&slayer()), &with_streak(), ReloadMode::InPlace) {
        Decision::Refuse(why) => assert!(why.starts_with("not in place: "), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_restart_this_game_cannot_make_is_refused_with_both_reasons() {
    let cases = [
        (host(Seat::LiveHost), "Live game restarts from its lobby"),
        (
            Situation { restart: Some("the agent-calls feature isn't installed"), ..host(Seat::Alone) },
            "agent-calls",
        ),
        (Situation { engine: Some(3), ..host(Seat::Alone) }, "the variant is engine 3"),
        (Situation { engine: None, ..host(Seat::Alone) }, "could not be read"),
    ];
    for (s, want) in cases {
        match decide(&s, Some(&slayer()), &with_streak(), ReloadMode::Auto) {
            Decision::Refuse(why) => {
                assert!(why.contains(want), "{want}: {why}");
                assert!(why.contains("player variables"), "the shape reason too: {why}");
            }
            other => panic!("{s:?}: {other:?}"),
        }
    }
}

#[test]
fn a_script_this_host_cannot_run_is_refused_before_anything() {
    let src = format!("{}\non round_start {{\n}}\n", ::rules::builtin::SLAYER);
    let ir = ::rules::compile(&src).unwrap();
    match decide(&host(Seat::Alone), Some(&slayer()), &ir, ReloadMode::Auto) {
        Decision::Refuse(why) => assert!(why.starts_with("this host cannot run the script"), "{why}"),
        other => panic!("{other:?}"),
    }
}

/// An options block whose record carries `block`; every other byte is
/// its offset's low byte, so a stray write shows.
fn options(block: [u8; ext::LEN]) -> Vec<u8> {
    let mut o: Vec<u8> = (0..agent::sim::calls::OPTIONS_SIZE as usize).map(|i| i as u8).collect();
    o[BLOCK_AT..BLOCK_AT + ext::LEN].copy_from_slice(&block);
    o
}

fn record(prefix: [u8; 8], bits: u32) -> [u8; ext::LEN] {
    Ext {
        gravity: 50,
        script: Script { mode: Mode::Rules, prefix, options: bits },
        ..Ext::default()
    }
    .encode()
}

#[test]
fn a_restart_names_the_new_blob_and_keeps_the_rest_of_the_options() {
    let old = slayer();
    let new = with_streak();
    let hash = new.id();
    let before = options(record([1; 8], 2));
    let (after, block) = restart_options(&before, Some(&old), &new, &hash).unwrap();
    assert_eq!(after.len(), before.len());
    assert_eq!(after[..BLOCK_AT], before[..BLOCK_AT]);
    assert_eq!(after[BLOCK_AT + ext::LEN..], before[BLOCK_AT + ext::LEN..]);
    assert_eq!(after[BLOCK_AT..BLOCK_AT + ext::LEN], block);
    let e = Ext::in_effect(&block);
    assert_eq!(e.script.mode, Mode::Rules);
    assert_eq!(e.script.prefix[..], hash.0[..8]);
    assert_eq!(e.script.options, 2, "the options read the same: the record's bits");
    assert_eq!(e.gravity, 50, "the other extras are kept");

    // Changed options: the new blob's defaults.
    let other = edited("int 1..5 = 1", "int 1..9 = 4");
    let (_, block) = restart_options(&before, Some(&old), &other, &other.id()).unwrap();
    assert_eq!(Ext::in_effect(&block).script.options, other.default_options());
    // No script ran: the defaults too.
    let (_, block) = restart_options(&before, None, &new, &hash).unwrap();
    assert_eq!(Ext::in_effect(&block).script.options, new.default_options());

    assert!(restart_options(&before[..BLOCK_AT], Some(&old), &new, &hash).is_err());
}

#[test]
fn a_restart_writes_the_options_then_queues_them() {
    let steps = restart_steps(vec![1, 2, 3]);
    assert_eq!(
        steps,
        [
            GameStep::Write { offset: 0, bytes: vec![1, 2, 3] },
            GameStep::Call {
                target: crate::level_boot::OPTIONS_QUEUE,
                args: vec![],
                eax: Some(Arg::Options(0)),
                esi: None,
                report: None,
            },
        ]
    );
}

#[test]
fn params_give_a_blob_or_source() {
    let ir = slayer();
    let b64 = variants::portable::base64_encode(&ir.encode());
    let p = ReloadParams { ir_b64: Some(b64), ..Default::default() };
    assert_eq!(blob(&p).unwrap().0.id(), ir.id());
    let p = ReloadParams { source: Some(::rules::builtin::SLAYER.into()), ..Default::default() };
    assert_eq!(blob(&p).unwrap().0.id(), ir.id());
    let p = ReloadParams { ir_b64: Some("not base64!".into()), ..Default::default() };
    assert_eq!(blob(&p).unwrap_err().code, code::INVALID_PARAMS);
    let p = ReloadParams { source: Some("rules \"x\" 1\ncarrier nowhere\n".into()), ..Default::default() };
    let e = blob(&p).unwrap_err();
    assert_eq!(e.code, agent_proto::sim::REFUSED);
    assert!(e.message.contains("does not build"), "{}", e.message);
}
