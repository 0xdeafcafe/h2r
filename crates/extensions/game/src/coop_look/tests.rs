use super::*;
use guest_layout::datum::layout as array;
use guest_layout::testing::FakeGuest;
use live_telemetry::layout;
use serde_json::json;
use std::f32::consts::TAU;

fn aim(yaw: f32, pitch: f32) -> Aim {
    Aim { yaw, pitch }
}

fn close(a: f32, b: f32) {
    assert!((a - b).abs() < 0.00001, "{a} vs {b}");
}

#[test]
fn acknowledgements_require_a_later_update_and_the_local_wire_aim() {
    let mut p = Prediction::default();
    p.apply(19, Some(aim(0.0, 0.0)));
    p.sent(30, 20, aim(0.1, 0.2), true);
    p.sent(31, 20, aim(0.13, 0.2), true);
    p.sent(32, 20, aim(0.16, 0.22), true);
    p.apply(20, Some(aim(0.1, 0.2))); // already on the guest when sent
    p.apply(21, Some(aim(0.1, 0.25))); // same yaw, different pitch
    assert_eq!(p.pending(), 3);
    p.apply(22, Some(aim(0.1302, 0.1999)));
    assert_eq!((p.pending(), p.ack_seq, p.dropped), (1, Some(31), 1));
    assert_eq!(p.target(), Some(aim(0.16, 0.22)));
    p.apply(21, Some(aim(0.16, 0.22)));
    assert_eq!(p.pending(), 1);
    p.apply(23, Some(aim(0.16, 0.22)));
    assert_eq!(p.pending(), 0);
    assert_eq!(p.target(), p.confirmed);
}

#[test]
fn update_and_send_numbers_wrap_and_retransmissions_do_not_add_turns() {
    let mut p = Prediction::default();
    p.apply(u32::MAX, Some(aim(6.2, 0.0)));
    p.sent(u32::MAX, u32::MAX, aim(TAU - 0.0001, 0.03), true);
    p.sent(0, u32::MAX, aim(0.08, 0.05), true);
    p.sent(0, u32::MAX, aim(0.08, 0.05), true);
    p.apply(0, Some(aim(0.0, 0.03)));
    assert_eq!(p.ack_seq, Some(u32::MAX));
    assert_eq!(p.pending(), 1);
    let (offset, _) = p.draw(aim(0.0, 0.03)).unwrap();
    close(offset.yaw, 0.08);
    close(offset.pitch, 0.02);
    p.apply(1, Some(aim(0.08, 0.05)));
    assert_eq!(p.pending(), 0);
}

#[test]
fn absolute_look_is_not_integrated_twice_and_native_look_is_preserved() {
    let confirmed = aim(TAU - 0.04, -0.2);
    let mut p = Prediction::default();
    p.apply(10, Some(confirmed));
    p.sent(0, 10, aim(0.01, -0.18), true);
    p.sent(1, 10, aim(0.05, -0.16), true);
    let (d, _) = p.draw(confirmed).unwrap();
    close(d.yaw, 0.09);
    close(d.pitch, 0.04);
    let (d, _) = p.draw(aim(0.01, -0.18)).unwrap();
    close(d.yaw, 0.04);
    close(d.pitch, 0.02);
    let (d, _) = p.draw(aim(0.05, -0.16)).unwrap();
    close(d.yaw, 0.0);
    close(d.pitch, 0.0);
}

#[test]
fn reversing_before_confirmation_replaces_the_first_turn() {
    let mut p = Prediction::default();
    p.apply(2, Some(aim(1.0, 0.0)));
    p.sent(3, 2, aim(1.12, 0.0), true);
    p.sent(4, 2, aim(0.95, 0.0), true);
    let camera = aim(1.10, 0.0);
    let (offset, _) = p.draw(camera).unwrap();
    close(camera.plus(offset).yaw, 0.95);
}

#[test]
fn a_matching_update_that_precedes_the_camera_hands_off_in_two_frames() {
    let mut p = Prediction::default();
    let turn = aim(0.1, 0.05);
    p.apply(1, Some(Aim::default()));
    p.sent(7, 1, turn, true);
    assert_eq!(p.draw(Aim::default()).unwrap().0, turn);
    p.apply(2, Some(turn));
    assert_eq!(p.pending(), 0);
    let (offset, correction) = p.draw(aim(0.06, 0.05)).unwrap();
    close(offset.yaw, 0.02);
    close(correction, 0.04);
    assert_eq!(p.draw(turn), Some((Aim::default(), 0.0)));
}

