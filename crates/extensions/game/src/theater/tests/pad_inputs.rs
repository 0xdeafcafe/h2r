use super::*;
use input::{
    HostBackend, InputManager, PortIndex,
    host::{HostPad, HostPadId, HostPadState},
};

struct Controller(HostPadState);

impl HostBackend for Controller {
    fn name(&self) -> &str {
        "film fixture"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        vec![HostPad {
            id: HostPadId(1),
            name: "Controller".into(),
            state: self.0,
            physical: true,
            supports_rumble: false,
        }]
    }

    fn preferred_port(&self, _: HostPadId) -> Option<usize> {
        Some(2)
    }
}

fn controllers(pad: Option<HostPadState>) -> crate::xapi_glue::Shared {
    let mut input = InputManager::new(Default::default()).unwrap();
    if let Some(pad) = pad {
        input.add_backend(Box::new(Controller(pad)));
    }
    let storage = xapi::VfsStorage::new(Arc::new(Mutex::new(vfs::Vfs::new())));
    Arc::new(Mutex::new(
        xapi::Xapi::new(xapi::Config::for_gamepad(0x2000), input, storage).unwrap(),
    ))
}

#[test]
fn replay_reads_the_actual_xinput_packet_from_the_recorded_controller_port() {
    let dir = std::env::temp_dir().join(format!("film-pad-{}", live_proto::sign::random_id()));
    std::fs::create_dir(&dir).unwrap();
    let file = dir.join("local.h2film");
    let opts = options(&dir);
    let facts = || Facts {
        bots: false,
        light: false,
        ..host_facts()
    };
    let record = Session::open(Some(file.clone()), None, facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    let g = arena();

    let pad = HostPadState {
        left_stick: (0.75, -0.5),
        right_trigger: 1.0,
        ..Default::default()
    };
    let record_xapi = controllers(Some(pad));
    record.install_local_pads(&record_xapi).unwrap();
    let port = PortIndex::new(2).unwrap();
    let token = 0x5800_0000;
    let recorded = {
        let mut x = record_xapi.lock().unwrap();
        let mut tap = x.take_input_tap().unwrap();
        tap.sync(&mut x.input);
        let open = x.input.open(port).unwrap();
        tap.sync(&mut x.input);
        let state = x.input.get_state(open).unwrap();
        assert_eq!(state.gamepad, pad.to_gamepad(&Default::default(), true));
        let mut packet = state.to_bytes();
        let mut status = 0;
        tap.read(token, &mut status, &mut packet);
        packet
    };
    record.checkpoint(&g);
    record.finish(1, true).unwrap();

    let replay = Session::open(
        None,
        Some(Replay {
            verification: None,
            path: file,
            render_every: 0,
            speed: 1.,
            seek_tick: 0,
            paused_at_target: false,
            presentation: Default::default(),
            restart: dir.join("restart"),
            camera: Default::default(),
            window_closed: None,
        }),
        facts(),
        b"xbe",
        &opts,
    )
    .unwrap()
    .unwrap();
    let replay_xapi = controllers(None);
    replay.install_local_pads(&replay_xapi).unwrap();
    {
        let mut x = replay_xapi.lock().unwrap();
        let mut tap = x.take_input_tap().unwrap();
        tap.sync(&mut x.input);
        assert_eq!(x.input.connected_mask(), 1 << 2);
        let open = x.input.open(port).unwrap();
        tap.sync(&mut x.input);
        let neutral = x.input.get_state(open).unwrap();
        assert_eq!(neutral.gamepad, input::Gamepad::default());
        let mut packet = neutral.to_bytes();
        let mut status = 0;
        tap.read(token, &mut status, &mut packet);
        assert_eq!(status, 0);
        assert_eq!(packet, recorded);
    }
    replay.checkpoint(&g);
    replay.finish(1, true).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}
