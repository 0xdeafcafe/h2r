//! The pads of a film played from its game start: a neutral pad on each of
//! the ports the film's options name, and none of the host's. The film's
//! actions drive the players; the pads only keep the guest's controllers
//! connected.
use super::Session;
use anyhow::Result;
use input::{HostBackend, HostPad, HostPadId, InputManager};
use std::sync::Arc;
use xapi::tap::{InputTap, PACKET};

type Ports = [Option<(u32, bool)>; 4];

/// The input manager still handles open handles, removal, rumble completion
/// and polling. Only the source of devices is replaced for a film.
#[derive(Clone)]
struct FilmBackend(Ports);

impl HostBackend for FilmBackend {
    fn name(&self) -> &str {
        "film"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        self.0
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

/// A neutral pad on each of the first `count` ports.
fn neutral_ports(count: usize) -> Ports {
    let mut ports = [None; 4];
    for port in ports.iter_mut().take(count) {
        *port = Some((1, false));
    }
    ports
}

struct NeutralPads {
    /// The tap this one replaced; it sees handles close, nothing else.
    inner: Option<Box<dyn InputTap>>,
    sync: u64,
    ports: Ports,
    /// The handles the guest opened and the port each is on.
    opened: Vec<(u32, u32)>,
}

impl InputTap for NeutralPads {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        packet.fill(0);
        let port = self.opened.iter().rev().find(|(h, _)| *h == handle).map(|(_, port)| *port);
        let connected = port.is_some_and(|port| self.ports.get(port as usize).is_some_and(Option::is_some));
        *status = if connected { 0 } else { input::gamepad::ERROR_DEVICE_NOT_CONNECTED };
    }

    fn opened(&mut self, handle: u32, port: u32) {
        self.opened.push((handle, port));
    }

    fn closed(&mut self, handle: u32) {
        if let Some(inner) = &mut self.inner {
            inner.closed(handle);
        }
    }

    fn sync(&mut self, input: &mut InputManager) {
        self.sync += 1;
        input.sync();
    }

    fn cursor(&self) -> Option<u64> {
        Some(self.sync)
    }

    fn restore_input(&mut self, cursor: u64, input: &mut InputManager) -> Result<(), String> {
        input.sync();
        self.sync = cursor;
        Ok(())
    }
}

impl Session {
    /// A film played from its game start reads neutral pads on its ports;
    /// a recording leaves the pads alone.
    pub fn install_neutral_pads(self: &Arc<Self>, xapi: &crate::xapi_glue::Shared) -> Result<()> {
        let Some(count) = self.lock().play.as_ref().map(|play| play.ports()) else {
            return Ok(());
        };
        let ports = neutral_ports(count);
        let mut x = xapi.lock().unwrap_or_else(|e| e.into_inner());
        let pads = NeutralPads { inner: x.take_input_tap(), sync: 0, ports, opened: Vec::new() };
        let mut input = InputManager::new(Default::default())?;
        input.add_backend(Box::new(FilmBackend(ports)));
        input.sync();
        x.input = input;
        x.set_input_tap(Some(Box::new(pads)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neutral_pad_is_connected_on_the_film_ports_only() {
        let mut input = InputManager::new(Default::default()).unwrap();
        input.add_backend(Box::new(FilmBackend(neutral_ports(2))));
        input.sync();
        assert_eq!(input.connected_mask(), 0b11);
        let mut pads = NeutralPads { inner: None, sync: 0, ports: neutral_ports(2), opened: Vec::new() };
        pads.opened(7, 1);
        pads.opened(8, 3);
        let (mut status, mut packet) = (99, [0xff; PACKET]);
        pads.read(7, &mut status, &mut packet);
        assert_eq!((status, packet), (0, [0; PACKET]));
        pads.read(8, &mut status, &mut packet);
        assert_eq!(status, input::gamepad::ERROR_DEVICE_NOT_CONNECTED);
    }
}