#[test]
fn expired_look_corrects_halfway_then_finishes_and_large_errors_snap() {
    for (yaw, expected) in [(0.12, 0.06), (1.0, 0.0)] {
        let mut p = Prediction::default();
        let zero = Aim::default();
        p.apply(1, Some(zero));
        p.sent(0, 1, aim(yaw, 0.0), true);
        p.apply(32, None);
        assert_eq!((p.pending(), p.dropped), (0, 1));
        let (offset, correction) = p.draw(zero).unwrap();
        close(offset.yaw, expected);
        close(correction, yaw);
        assert_eq!(p.draw(zero), Some((zero, 0.0)));
    }
}

const WORLD: u32 = 0x810000;
const PG: u32 = 0x820000;
const PH: u32 = 0x830000;
const PS: u32 = 0x840000;
const OH: u32 = 0x850000;
const OS: u32 = 0x860000;
const BODY: u32 = 0x870000;
const CONTROL: u32 = 0x880000;
const CINE: u32 = 0x890000;
const USER: usize = 1; // guest's second local pad, player slot 3 on the host
const PLAYER: u32 = 0x20003;
const UNIT: u32 = 0x30005;
const PACKET: u32 = 0x900000;
const UPDATE: u32 = 0x910000;
const ORIGINAL: u32 = 0x920000;
const PRIVATE: u32 = 0x930000;
const ARG: u32 = 0x940000;
const PC: u32 = CONTROL + crate::touch_glue::PC_HEADER + crate::touch_glue::PC_STRIDE;
const P: u32 = PS + 3 * layout::PLAYER_SIZE;

fn fixture() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.put_u32(points::SIM_GLOBALS.base, WORLD)
        .put_u32(WORLD + 8, 3)
        .put_u32(crate::coop_jip::at::PLAYERS_GLOBALS, PG)
        .put_u32(PG + 16, PLAYER)
        .put_u32(layout::PLAYERS_PTR, PH)
        .put_u32(PH + array::DATA, PS)
        .put_u32(PH + array::ELEMENT_SIZE, layout::PLAYER_SIZE)
        .put_u32(PH + array::HIGH_WATER, 4)
        .put_u16(P, 2)
        .put_u16(P + layout::PLAYER_LOCAL_USER, USER as u16)
        .put_u32(P + layout::PLAYER_UNIT, UNIT)
        .put_u32(layout::OBJECTS_PTR, OH)
        .put_u32(OH + array::DATA, OS)
        .put_u32(OH + array::ELEMENT_SIZE, layout::OBJECT_HEADER_SIZE)
        .put_u32(OH + array::HIGH_WATER, 6)
        .put_u16(OS + 5 * 12, 3)
        .put_u32(OS + 5 * 12 + layout::OBJECT_BODY, BODY)
        .put_u32(BODY + layout::UNIT_PLAYER, PLAYER)
        .put_u32(BODY + layout::BODY_PARENT, u32::MAX)
        .put_u32(crate::touch_glue::PLAYER_CONTROL, CONTROL)
        .put_u32(PC, UNIT)
        .put_u32(layout::CINEMATIC_GLOBALS_PTR, CINE)
        .put_u8(CINE + 5, 0)
        .put_u32(0x004e_8c50 + 0x140, view::FIRST_PERSON)
        .put_u32(ORIGINAL + 8, USER as u32)
        .put_u32(ARG, ORIGINAL)
        .put_u32(engine::render_loop::occlusion::guest::FRAME, 17)
        .map(PRIVATE, 4 * view::STRIDE);
    for camera in [ORIGINAL + 12, ORIGINAL + 0x80] {
        m.put_f32(camera + 12, 1.0).put_f32(camera + 32, 1.0);
    }
    m
}

