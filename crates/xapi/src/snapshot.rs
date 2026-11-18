//! Machine snapshots (docs/runtime/snapshots.md): the `xapi` section.
//!
//! Saved: the guest's open pad handles (token, port, last state read),
//! `next_handle`, so tokens made after a load are the saved run's, the
//! device-change words of the gamepad and communicator types, and each
//! port's published state and polling parameters, so packet numbers carry
//! on. A load rebinds each handle to the device now on its port
//! ([`InputManager::reopen`]); a port that is empty on the loading host
//! reads as unplugged, and the change words report the difference as an
//! insertion or removal at the guest's next `XGetDeviceChanges`.
//!
//! Refused while live: rumble requests not yet completed and acknowledged,
//! a title launch request, save-game searches.
//!
//! [`InputManager::reopen`]: input::InputManager::reopen
use crate::controllers::{DeviceChanges, OpenInput};
use crate::{Storage, Xapi};
use ::snapshot::codec::{Reader, Writer};
use ::snapshot::{Codec, Snapshot, SnapshotWriter, Tag};
use input::device::PORT_COUNT;
use input::gamepad::{Gamepad, PollingParameters, State};
use input::{PortIndex, PortSnapshot};

pub const SECTION: Tag = Tag::new("xapi");
const VERSION: u32 = 1;
/// The most handles `reserve_handle` lets the guest hold.
const MAX_HANDLES: usize = 4096;

