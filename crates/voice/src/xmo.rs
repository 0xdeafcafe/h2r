//! The communicator's media objects (docs/live/live-voice.md, 3.3).
//!
//! [`MediaObjects::create`] is XVoiceCreateMediaObjectEx (`0x411143`, over
//! XPP's `0x410f0c`): the same checks in the same order and the same
//! HRESULTs. [`MediaObjects::process`] is the XMO's Process slot
//! (`0x410e77`): it copies the caller's 24-byte XMEDIAPACKET (the engine
//! builds it on its stack, `0x334c43`), marks it pending, and queues it.
//! [`MediaObjects::tick`] runs once per 20 ms: each microphone object fills
//! its head packet with captured PCM, each headphone object plays its head
//! packet, and every packet that is full comes back as a [`Completion`]
//! for the caller to deliver as `0x37f3a8` does.

use crate::{
    HOST_FRAME,
    io::{Frame, VoiceIo},
    resample,
};
use std::collections::{BTreeMap, VecDeque};

/// HRESULTs the original returns.
pub mod hresult {
    pub const S_OK: u32 = 0;
    /// A packet's status while it is queued (`0x37f38a`).
    pub const E_PENDING: u32 = 0x8000_000a;
    pub const E_FAIL: u32 = 0x8000_4005;
    pub const E_NOTIMPL: u32 = 0x8000_4001;
    /// XMEDIAPACKET_STATUS_FLUSHED, for packets still queued at Release
    /// (inferred from the XDK headers, not seen in the title).
    pub const E_ABORT: u32 = 0x8000_4004;
    pub const E_INVALIDARG: u32 = 0x8007_0057;
    /// No communicator on the port (`0x410f0c`), and the status of packets
    /// queued when it is pulled.
    pub const DEVICE_NOT_CONNECTED: u32 = 0x8007_048f;
    /// The port's object of this kind is already open.
    pub const SHARING_VIOLATION: u32 = 0x8007_0020;
}

/// The two halves of a communicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// XPP type `0x4088b0`: fills packets with captured PCM.
    Microphone,
    /// XPP type `0x4088bc`: plays the packets it is given.
    Headphone,
}

/// A WAVEFORMATEX, as the engine passes it (`0x484290`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub tag: u16,
    pub channels: u16,
    pub rate: u32,
    pub average_bytes: u32,
    pub block_align: u16,
    pub bits: u16,
}

impl Format {
    pub const SIZE: usize = 16;

    /// The engine's format: PCM, mono, 16 kHz, 16-bit (`0x332f67`).
    pub const ENGINE: Self = Self {
        tag: 1,
        channels: 1,
        rate: 16_000,
        average_bytes: 32_000,
        block_align: 2,
        bits: 16,
    };

    pub fn parse(bytes: &[u8; Self::SIZE]) -> Self {
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        Self {
            tag: u16_at(0),
            channels: u16_at(2),
            rate: u32_at(4),
            average_bytes: u32_at(8),
            block_align: u16_at(12),
            bits: u16_at(14),
        }
    }

    /// Whether the original accepts the rate for a communicator: the table
    /// at `0x408890` under mask 0x1f, so 8, 11.025, 16, 22.05 or 24 kHz.
    pub fn rate_allowed(rate: u32) -> bool {
        matches!(rate, 8_000 | 11_025 | 16_000 | 22_050 | 24_000)
    }

    /// What the host implements: 16-bit mono PCM at an allowed rate that
    /// divides 48 kHz (8, 16 or 24 kHz). The engine only asks for 16 kHz;
    /// 11.025 and 22.05 kHz are refused rather than resampled.
    pub fn supported(&self) -> bool {
        self.tag == 1
            && self.channels == 1
            && self.bits == 16
            && self.block_align == 2
            && Self::rate_allowed(self.rate)
            && resample::factor(self.rate).is_some()
    }

    /// Bytes of PCM in one 20 ms frame.
    pub fn frame_bytes(&self) -> u32 {
        self.rate / 50 * 2
    }
}

/// An XMEDIAPACKET: 24 bytes, as the engine zeroes and fills it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Packet {
    /// +0x00
    pub buffer: u32,
    /// +0x04; the engine uses 0x280 (640 bytes, 20 ms at 16 kHz).
    pub max_size: u32,
    /// +0x08, may be 0
    pub completed_size: u32,
    /// +0x0c, may be 0
    pub status: u32,
    /// +0x10: the callback's packet context, or the event to set when the
    /// object has no callback. The engine puts the packet index here.
    pub context: u32,
    /// +0x14
    pub timestamp: u32,
}

impl Packet {
    pub const SIZE: usize = 24;

    pub fn parse(bytes: &[u8; Self::SIZE]) -> Self {
        let at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        Self {
            buffer: at(0),
            max_size: at(4),
            completed_size: at(8),
            status: at(12),
            context: at(16),
            timestamp: at(20),
        }
    }
}

/// A guest memory access that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub address: u32,
    pub length: usize,
    pub write: bool,
}