fn submit_turn(s: &mut State, m: &mut FakeGuest) {
    let action = PACKET + 16 + ACTION_BYTES;
    m.put_u32(PACKET, 7)
        .put_u32(PACKET + 4, 5)
        .put_u32(PACKET + 12, 2)
        .put_f32(action + 4, 0.19)
        .put_f32(action + 8, -0.04)
        .put_u32(UPDATE, 5)
        .put_u32(UPDATE + 8, 1 << 3)
        .put_f32(UPDATE + 12 + 3 * ACTION_BYTES + 4, 0.0);
    s.applied(m, UPDATE).unwrap();
    s.sent(m, PACKET, true).unwrap();
}

#[test]
fn action_stream_preserves_retail_angles_through_host_wire_quantization() {
    // Post-player-control samples, before encoding. Use different axis
    // speeds, a held turn, a reversal and very small zoomed motion. The
    // network fixture quantizes independently of Prediction::same_on_wire.
    let sequence = [
        aim(6.20, 0.12),
        aim(6.25, 0.13),
        aim(0.04, 0.15),
        aim(0.14, 0.18),
        aim(0.24, 0.21),
        aim(0.22, 0.20),
        aim(0.215, 0.201),
        aim(0.214, 0.201),
    ];
    let mut m = fixture();
    let mut s = State::default();
    let action = PACKET + 16 + ACTION_BYTES;
    let echoed = UPDATE + 12 + 3 * ACTION_BYTES;
    for (i, expected) in sequence.into_iter().enumerate() {
        let seq = i as u32;
        m.put_u32(PACKET, seq)
            .put_u32(PACKET + 4, seq)
            .put_u32(PACKET + 12, 2)
            .put_f32(action + 4, expected.yaw)
            .put_f32(action + 8, expected.pitch);
        s.sent(&m, PACKET, true).unwrap();
        assert_eq!(s.users[USER].prediction.target(), Some(expected));
        let wire = |r: f32, max: f32| (r * (max / TAU)).round_ties_even() * (TAU / max);
        let yaw = wire(expected.yaw, 8191.0);
        let pitch = wire(expected.pitch + std::f32::consts::PI, 4095.0) - std::f32::consts::PI;
        m.put_u32(UPDATE, seq + 1)
            .put_u32(UPDATE + 8, 1 << 3)
            .put_f32(echoed + 4, yaw)
            .put_f32(echoed + 8, pitch);
        s.applied(&m, UPDATE).unwrap();
        let p = &s.users[USER].prediction;
        assert_eq!((p.pending(), p.ack_seq), (0, Some(seq)));
        assert_eq!(p.target(), Some(aim(yaw, pitch)));
    }
}

#[test]
fn a_guest_draws_both_private_cameras_and_the_api_reports_the_rendered_frame() {
    let mut m = fixture();
    let mut state = State::default();
    submit_turn(&mut state, &mut m);
    let view_before = m.bytes(ORIGINAL, view::STRIDE as usize).unwrap();
    let control_before = m.bytes(PC, 0x94).unwrap();
    let body_before = m.bytes(BODY, 0x244).unwrap();
    state.render(&mut m, ARG, PRIVATE, true).unwrap();
    let private = PRIVATE + view::STRIDE;
    assert_eq!(m.u32(ARG).unwrap(), private);
    for camera in [private + 12, private + 0x80] {
        let f = m.vec3(camera + 12).unwrap();
        let up = m.vec3(camera + 24).unwrap();
        let drawn = view::facing(f).unwrap();
        close(drawn.yaw, 0.19);
        close(drawn.pitch, -0.04);
        close(f.into_iter().zip(up).map(|(a, b)| a * b).sum(), 0.0);
    }
    assert_eq!(
        view_before,
        m.bytes(ORIGINAL, view::STRIDE as usize).unwrap()
    );
    assert_eq!(control_before, m.bytes(PC, 0x94).unwrap());
    assert_eq!(body_before, m.bytes(BODY, 0x244).unwrap());
    let reply = state.read(json!({})).unwrap();
    assert_eq!(reply["frames"][0]["status"], "active");
    assert_eq!(reply["frames"][0]["frame"], 17);
    assert_eq!(reply["frames"][0]["pending"], 1);
    assert_eq!(reply["frames"][0]["confirmed"]["yaw"], 0.0);
    assert_eq!(state.read(json!({"after":1})).unwrap()["frames"], json!([]));
    assert!(state.read(json!({"unknown":true})).is_err());
}