#[derive(Debug)]
pub enum SnapshotError {
    /// Something is in flight that a snapshot cannot carry.
    Busy(Vec<String>),
    Codec(::snapshot::Error),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(why) => write!(f, "xapi is not quiescent: {}", why.join("; ")),
            Self::Codec(e) => write!(f, "xapi section: {e}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

impl From<::snapshot::Error> for SnapshotError {
    fn from(e: ::snapshot::Error) -> Self {
        Self::Codec(e)
    }
}

fn corrupt(what: &str) -> ::snapshot::Error {
    ::snapshot::Error::Corrupt("xapi", what.to_owned())
}

fn put_state(w: &mut Writer, s: &State) {
    w.u32(s.packet_number).raw(&s.gamepad.to_bytes());
}

fn get_state(r: &mut Reader<'_>) -> Result<State, ::snapshot::Error> {
    Ok(State {
        packet_number: r.u32()?,
        gamepad: Gamepad::from_bytes(&r.array::<{ Gamepad::SIZE }>()?),
    })
}

fn put_changes(w: &mut Writer, c: &DeviceChanges) {
    w.u32(c.current).u32(c.changed).u32(c.previous);
}

fn get_changes(r: &mut Reader<'_>) -> Result<DeviceChanges, ::snapshot::Error> {
    Ok(DeviceChanges {
        current: r.u32()?,
        changed: r.u32()?,
        previous: r.u32()?,
    })
}

fn get_port(r: &mut Reader<'_>) -> Result<PortIndex, ::snapshot::Error> {
    PortIndex::new(r.u8()? as usize).ok_or_else(|| corrupt("port out of range"))
}

/// The section, decoded and checked before anything changes.
struct Saved {
    next_handle: u32,
    handles: Vec<(u32, PortIndex, State)>,
    ports: Vec<Option<PortSnapshot>>,
    changes: DeviceChanges,
    voice_ports: u32,
    microphone_changes: DeviceChanges,
    headphone_changes: DeviceChanges,
}

fn decode(bytes: &[u8]) -> Result<Saved, ::snapshot::Error> {
    let r = &mut Reader::new(bytes, "xapi");
    let next_handle = r.u32()?;
    let handles = r.list(5 + State::SIZE, |r| Ok((r.u32()?, get_port(r)?, get_state(r)?)))?;
    if handles.len() > MAX_HANDLES {
        return Err(corrupt("too many handles"));
    }
    let mut ports = Vec::with_capacity(PORT_COUNT);
    for _ in 0..PORT_COUNT {
        ports.push(if r.bool()? {
            Some(PortSnapshot {
                published: get_state(r)?,
                polling: PollingParameters {
                    auto_poll: r.bool()?,
                    interrupt_out: r.bool()?,
                    input_interval: r.u8()?,
                    output_interval: r.u8()?,
                },
            })
        } else {
            None
        });
    }
    let saved = Saved {
        next_handle,
        handles,
        ports,
        changes: get_changes(r)?,
        voice_ports: r.u32()?,
        microphone_changes: get_changes(r)?,
        headphone_changes: get_changes(r)?,
    };
    r.finish()?;
    Ok(saved)
}

impl<StorageBackend: Storage> Xapi<StorageBackend> {
    /// What a snapshot taken now would refuse; empty when quiescent.
    pub fn snapshot_blockers(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.pending_feedback.is_empty()
            || !self.completions.is_empty()
            || !self.signals.is_empty()
        {
            out.push(format!(
                "{} rumble requests in flight",
                self.pending_feedback.len()
            ));
        }
        if self.launch_request.is_some() {
            out.push("a title launch is requested".into());
        }
        if self.pending_search.is_some() || !self.searches.is_empty() {
            out.push("a save-game search is open".into());
        }
        out
    }

    /// Add the `xapi` section. Refuses unless [`Self::snapshot_blockers`]
    /// is empty.
    pub fn save_snapshot(&self, out: &mut SnapshotWriter) -> Result<(), SnapshotError> {
        let blockers = self.snapshot_blockers();
        if !blockers.is_empty() {
            return Err(SnapshotError::Busy(blockers));
        }
        let w = &mut Writer::new();
        w.u32(self.next_handle);
        let mut handles: Vec<_> = self.handles.iter().collect();
        handles.sort_by_key(|(token, _)| **token);
        w.list(&handles, |w, (token, open)| {
            w.u32(**token).u8(open.port.get() as u8);
            put_state(w, &open.last_state);
        });
        for port in 0..PORT_COUNT {
            let port = PortIndex::new(port).expect("below PORT_COUNT");
            match self.input.port_snapshot(port) {
                Some(s) => {
                    w.bool(true);
                    put_state(w, &s.published);
                    w.bool(s.polling.auto_poll)
                        .bool(s.polling.interrupt_out)
                        .u8(s.polling.input_interval)
                        .u8(s.polling.output_interval);
                }
                None => {
                    w.bool(false);
                }
            }
        }
        put_changes(w, &self.changes);
        w.u32(self.voice_ports);
        put_changes(w, &self.microphone_changes);
        put_changes(w, &self.headphone_changes);
        let bytes = std::mem::take(w).into_bytes();
        out.section(SECTION, VERSION, Codec::Stored, bytes)?;
        Ok(())
    }

    /// Replace the guest-visible XAPI state with a snapshot's, after guest
    /// RAM is loaded. Anything in flight on this machine (rumble requests,
    /// a launch request, searches) belonged to the RAM the load replaced and
    /// is dropped. The input tap is told every old handle closed and every
    /// loaded one opened.
    pub fn load_snapshot(&mut self, snapshot: &Snapshot<'_>) -> Result<(), SnapshotError> {
        let bytes = snapshot.require(SECTION, VERSION..=VERSION)?;
        let saved = decode(&bytes)?;

        for (token, open) in std::mem::take(&mut self.handles) {
            self.input.close(open.device_handle);
            if let Some(tap) = self.input_tap.as_mut() {
                tap.closed(token);
            }
        }
        self.pending_feedback.clear();
        self.completions.clear();
        self.signals.clear();
        self.launch_request = None;
        self.pending_search = None;
        self.searches.clear();
        let _ = self.input.take_completions();
        // Edges seen before the load are this run's; the difference from
        // the saved ports is worked out below instead.
        let _ = self.input.device_changes();

        for (port, state) in saved.ports.iter().enumerate() {
            if let Some(state) = state {
                let port = PortIndex::new(port).expect("below PORT_COUNT");
                self.input.restore_port(port, *state);
            }
        }
        for (token, port, last_state) in saved.handles {
            let device_handle = self.input.reopen(port);
            self.handles.insert(
                token,
                OpenInput {
                    device_handle,
                    port,
                    last_state,
                },
            );
            if let Some(tap) = self.input_tap.as_mut() {
                tap.opened(token, port.get() as u32);
            }
        }
        self.next_handle = saved.next_handle;

        // The guest last saw the saved ports; whatever differs on this host
        // is a change it has yet to consume.
        let connected = self.input.connected_mask();
        self.changes = saved.changes;
        self.changes.changed |= self.changes.current ^ connected;
        self.changes.current = connected;
        let host_voice = self.voice_ports;
        self.voice_ports = saved.voice_ports;
        self.microphone_changes = saved.microphone_changes;
        self.headphone_changes = saved.headphone_changes;
        self.set_voice_ports(host_voice);
        Ok(())
    }
}
