//! A client film's machine tape (docs/game/film-format.md, "The machine
//! tape"): a fake client session receives real localhost traffic while its
//! tape records, the tape goes through the film's block format, and a
//! replay with no host and no network gives the guest the same reads in
//! the same order. No XBE or game image.
use film::tape::Role;
use live_hle::{Hle, LiveConfig, NetworkConfig, entries::ENTRIES, tape::Tape};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::Arc,
};

const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const ADDR: u32 = DATA + 0x2000;
const SET: u32 = DATA + 0x3000;
const TIME: u32 = DATA + 0x4000;
const KID: u32 = DATA + 0x5000;
const KEY: u32 = KID + 0x100;
const FRAME: u32 = 0x600f00;
const ERROR: u32 = u32::MAX;

struct World {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    config: NetworkConfig,
    hle: Arc<Hle>,
}

impl World {
    fn with(config: NetworkConfig, tape: Option<Arc<Tape>>) -> Self {
        let hle = Hle::with_network(
            LiveConfig {
                enabled: true,
                ..LiveConfig::default()
            },
            config.clone(),
        )
        .unwrap();
        if let Some(tape) = tape {
            assert!(hle.set_tape(tape).is_ok());
        }
        let mut guest = Guest::new();
        guest.map_zeroed(0x360000, 0x10000, Permissions::RX).unwrap();
        guest.map_zeroed(0x3a0000, 0x40000, Permissions::RX).unwrap();
        guest.map_zeroed(0x600000, 0x1000, Permissions::RW).unwrap();
        guest.map_zeroed(DATA, 0x10000, Permissions::RW).unwrap();
        let mut dispatch = Dispatch::default();
        live_hle::register(&mut dispatch, &hle).unwrap();
        let rt = Runtime::new(guest, dispatch);
        let ctx = ExecCtx::new(rt.clone(), 1);
        Self {
            rt,
            ctx,
            cpu: CpuAbi::default(),
            config,
            hle,
        }
    }

    /// The call's `eax`, or what it said when it stopped the guest.
    fn try_call(&mut self, name: &str, args: &[u32]) -> Result<u32, String> {
        let entry = ENTRIES.iter().find(|e| e.name == name).unwrap();
        assert_eq!(args.len(), entry.stack_words as usize);
        {
            let mut m = self.rt.memory();
            m.set_u32(FRAME, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(FRAME + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu = CpuAbi {
            esp: FRAME,
            eip: entry.va,
            ..CpuAbi::default()
        };
        let transfer = Runner::default().run(&mut self.cpu, &mut self.ctx, entry.va);
        match self.ctx.event(transfer.event_id) {
            Some(Event::HarnessExit) => Ok(self.cpu.eax),
            Some(Event::Unsupported { detail, .. }) => Err(detail.clone()),
            other => panic!("{name}: {other:?}"),
        }
    }

    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        self.try_call(name, args).unwrap_or_else(|why| panic!("{name}: {why}"))
    }

    fn put(&self, p: u32, data: &[u8]) {
        self.rt.memory().write_bytes(p, data, 1).unwrap();
    }

    fn word(&self, p: u32, v: u32) {
        self.put(p, &v.to_le_bytes());
    }

    fn get(&self, p: u32, n: usize) -> Vec<u8> {
        let mut b = vec![0; n];
        self.rt.memory().read_bytes(p, &mut b, 1).unwrap();
        b
    }

    fn start(&mut self) {
        assert_eq!(self.call("XNetStartup", &[0]), 0);
        assert_eq!(self.call("WSAStartup", &[2, DATA]), 0);
    }
}

fn sockaddr(a: SocketAddrV4) -> [u8; 16] {
    let mut b = [0; 16];
    b[..2].copy_from_slice(&2u16.to_le_bytes());
    b[2..4].copy_from_slice(&a.port().to_be_bytes());
    b[4..8].copy_from_slice(&a.ip().octets());
    b
}

/// A free seven-port loopback range, released just before the HLE binds.
fn free_base() -> SocketAddrV4 {
    for _ in 0..128 {
        let first = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = first.local_addr().unwrap().port();
        if !(20000..65000).contains(&base) {
            continue;
        }
        let mut held = vec![first];
        for offset in 1..=6 {
            match UdpSocket::bind((Ipv4Addr::LOCALHOST, base + offset)) {
                Ok(s) => held.push(s),
                Err(_) => break,
            }
        }
        if held.len() == 7 {
            return SocketAddrV4::new(Ipv4Addr::LOCALHOST, base);
        }
    }
    panic!("no free loopback port range");
}

/// What the guest saw from one call: the call, its `eax` and the bytes of
/// its buffers after it.
#[derive(Debug, PartialEq, Eq)]
struct Read {
    call: &'static str,
    eax: u32,
    data: Vec<u8>,
    addr: Vec<u8>,
    out: Vec<u8>,
}

fn read(w: &World, call: &'static str, eax: u32) -> Read {
    Read {
        call,
        eax,
        data: w.get(DATA, 16),
        addr: w.get(ADDR, 16),
        out: w.get(OUT, 4),
    }
}

/// The client's side of a short session. `host(n)` runs where the host
/// sends its n-th datagram; the replay passes one that does nothing.
fn client(w: &mut World, mut host: impl FnMut(u32)) -> Vec<Read> {
    let mut reads = Vec::new();
    w.start();
    let id = w.call("socket", &[2, 2, 17]);
    assert_ne!(id, ERROR);
    w.put(ADDR, &sockaddr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1000)));
    assert_eq!(w.call("bind", &[id, ADDR, 16]), 0);
    w.word(OUT, 1);
    assert_eq!(w.call("ioctlsocket", &[id, 0x8004667e, OUT]), 0);
    // Nothing has come yet.
    let eax = w.call("recv", &[id, DATA, 16, 0]);
    reads.push(read(w, "recv", eax));
    let eax = w.call("WSAGetLastError", &[]);
    reads.push(read(w, "WSAGetLastError", eax));
    for n in 0..2 {
        host(n);
        w.word(SET, 1);
        w.word(SET + 4, id);
        w.word(TIME, 1);
        w.word(TIME + 4, 0);
        let eax = w.call("select", &[0, SET, 0, 0, TIME]);
        reads.push(read(w, "select", eax));
        let eax = w.call("ioctlsocket", &[id, 0x4004667f, OUT]);
        reads.push(read(w, "ioctlsocket FIONREAD", eax));
        w.word(OUT, 16);
        let eax = w.call("recvfrom", &[id, DATA, 16, 0, ADDR, OUT]);
        reads.push(read(w, "recvfrom", eax));
    }
    let eax = w.call("XNetRandom", &[DATA, 8]);
    reads.push(read(w, "XNetRandom", eax));
    reads
}