#[test]
fn host_death_cutscene_vehicle_spectator_and_disabled_setting_use_the_original_view() {
    for why in [
        "host",
        "dead",
        "cinematic",
        "vehicle",
        "director",
        "no local control",
        "setting off",
    ] {
        let mut m = fixture();
        let mut s = State::default();
        submit_turn(&mut s, &mut m);
        match why {
            "host" => {
                m.put_u32(WORLD + 8, 2);
            }
            "dead" => {
                m.put_u16(BODY + layout::BODY_DAMAGE_FLAGS, layout::BODY_DEPLETED);
            }
            "cinematic" => {
                m.put_u8(CINE + 5, 1);
            }
            "vehicle" => {
                m.put_u32(BODY + layout::BODY_PARENT, 123);
            }
            "director" => {
                m.put_u32(0x004e_8c50 + 0x140, 0x16c840);
            }
            "no local control" => {
                m.put_u32(BODY + layout::UNIT_PLAYER, PLAYER + 1);
            }
            _ => {}
        }
        s.render(&mut m, ARG, PRIVATE, why != "setting off")
            .unwrap();
        assert_eq!(m.u32(ARG).unwrap(), ORIGINAL, "{why}");
        assert_eq!(s.users[USER].prediction.pending(), 0, "{why}");
        assert_eq!(s.history.back().unwrap().status, why);
    }
}

#[test]
fn zoom_teleport_and_replacement_unit_discard_the_previous_turn() {
    for cut in ["zoom", "teleport", "respawn"] {
        let mut m = fixture();
        let mut s = State::default();
        submit_turn(&mut s, &mut m);
        match cut {
            "zoom" => {
                m.put_u16(PC + 0x2e, 1);
            }
            "teleport" => {
                m.put_f32(BODY + layout::BODY_POSITION, 3.0);
            }
            _ => {
                m.put_u16(OS + 5 * 12, 4)
                    .put_u32(P + layout::PLAYER_UNIT, 0x40005)
                    .put_u32(PC, 0x40005);
            }
        }
        s.render(&mut m, ARG, PRIVATE, true).unwrap();
        assert_eq!(m.u32(ARG).unwrap(), ORIGINAL);
        assert_eq!(s.users[USER].prediction.pending(), 0, "{cut}");
    }
}

