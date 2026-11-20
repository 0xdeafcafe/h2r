//! Local pads cross two guest boundaries. XInputGetState returns the final
//! packet after bot and agent overrides. The input pump publishes port changes
//! that the guest can observe even without calling XInputGetState.
use super::{Mode, Session};
use anyhow::{Result, ensure};
use film::tape::{Call, PADR, PDEV, PortChange, Tap};
use input::{HostBackend, HostPad, HostPadId, InputManager};
use std::sync::{Arc, Mutex};
use xapi::tap::{InputTap, PACKET};

type Ports = [Option<(u32, bool)>; 4];
const EMPTY: Ports = [None; 4];

/// The input manager still handles open handles, removal, rumble completion
/// and polling. Only the source of devices is replaced for a replay.
#[derive(Clone, Default)]
struct FilmBackend(Arc<Mutex<Ports>>);

impl FilmBackend {
    fn set(&self, ports: Ports) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = ports;
    }
}

impl HostBackend for FilmBackend {
    fn name(&self) -> &str {
        "film"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        let ports = *self.0.lock().unwrap_or_else(|e| e.into_inner());
        ports
            .into_iter()
            .enumerate()
            .filter_map(|(port, device)| {
                let (generation, supports_rumble) = device?;
                Some(HostPad {
                    id: HostPadId(u64::from(generation) << 2 | port as u64),
                    name: format!("Film controller {}", port + 1),
                    state: Default::default(),
                    physical: false,
                    supports_rumble,
                })
            })
            .collect()
    }

    fn preferred_port(&self, id: HostPadId) -> Option<usize> {
        Some((id.0 & 3) as usize)
    }
}

struct PortReplay {
    backend: FilmBackend,
    events: Vec<PortChange>,
    next: usize,
}

impl PortReplay {
    fn at(&mut self, cursor: u64) {
        self.next = self.events.partition_point(|change| change.sync <= cursor);
        let ports = self.events[..self.next]
            .last()
            .map_or(EMPTY, |change| change.ports);
        self.backend.set(ports);
    }

    fn advance(&mut self, cursor: u64) {
        if let Some(change) = self.events.get(self.next)
            && change.sync == cursor
        {
            self.backend.set(change.ports);
            self.next += 1;
        }
    }
}

struct FilmPads {
    session: Arc<Session>,
    inner: Option<Box<dyn InputTap>>,
    sync: u64,
    last: Ports,
    replay: Option<PortReplay>,
}

impl FilmPads {
    fn record_ports(&mut self, input: &InputManager) {
        let ports = input.port_connections();
        if ports == self.last {
            return;
        }
        self.last = ports;
        let event = PortChange {
            sync: self.sync,
            ports,
        };
        let mut state = self.session.lock();
        if state.complete || state.failure.is_some() {
            return;
        }
        if let Mode::Record(rec) = &mut state.mode
            && let Some(out) = rec.out.as_mut()
            && let Err(e) = out.chunk(PDEV, &event)
        {
            state.fail(format!("recording XAPI devices: {e}"));
        }
    }
}

impl InputTap for FilmPads {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        const OP: &str = "XInputGetState";
        if self.replay.is_some() {
            // Never return live input if the tape is exhausted or malformed.
            *status = input::gamepad::ERROR_DEVICE_NOT_CONNECTED;
            packet.fill(0);
            let Some(call) = self.session.next(OP, &[handle], 0) else {
                return;
            };
            if let (Ok(recorded_status), [output]) = (call.result, call.outputs.as_slice())
                && let Ok(bytes) = <[u8; PACKET]>::try_from(output.as_slice())
            {
                *status = recorded_status;
                *packet = bytes;
            } else {
                self.session
                    .fault("invalid taped XInputGetState packet".into());
            }
        } else {
            if let Some(inner) = &mut self.inner {
                inner.read(handle, status, packet);
            }
            self.session.record(
                PADR,
                Call {
                    index: 0, // Session assigns the shared call index.
                    op: OP.into(),
                    args: vec![handle],
                    thread: 0,
                    result: Ok(*status),
                    outputs: vec![packet.to_vec()],
                },
            );
        }
    }

    fn opened(&mut self, handle: u32, port: u32) {
        if let Some(inner) = &mut self.inner {
            inner.opened(handle, port);
        }
    }

    fn closed(&mut self, handle: u32) {
        if let Some(inner) = &mut self.inner {
            inner.closed(handle);
        }
    }

    fn sync(&mut self, input: &mut InputManager) {
        self.sync += 1;
        if let Some(replay) = &mut self.replay {
            replay.advance(self.sync);
            input.sync();
        } else {
            if let Some(inner) = &mut self.inner {
                inner.sync(input);
            } else {
                input.sync();
            }
            self.record_ports(input);
        }
    }

    // PADR uses the shared HLE cursor stored in the keyframe. The XAPI input
    // cursor therefore carries the independent device-pump index.
    fn cursor(&self) -> Option<u64> {
        Some(self.sync)
    }

    fn restore_input(&mut self, cursor: u64, input: &mut InputManager) -> Result<(), String> {
        let replay = self
            .replay
            .as_mut()
            .ok_or("cannot rewind a film recorder")?;
        replay.at(cursor);
        input.sync();
        self.sync = cursor;
        Ok(())
    }
}