/// Guest memory, as the caller holds it.
pub trait Memory {
    fn read(&self, address: u32, bytes: &mut [u8]) -> Result<(), Fault>;
    fn write(&mut self, address: u32, bytes: &[u8]) -> Result<(), Fault>;
}

/// An open media object, as [`MediaObjects::create`] returns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle(pub u32);

/// How a finished packet is reported, as `0x37f3a8` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    /// stdcall `function(context, packet_context, status)`.
    Callback {
        function: u32,
        context: u32,
        packet_context: u32,
        status: u32,
    },
    /// No callback: set this event.
    Event(u32),
    /// No callback and no event.
    Nothing,
}

/// A packet the object is done with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Completion {
    pub object: Handle,
    pub packet: Packet,
    /// Bytes filled (microphone) or played (headphone).
    pub size: u32,
    pub status: u32,
    pub delivery: Delivery,
}

impl Completion {
    /// Write the completed size and status through the packet's pointers
    /// (each only when set). Do this before [`Completion::delivery`].
    pub fn write(&self, memory: &mut impl Memory) -> Result<(), Fault> {
        if self.packet.completed_size != 0 {
            memory.write(self.packet.completed_size, &self.size.to_le_bytes())?;
        }
        if self.packet.status != 0 {
            memory.write(self.packet.status, &self.status.to_le_bytes())?;
        }
        Ok(())
    }
}

struct Object {
    kind: Kind,
    port: u8,
    format: Format,
    max_packets: u32,
    callback: u32,
    context: u32,
    queue: VecDeque<Packet>,
    /// Bytes of the head packet already filled or played.
    offset: u32,
    up: resample::Up,
}

impl Object {
    fn complete(&self, handle: Handle, packet: Packet, size: u32, status: u32) -> Completion {
        let delivery = if self.callback != 0 {
            Delivery::Callback {
                function: self.callback,
                context: self.context,
                packet_context: packet.context,
                status,
            }
        } else if packet.context != 0 {
            Delivery::Event(packet.context)
        } else {
            Delivery::Nothing
        };
        Completion {
            object: handle,
            packet,
            size,
            status,
            delivery,
        }
    }
}

/// Every port's communicator objects, and the one host microphone and
/// output they share.
pub struct MediaObjects {
    present: u32,
    talker: Option<u8>,
    objects: BTreeMap<Handle, Object>,
    next: u32,
}

impl Default for MediaObjects {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaObjects {
    pub fn new() -> Self {
        Self {
            present: 0,
            talker: None,
            objects: BTreeMap::new(),
            next: 1,
        }
    }

    /// Which ports have a communicator (bits 0..=3); the same mask the
    /// device calls report. Pulling one fails its objects' queued packets
    /// with [`hresult::DEVICE_NOT_CONNECTED`]; the objects stay open until
    /// released, and Process on them fails.
    pub fn set_present(&mut self, ports: u32) -> Vec<Completion> {
        self.present = ports & 0xf;
        let mut done = Vec::new();
        for (&handle, object) in &mut self.objects {
            if self.present & (1 << object.port) == 0 {
                object.offset = 0;
                while let Some(packet) = object.queue.pop_front() {
                    done.push(object.complete(handle, packet, 0, hresult::DEVICE_NOT_CONNECTED));
                }
            }
        }
        done
    }

    pub fn present(&self) -> u32 {
        self.present
    }

    /// The port the host microphone feeds: the player holding push-to-talk,
    /// or in open-mic mode the first signed-in player with voice. `None`
    /// (muted, or push-to-talk up) gives every microphone object silence,
    /// so the engine's voice detection never fires.
    pub fn set_talker(&mut self, port: Option<u8>) {
        self.talker = port.filter(|&p| p < 4);
    }

    /// XVoiceCreateMediaObjectEx(type, port, max packets, format, callback,
    /// context, out), after the type is known. The checks and their
    /// HRESULTs are the original's (`0x410f0c`); the order between them is
    /// the host's: arguments, the device, whether it is open, then what the
    /// host implements.
    pub fn create(
        &mut self,
        kind: Kind,
        port: u32,
        max_packets: u32,
        format: Format,
        callback: u32,
        context: u32,
    ) -> Result<Handle, u32> {
        if port >= 4 || max_packets == 0 || !Format::rate_allowed(format.rate) {
            return Err(hresult::E_INVALIDARG);
        }
        if self.present & (1 << port) == 0 {
            return Err(hresult::DEVICE_NOT_CONNECTED);
        }
        if self
            .objects
            .values()
            .any(|o| o.kind == kind && u32::from(o.port) == port)
        {
            return Err(hresult::SHARING_VIOLATION);
        }
        if !format.supported() {
            return Err(hresult::E_INVALIDARG);
        }
        let handle = Handle(self.next);
        self.next += 1;
        self.objects.insert(
            handle,
            Object {
                kind,
                port: port as u8,
                format,
                max_packets,
                callback,
                context,
                queue: VecDeque::new(),
                offset: 0,
                up: resample::Up::default(),
            },
        );
        Ok(handle)
    }