#[test]
fn settings_and_registered_agent_method_are_available_in_live_coop() {
    assert_eq!(COOP_LOOK_PREDICTION.default, settings::Value::Bool(true));
    assert_eq!(
        COOP_LOOK_PREDICTION.parse_env_text("0"),
        Ok(settings::Value::Bool(false))
    );
    let a = agent::Agent::new();
    let mut registry = Registry::new(hooks::Switches::defaults());
    install(&mut registry, Arm::session(), None, Some(&a)).unwrap();
    let plan = registry.build(&hooks::Base).unwrap().text();
    for point in [
        &points::SIM_ACTIONS_ENCODE,
        &points::SIM_UPDATE_APPLY,
        &points::RENDER_GAME_VIEW,
    ] {
        assert!(plan.contains(point.name));
    }
    struct Reader;
    impl agent::Reader for Reader {
        fn status(&mut self) -> agent::GameStatus {
            Default::default()
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(Vec::new())
        }
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let conn = a.connect(tx);
    a.submit(
        conn,
        &json!({"jsonrpc":"2.0","id":1,"method":method::COOP_LOOK,"params":{}}).to_string(),
    );
    a.between_slices(&mut Reader, 0);
    let line = rx.try_recv().unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    // Host methods also carry the agent's `at` stamp (theater films).
    assert_eq!(reply["result"]["latest"], json!(0));
    assert_eq!(reply["result"]["frames"], json!([]));
}

/// Replay the on-foot portion of a real stick capture without the game.
/// The two inputs come from input.record and render.coop_look (15.5).
#[test]
#[ignore = "requires the coordinator's recorded stick sweep and guest look capture"]
fn recorded_stick_turns_keep_the_retail_action_and_match_host_angles() {
    use std::collections::{BTreeMap, BTreeSet};
    fn recording(var: &str) -> String {
        std::fs::read_to_string(std::env::var(var).expect(var)).expect("read recording")
    }
    let pads = xapi::tap::InputLog::parse(&recording("H2_COOP_LOOK_INPUT")).unwrap();
    let sticks: BTreeSet<_> = pads
        .reads
        .iter()
        .filter(|r| r.status == 0)
        .map(|r| {
            (
                i16::from_le_bytes([r.packet[18], r.packet[19]]),
                i16::from_le_bytes([r.packet[20], r.packet[21]]),
            )
        })
        .collect();
    assert!(
        sticks.len() >= 3
            && sticks.iter().any(|(x, _)| *x < 0)
            && sticks.iter().any(|(x, _)| *x > 0)
            && sticks.iter().any(|(_, y)| *y != 0),
        "record turns in both directions and vertical look"
    );

    #[derive(Deserialize)]
    struct Sample {
        user: usize,
        status: String,
        pending: usize,
        last_send: Option<u32>,
        sent: Option<Aim>,
        predicted: Option<Aim>,
        update: Option<u32>,
        matched_update: Option<u32>,
        matched_send: Option<u32>,
        confirmed: Option<Aim>,
    }
    #[derive(Deserialize)]
    struct Capture {
        frames: Vec<Sample>,
    }
    let log: Capture = serde_json::from_str(&recording("H2_COOP_LOOK_CAPTURE")).unwrap();
    let mut sent = BTreeMap::new();
    let mut echoes = BTreeSet::new();
    let mut moving = BTreeSet::new();
    for frame in log
        .frames
        .into_iter()
        .filter(|f| f.user == 0 && f.status == "active")
    {
        if let (Some(seq), Some(raw)) = (frame.last_send, frame.sent) {
            sent.insert(seq, raw);
            if frame.pending > 0 {
                assert_eq!(
                    frame.predicted,
                    Some(raw),
                    "changed the output of retail player control"
                );
                moving.insert((raw.yaw.to_bits(), raw.pitch.to_bits()));
            }
        }
        if frame.update == frame.matched_update
            && let (Some(seq), Some(real)) = (frame.matched_send, frame.confirmed)
            && let Some(raw) = sent.get(&seq)
        {
            let difference = raw.difference(real);
            assert!(
                difference.yaw.abs() <= TAU / 8191.0 && difference.pitch.abs() <= TAU / 4095.0,
                "sequence {seq}: {raw:?} vs host {real:?}"
            );
            echoes.insert(seq);
        }
    }
    assert!(
        moving.len() >= 3 && echoes.len() >= 3,
        "capture an active turn and its echoes on Bravo"
    );
}

fn second_turn(s: &mut State, m: &mut FakeGuest) {
    let action = PACKET + 16 + ACTION_BYTES;
    let echoed = UPDATE + 12 + 3 * ACTION_BYTES;
    m.put_u32(PACKET, 8)
        .put_u32(PACKET + 4, 6)
        .put_u32(PACKET + 12, 2)
        .put_f32(action + 4, 0.25)
        .put_f32(action + 8, -0.02)
        .put_u32(UPDATE, 6)
        .put_f32(echoed + 4, 0.19)
        .put_f32(echoed + 8, -0.04);
    s.applied(m, UPDATE).unwrap();
    s.sent(m, PACKET, true).unwrap();
}

fn look() -> Look {
    Look {
        state: Arc::default(),
        shared: Arc::default(),
        arm: Arm::session(),
        setting: None,
        default: true,
    }
}

/// The game thread's observers and the render hook, in turns, give the
/// same private cameras, users and agent frames as the one state did.
#[test]
fn the_published_copy_draws_and_logs_as_the_single_state_did() {
    let (mut old_guest, mut new_guest) = (fixture(), fixture());
    let mut old = State::default();
    let look = look();
    let camera = |m: &FakeGuest| (m.u32(ARG).unwrap(), m.bytes(PRIVATE, 4 * view::STRIDE as usize).unwrap());
    submit_turn(&mut old, &mut old_guest);
    submit_turn(&mut look.state(), &mut new_guest);
    // Two views in one frame, with no game-thread work between them.
    for _ in 0..2 {
        old.render(&mut old_guest, ARG, PRIVATE, true).unwrap();
        look.render(&mut new_guest, ARG, PRIVATE);
        assert_eq!(camera(&old_guest), camera(&new_guest));
        old_guest.put_u32(ARG, ORIGINAL);
        new_guest.put_u32(ARG, ORIGINAL);
    }
    // The next tick, then another view.
    second_turn(&mut old, &mut old_guest);
    second_turn(&mut look.state(), &mut new_guest);
    old.render(&mut old_guest, ARG, PRIVATE, true).unwrap();
    look.render(&mut new_guest, ARG, PRIVATE);
    assert_eq!(camera(&old_guest), camera(&new_guest));
    let new = look.state();
    assert_eq!(
        serde_json::to_string(&old.users).unwrap(),
        serde_json::to_string(&new.users).unwrap()
    );
    assert_eq!(old.read(json!({})).unwrap(), new.read(json!({})).unwrap());
    assert_eq!(new.history.len(), 3);
}

/// The render hook does not touch the game thread's state.
#[test]
fn a_render_call_changes_the_owner_only_when_the_game_thread_next_looks() {
    let mut m = fixture();
    let look = look();
    submit_turn(&mut look.state(), &mut m);
    look.render(&mut m, ARG, PRIVATE);
    assert!(lock(&look.state).history.is_empty());
    assert_eq!(look.state().history.len(), 1);
}

#[test]
fn a_look_prediction_render_hook_runs_on_the_render_thread() {
    let mut registry = Registry::new(hooks::Switches::defaults());
    install(&mut registry, Arm::session(), None, None).unwrap();
    let plan = registry.build(&hooks::Base).unwrap();
    let chain = &plan.chains[&points::RENDER_GAME_VIEW.base];
    assert!(chain.hooks().any(|h| h.feature == "coop-look-prediction"));
    assert!(chain.render_thread().is_ok());
}

/// A film's update writes the action's facing to the user's look.
#[test]
fn a_film_update_writes_the_recorded_facing_to_the_look_state() {
    let mut m = fixture();
    let action = UPDATE + 12 + 3 * ACTION_BYTES + 4;
    m.put_f32(PC + crate::touch_glue::PC_YAW, 1.0)
        .put_f32(PC + crate::touch_glue::PC_PITCH, 0.5)
        .put_u32(UPDATE + 8, 1 << 3);
    for (yaw, pitch) in [(0.25, -0.1), (6.2, 0.3), (3.0, 0.0)] {
        m.put_f32(action, yaw).put_f32(action + 4, pitch);
        assert_eq!(follow_film(&mut m, UPDATE), 1);
        assert_eq!(m.f32(PC + crate::touch_glue::PC_YAW).unwrap(), yaw);
        assert_eq!(m.f32(PC + crate::touch_glue::PC_PITCH).unwrap(), pitch);
    }
}

#[test]
fn a_film_update_leaves_look_alone_without_an_action_or_a_first_person_subject() {
    let mut m = fixture();
    let action = UPDATE + 12 + 3 * ACTION_BYTES + 4;
    m.put_f32(PC + crate::touch_glue::PC_YAW, 1.0)
        .put_f32(PC + crate::touch_glue::PC_PITCH, 0.5)
        .put_f32(action, 2.0)
        .put_f32(action + 4, 0.1);
    let look = |m: &FakeGuest| {
        (
            m.f32(PC + crate::touch_glue::PC_YAW).unwrap(),
            m.f32(PC + crate::touch_glue::PC_PITCH).unwrap(),
        )
    };
    // The player has no action in this update.
    m.put_u32(UPDATE + 8, 1 << 2);
    assert_eq!(follow_film(&mut m, UPDATE), 0);
    assert_eq!(look(&m), (1.0, 0.5));
    // An angle out of range is not a facing.
    m.put_u32(UPDATE + 8, 1 << 3).put_f32(action + 4, 9.0);
    assert_eq!(follow_film(&mut m, UPDATE), 0);
    assert_eq!(look(&m), (1.0, 0.5));
    // A director that is not first person (a free camera's) is left alone.
    m.put_f32(action + 4, 0.1).put_u32(0x004e_8c50 + 0x140, 0);
    assert_eq!(follow_film(&mut m, UPDATE), 0);
    assert_eq!(look(&m), (1.0, 0.5));
}