impl Session {
    #[cfg(feature = "desktop-runtime")]
    pub fn keyboard_tape(self: &Arc<Self>) -> Option<Arc<dyn Tap>> {
        (self.version >= 2).then(|| self.clone() as Arc<dyn Tap>)
    }


    pub fn install_local_pads(
        self: &Arc<Self>,
        xapi: &crate::xapi_glue::Shared,
    ) -> Result<()> {
        let mut state = self.lock();
        if state.header.version < 2 {
            return Ok(());
        }
        let replay = match &mut state.mode {
            Mode::Record(_) => None,
            Mode::Replay(playback, _) => {
                let reader = &mut playback.reader;
                let mut events = Vec::new();
                let records: Vec<_> = reader.machine_section().entries()
                    .filter(|e| e.tag == PDEV).cloned().collect();
                for entry in records {
                    let event: PortChange = reader.read(&entry)?;
                    ensure!(
                        events
                            .last()
                            .is_none_or(|last: &PortChange| last.sync < event.sync),
                        "film device pumps are out of order"
                    );
                    events.push(event);
                }
                Some(PortReplay {
                    backend: FilmBackend::default(),
                    events,
                    next: 0,
                })
            }
        };
        drop(state);
        let mut x = xapi.lock().unwrap_or_else(|e| e.into_inner());
        let mut pads = FilmPads {
            session: self.clone(),
            inner: x.take_input_tap(),
            sync: 0,
            last: EMPTY,
            replay,
        };
        if let Some(replay) = &mut pads.replay {
            let mut input = InputManager::new(Default::default())?;
            replay.at(0);
            input.add_backend(Box::new(replay.backend.clone()));
            input.sync();
            x.input = input;
        } else {
            pads.record_ports(&x.input);
        }
        x.set_input_tap(Some(Box::new(pads)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::PortIndex;

    #[test]
    fn replacement_invalidates_old_handle_and_seek_restores_topology() {
        let port = PortIndex::new(3).unwrap();
        let mut replay = PortReplay {
            backend: FilmBackend::default(),
            events: vec![
                PortChange {
                    sync: 2,
                    ports: [None, None, None, Some((1, true))],
                },
                PortChange {
                    sync: 3,
                    ports: [None, None, None, Some((2, false))],
                },
                PortChange {
                    sync: 5,
                    ports: EMPTY,
                },
            ],
            next: 0,
        };
        let mut input = InputManager::new(Default::default()).unwrap();
        input.add_backend(Box::new(replay.backend.clone()));
        replay.advance(1);
        input.sync();
        assert_eq!(input.connected_mask(), 0);
        replay.advance(2);
        input.sync();
        let original = input.open(port).unwrap();
        assert_eq!(
            input.capabilities(original).unwrap().output.left_motor,
            u16::MAX
        );
        replay.advance(3);
        input.sync();
        assert!(input.get_state(original).is_err());
        assert_eq!(
            input
                .capabilities(input.open(port).unwrap())
                .unwrap()
                .output
                .left_motor,
            0
        );
        replay.at(6);
        input.sync();
        assert_eq!(input.connected_mask(), 0);
        replay.at(2);
        input.sync();
        assert_eq!(input.connected_mask(), 8);
        assert_eq!(
            input
                .capabilities(input.open(port).unwrap())
                .unwrap()
                .output
                .left_motor,
            u16::MAX
        );
    }
}