    pub fn kind(&self, handle: Handle) -> Option<Kind> {
        self.objects.get(&handle).map(|o| o.kind)
    }

    pub fn queued(&self, handle: Handle) -> usize {
        self.objects.get(&handle).map_or(0, |o| o.queue.len())
    }

    /// The XMO's Process(src, dst): a microphone takes a packet to fill as
    /// `dst` (src null), a headphone a packet to play as `src` (dst null).
    /// The packet is copied, so the caller's may be on its stack; its size
    /// and status cells read 0 and [`hresult::E_PENDING`] until it
    /// completes.
    pub fn process(
        &mut self,
        memory: &mut impl Memory,
        handle: Handle,
        src: u32,
        dst: u32,
    ) -> Result<u32, Fault> {
        let present = self.present;
        let Some(object) = self.objects.get_mut(&handle) else {
            return Ok(hresult::E_INVALIDARG);
        };
        let address = match (object.kind, src, dst) {
            (Kind::Microphone, 0, dst) if dst != 0 => dst,
            (Kind::Headphone, src, 0) if src != 0 => src,
            _ => return Ok(hresult::E_INVALIDARG),
        };
        if present & (1 << object.port) == 0 {
            return Ok(hresult::DEVICE_NOT_CONNECTED);
        }
        let mut bytes = [0; Packet::SIZE];
        memory.read(address, &mut bytes)?;
        let packet = Packet::parse(&bytes);
        if packet.buffer == 0 || packet.max_size == 0 {
            return Ok(hresult::E_INVALIDARG);
        }
        // More than the object was opened for. The engine never does this;
        // the original's answer is not traced, so the host refuses.
        if object.queue.len() as u32 >= object.max_packets {
            return Ok(hresult::E_FAIL);
        }
        if packet.completed_size != 0 {
            memory.write(packet.completed_size, &0u32.to_le_bytes())?;
        }
        if packet.status != 0 {
            memory.write(packet.status, &hresult::E_PENDING.to_le_bytes())?;
        }
        object.queue.push_back(packet);
        Ok(hresult::S_OK)
    }

    /// The XMO's last Release: the object goes, and its queued packets
    /// complete as flushed.
    pub fn release(&mut self, handle: Handle) -> Vec<Completion> {
        let Some(mut object) = self.objects.remove(&handle) else {
            return Vec::new();
        };
        let queue = std::mem::take(&mut object.queue);
        queue
            .into_iter()
            .map(|p| object.complete(handle, p, 0, hresult::E_ABORT))
            .collect()
    }

    /// One 20 ms tick. Reads one host frame if any microphone object is
    /// open and writes one mixed frame if any headphone object is; returns
    /// the packets that filled or emptied, in handle order.
    pub fn tick(
        &mut self,
        memory: &mut impl Memory,
        io: &mut dyn VoiceIo,
    ) -> Result<Vec<Completion>, Fault> {
        let mut done = Vec::new();
        let has = |kind| self.objects.values().any(|o| o.kind == kind);
        let (mics, phones) = (has(Kind::Microphone), has(Kind::Headphone));

        let mut captured: Frame = [0; HOST_FRAME];
        if mics && !io.capture().read(&mut captured) {
            captured.fill(0);
        }
        let mut mix = [0i32; HOST_FRAME];

        for (&handle, object) in &mut self.objects {
            if self.present & (1 << object.port) == 0 {
                continue;
            }
            let Some(&packet) = object.queue.front() else {
                continue;
            };
            let factor = resample::factor(object.format.rate).expect("checked at create");
            let mut pcm = vec![0i16; resample::frame_len(object.format.rate)];
            let take = object
                .format
                .frame_bytes()
                .min(packet.max_size - object.offset);
            let at = packet.buffer.wrapping_add(object.offset);
            match object.kind {
                Kind::Microphone => {
                    if self.talker == Some(object.port) {
                        resample::down(&captured, factor, &mut pcm);
                    }
                    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
                    memory.write(at, &bytes[..take as usize])?;
                }
                Kind::Headphone => {
                    let mut bytes = vec![0u8; pcm.len() * 2];
                    memory.read(at, &mut bytes[..take as usize])?;
                    for (s, b) in pcm.iter_mut().zip(bytes.as_chunks::<2>().0) {
                        *s = i16::from_le_bytes(*b);
                    }
                    let mut up = [0i16; HOST_FRAME];
                    object.up.run(&pcm, factor, &mut up);
                    for (m, s) in mix.iter_mut().zip(up) {
                        *m += i32::from(s);
                    }
                }
            }
            object.offset += take;
            if object.offset >= packet.max_size {
                object.queue.pop_front();
                object.offset = 0;
                done.push(object.complete(handle, packet, packet.max_size, hresult::S_OK));
            }
        }

        if phones {
            let mut out: Frame = [0; HOST_FRAME];
            for (o, m) in out.iter_mut().zip(mix) {
                *o = m.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            }
            io.playback().write(&out);
        }
        Ok(done)
    }
}
