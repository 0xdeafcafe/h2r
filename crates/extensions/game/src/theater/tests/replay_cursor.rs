use super::*;

#[test]
fn ring_cursor_preserves_an_applied_action_and_unconsumed_pause_edges() {
    let dir = std::env::temp_dir().join(format!("ring-cursor-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("cursor.h2film");
    let opts = options(&dir);
    let record = Session::open(Some(path.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    let mut guest = arena();
    let time = crate::telemetry::layout::GAME_TIME_PTR;
    guest
        .map_zeroed(time & !4095, 4096, Permissions::RW)
        .unwrap();
    guest.set_u32(time, 0x800000).unwrap();
    guest.set_u8(0x800000, 1).unwrap();
    let action = film::blam::SeatInput {
        seat: 2,
        action: Some([17; 92]),
        pad: None,
    };
    record.record_actions(vec![action]);
    for value in [1, 0] {
        guest.set_u8(0x800001, value).unwrap();
        record.observe_pause(&guest);
    }
    record.checkpoint(&guest);
    record.checkpoint(&guest);
    record.finish(10, true).unwrap();
    let play = Session::open(
        None,
        Some(Replay {
            verification: None,
            path,
            render_every: 0,
            speed: 1.0,
            seek_tick: 0,
            paused_at_target: false,
            presentation: Default::default(),
            restart: dir.join("restart"),
            camera: Default::default(),
            window_closed: None,
        }),
        host_facts(),
        b"xbe",
        &opts,
    )
    .unwrap()
    .unwrap();
    {
        let mut state = play.lock();
        let engine = state.engine.as_mut().unwrap();
        engine.note_actions().unwrap();
        assert_eq!(engine.next_tick().unwrap().inputs, vec![action]);
        engine.observe_pause(true);
    }
    let cursor = play.capture_replay_cursor().unwrap();
    for pass in 0..3 {
        if pass != 0 {
            play.restore_replay_cursor(&cursor).unwrap();
        }
        play.observe_pause(&guest);
        play.checkpoint(&guest);
        play.checkpoint(&guest);
        play.finish(10, true).unwrap();
        assert!(play.lock().complete);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