fn pair() -> (World, World) {
    let a = free_base();
    let mut b = free_base();
    while a.port().abs_diff(b.port()) <= 6 {
        b = free_base();
    }
    let mut ac = NetworkConfig::new(a);
    ac.loopback_peers = vec![b];
    let mut bc = NetworkConfig::new(b);
    bc.loopback_peers = vec![a];
    let host = World::with(ac, None);
    let client = World::with(bc, Some(Tape::recorder()));
    (host, client)
}

/// Record the client's session against a live host, and return its reads
/// and its tape as the film file stores it.
fn recorded() -> (Vec<Read>, Vec<film::tape::Call>) {
    let (mut host, mut client_world) = pair();
    host.start();
    let hs = host.call("socket", &[2, 2, 17]);
    host.put(ADDR, &sockaddr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1000)));
    assert_eq!(host.call("bind", &[hs, ADDR, 16]), 0);
    let to = client_world.config.address;
    let reads = client(&mut client_world, |n| {
        let payload: &[u8] = if n == 0 { b"one" } else { b"second" };
        host.put(DATA, payload);
        host.put(ADDR, &sockaddr(to));
        assert_eq!(host.call("sendto", &[hs, DATA, payload.len() as u32, 0, ADDR, 16]), payload.len() as u32);
    });
    // What the client got from the host, as the guest saw it.
    assert_eq!(reads[0].eax, ERROR);
    assert_eq!(reads[1].eax, 10035);
    assert_eq!(reads[4].eax, 3);
    assert_eq!(&reads[4].data[..3], b"one");
    assert_eq!(reads[7].eax, 6);
    assert_eq!(&reads[7].data[..6], b"second");
    let tape = client_world.hle.tape().unwrap();
    assert_eq!(tape.spoiled(), None);
    // Through the film file's tape blocks and back.
    let film_tape = film::tape::Tape {
        calls: tape.calls(),
        pads: Vec::new(),
    };
    let mut back = film::tape::Tape::default();
    for block in film_tape.blocks().unwrap() {
        back.push_block(&block).unwrap();
    }
    assert_eq!(back, film_tape);
    (reads, back.calls)
}

/// A replay world at a loopback range nobody sends to: no host runs.
fn replay(calls: Vec<film::tape::Call>) -> World {
    World::with(NetworkConfig::new(free_base()), Some(Tape::replayer(calls)))
}

#[test]
fn a_replayed_client_gets_the_same_reads_in_the_same_order_with_no_network() {
    let (reads, calls) = recorded();
    let total = calls.len() as u64;
    let mut w = replay(calls);
    let replayed = client(&mut w, |_| {});
    assert_eq!(replayed, reads);
    let tape = w.hle.tape().unwrap();
    assert_eq!(tape.failure(), None);
    assert_eq!(tape.position(), total);
}

#[test]
fn a_read_past_the_tape_is_a_named_error_not_live_data() {
    let (_, calls) = recorded();
    let total = calls.len();
    let mut w = replay(calls);
    let _ = client(&mut w, |_| {});
    let why = w.try_call("recv", &[0x100, DATA, 16, 0]).unwrap_err();
    assert!(why.contains(&format!("call {total} (recv) has no recorded answer")), "{why}");
    // The replay stays stopped: a later call is the same error.
    let again = w.try_call("WSAGetLastError", &[]).unwrap_err();
    assert_eq!(again, why);
}

#[test]
fn a_call_that_differs_from_the_tape_is_a_named_error() {
    let (_, calls) = recorded();
    let mut w = replay(calls);
    // The recording started with XNetStartup; a replay that calls socket
    // first has gone another way.
    let why = w.try_call("socket", &[2, 2, 17]).unwrap_err();
    assert!(why.contains("call 0 is socket"), "{why}");
    assert!(why.contains("tape holds XNetStartup"), "{why}");
}

#[test]
fn a_console_that_joined_another_consoles_session_is_its_client() {
    let (mut host, mut client) = pair();
    host.start();
    client.start();
    assert_eq!(host.hle.film_role(), Role::Local);
    assert_eq!(host.call("XNetCreateKey", &[KID, KEY]), 0);
    assert_eq!(host.call("XNetRegisterKey", &[KID, KEY]), 0);
    // The client registers the host's key, as a join hands it over.
    client.put(KID, &host.get(KID, 8));
    client.put(KEY, &host.get(KEY, 16));
    assert_eq!(client.call("XNetRegisterKey", &[KID, KEY]), 0);
    assert_eq!(host.hle.film_role(), Role::Host);
    assert_eq!(client.hle.film_role(), Role::SystemLinkClient);
}
