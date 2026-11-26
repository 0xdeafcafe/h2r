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
fn a_recording_leaves_the_controller_ports_to_the_host() {
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
    record.install_neutral_pads(&record_xapi).unwrap();
    {
        let mut x = record_xapi.lock().unwrap();
        assert!(x.take_input_tap().is_none(), "a recording installs no pad tap");
        x.input.sync();
        let open = x.input.open(PortIndex::new(2).unwrap()).unwrap();
        let state = x.input.get_state(open).unwrap();
        assert_eq!(state.gamepad, pad.to_gamepad(&Default::default(), true));
    }
    record.checkpoint(&g);
    record.finish(true).unwrap();

    std::fs::remove_dir_all(dir).unwrap();
}
