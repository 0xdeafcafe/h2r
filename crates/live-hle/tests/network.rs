//! Real localhost traffic through synthetic guest frames. No XBE or game image.
use live_hle::{Hle, LiveConfig, NetworkConfig, entries::ENTRIES};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, UdpSocket},
    sync::Arc,
    time::Duration,
};
const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const ADDR: u32 = DATA + 0x2000;
const SET: u32 = DATA + 0x3000;
const TIME: u32 = DATA + 0x4000;
const FRAME: u32 = 0x600f00;
const ERROR: u32 = u32::MAX;
const KID: u32 = DATA + 0x5000;
const KEY: u32 = KID + 0x100;
const XNA: u32 = KID + 0x200;

struct World {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    config: NetworkConfig,
}
impl World {
    fn new() -> Self {
        Self::with(NetworkConfig::new(free_base()))
    }
    fn with(config: NetworkConfig) -> Self {
        let hle = Hle::with_network(
            LiveConfig {
                enabled: true,
                ..LiveConfig::default()
            },
            config.clone(),
        )
        .unwrap();
        let mut guest = Guest::new();
        guest
            .map_zeroed(0x360000, 0x10000, Permissions::RX)
            .unwrap();
        guest
            .map_zeroed(0x3a0000, 0x40000, Permissions::RX)
            .unwrap();
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
        }
    }
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
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
            ebx: 123,
            esi: 456,
            edi: 789,
            ebp: 987,
            ecx: 654,
            edx: 321,
            ..CpuAbi::default()
        };
        let transfer = Runner::default().run(&mut self.cpu, &mut self.ctx, entry.va);
        assert!(
            matches!(self.ctx.event(transfer.event_id), Some(Event::HarnessExit)),
            "{name}: {:?}",
            self.ctx.event(transfer.event_id)
        );
        assert_eq!(self.cpu.esp, FRAME + 4 + args.len() as u32 * 4, "{name}");
        assert_eq!(
            (
                self.cpu.ebx,
                self.cpu.esi,
                self.cpu.edi,
                self.cpu.ebp,
                self.cpu.ecx,
                self.cpu.edx
            ),
            (123, 456, 789, 987, 654, 321),
            "{name}"
        );
        self.cpu.eax
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
    fn u32(&self, p: u32) -> u32 {
        self.rt.memory().u32(p).unwrap()
    }
    fn start(&mut self) {
        assert_eq!(self.call("XNetStartup", &[0]), 0);
        assert_eq!(self.call("WSAStartup", &[2, DATA]), 0);
    }
    fn socket(&mut self, protocol: u32) -> u32 {
        let id = self.call("socket", &[2, if protocol == 6 { 1 } else { 2 }, protocol]);
        assert_ne!(id, ERROR);
        id
    }
    fn bind(&mut self, id: u32, port: u16) {
        self.put(
            ADDR,
            &sockaddr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)),
        );
        assert_eq!(self.call("bind", &[id, ADDR, 16]), 0);
    }
    fn nonblock(&mut self, id: u32, on: bool) {
        self.word(OUT, u32::from(on));
        assert_eq!(self.call("ioctlsocket", &[id, 0x8004667e, OUT]), 0);
    }
    fn option(&mut self, id: u32, name: u32, value: u32) {
        self.word(OUT, value);
        assert_eq!(
            self.call("setsockopt", &[id, 0xffff, name, OUT, 4]),
            0,
            "option {name:x}"
        );
    }
    fn sendto(&mut self, id: u32, dest: SocketAddrV4, data: &[u8]) -> u32 {
        self.put(DATA, data);
        self.put(ADDR, &sockaddr(dest));
        self.call("sendto", &[id, DATA, data.len() as u32, 0, ADDR, 16])
    }
    fn readable(&mut self, id: u32) -> u32 {
        self.word(SET, 1);
        self.word(SET + 4, id);
        self.word(TIME, 1);
        self.word(TIME + 4, 0);
        self.call("select", &[0, SET, 0, 0, TIME])
    }
    fn recvfrom(&mut self, id: u32, max: u32) -> u32 {
        self.word(OUT, 16);
        self.call("recvfrom", &[id, DATA, max, 0, ADDR, OUT])
    }
    fn error(&mut self) -> u32 {
        self.call("WSAGetLastError", &[])
    }
    fn key(&mut self) {
        assert_eq!(self.call("XNetCreateKey", &[KID, KEY]), 0);
        assert_eq!(self.call("XNetRegisterKey", &[KID, KEY]), 0);
    }
    fn peer(&mut self, other: &World) -> u32 {
        let mut b = [0; 36];
        b[..4].copy_from_slice(&other.config.address.ip().octets());
        b[4..8].copy_from_slice(&other.config.address.ip().octets());
        b[8..10].copy_from_slice(&other.config.address.port().to_be_bytes());
        self.put(XNA, &b);
        assert_eq!(self.call("XNetXnAddrToInAddr", &[XNA, KID, OUT]), 0);
        self.u32(OUT)
    }
}
fn sockaddr(a: SocketAddrV4) -> [u8; 16] {
    let mut b = [0; 16];
    b[..2].copy_from_slice(&2u16.to_le_bytes());
    b[2..4].copy_from_slice(&a.port().to_be_bytes());
    b[4..8].copy_from_slice(&a.ip().octets());
    b
}
// Reserve/check a free seven-port range; fixtures release it immediately before
// the HLE binds. No fixed shared port or daemon is needed by the test suite.
fn free_base() -> SocketAddrV4 {
    for _ in 0..128 {
        let first = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let base = first.local_addr().unwrap().port();
        if !(20000..65000).contains(&base) {
            continue;
        }
        let mut held = vec![first];
        for offset in 1..=6 {
            if let Ok(s) = UdpSocket::bind((Ipv4Addr::LOCALHOST, base + offset)) {
                held.push(s);
            } else {
                break;
            }
        }
        if held.len() == 7 {
            return SocketAddrV4::new(Ipv4Addr::LOCALHOST, base);
        }
    }
    panic!("no free loopback port range");
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
    let mut a = World::with(ac);
    let mut b = World::with(bc);
    a.start();
    b.start();
    (a, b)
}

#[test]
fn two_guest_udp_sockets_exchange_payloads_and_guest_sockaddrs() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    assert_eq!(a.sendto(sa, b.config.address, b"request"), 7);
    assert_eq!(b.readable(sb), 1);
    assert_eq!(b.recvfrom(sb, 64), 7);
    assert_eq!(b.get(DATA, 7), b"request");
    assert_eq!(b.u32(OUT), 16);
    assert_eq!(
        b.get(ADDR, 16),
        sockaddr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1000))
    );
    assert_eq!(b.sendto(sb, a.config.address, b"reply"), 5);
    assert_eq!(a.readable(sa), 1);
    assert_eq!(a.recvfrom(sa, 64), 5);
    assert_eq!(a.get(DATA, 5), b"reply");
    assert_eq!(a.call("closesocket", &[sa]), 0);
    assert_eq!(b.call("closesocket", &[sb]), 0);
}
#[test]
fn connected_udp_uses_send_and_recv_without_address_parameters() {
    let (mut a, mut b) = pair();
    let sa = a.socket(254);
    let sb = b.socket(254);
    a.bind(sa, 1005);
    b.bind(sb, 1005);
    let dest = SocketAddrV4::new(Ipv4Addr::LOCALHOST, b.config.address.port() + 5);
    a.put(ADDR, &sockaddr(dest));
    assert_eq!(a.call("connect", &[sa, ADDR, 16]), 0);
    a.put(DATA, b"voice-frame");
    assert_eq!(a.call("send", &[sa, DATA, 11, 0]), 11);
    assert_eq!(b.readable(sb), 1);
    assert_eq!(b.call("recv", &[sb, DATA, 64, 0]), 11);
    assert_eq!(b.get(DATA, 11), b"voice-frame");
}
#[test]
fn nonblocking_empty_recv_sets_guest_thread_error_and_success_preserves_it() {
    let mut w = World::new();
    w.start();
    let id = w.socket(17);
    w.bind(id, 1000);
    w.nonblock(id, true);
    assert_eq!(w.call("recv", &[id, DATA, 16, 0]), ERROR);
    assert_eq!(w.error(), 10035);
    let thread = w.ctx.thread_id;
    w.ctx.thread_id = 2;
    assert_eq!(w.error(), 0);
    assert_eq!(w.call("closesocket", &[999]), ERROR);
    assert_eq!(w.error(), 10038);
    w.ctx.thread_id = thread;
    assert_eq!(w.error(), 10035);
    assert_eq!(w.call("htons/ntohs", &[1000]), 0xe803);
    assert_eq!(w.error(), 10035);
}
#[test]
fn fionread_reports_next_datagram_and_peek_does_not_consume_it() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    a.sendto(sa, b.config.address, b"seven!!");
    assert_eq!(b.readable(sb), 1);
    assert_eq!(b.call("ioctlsocket", &[sb, 0x4004667f, OUT]), 0);
    assert_eq!(b.u32(OUT), 7);
    assert_eq!(b.call("recv", &[sb, DATA, 16, 2]), 7);
    assert_eq!(b.call("recv", &[sb, DATA, 16, 0]), 7);
    b.nonblock(sb, true);
    assert_eq!(b.call("recv", &[sb, DATA, 16, 0]), ERROR);
}
#[test]
fn select_filters_sets_and_fd_is_set_uses_xbox_count_and_handle_array() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    let empty = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    b.bind(empty, 1006);
    b.word(SET, 2);
    b.word(SET + 4, sb);
    b.word(SET + 8, empty);
    b.word(TIME, 0);
    b.word(TIME + 4, 0);
    assert_eq!(b.call("select", &[999, SET, 0, 0, TIME]), 0);
    assert_eq!(b.u32(SET), 0);
    a.sendto(sa, b.config.address, b"ready");
    b.word(SET, 2);
    b.word(SET + 4, sb);
    b.word(SET + 8, empty);
    b.word(TIME, 1);
    assert_eq!(b.call("select", &[0, SET, 0, 0, TIME]), 1);
    assert_eq!(b.u32(SET), 1);
    assert_eq!(b.u32(SET + 4), sb);
    assert_eq!(b.call("__WSAFDIsSet", &[sb, SET]), 1);
    assert_eq!(b.call("__WSAFDIsSet", &[empty, SET]), 0);
    b.word(SET, 1);
    b.word(SET + 4, sb);
    b.word(TIME, 0);
    assert_eq!(b.call("select", &[0, SET, SET, 0, TIME]), 2); // a socket may count in two sets
}
#[test]
fn loopback_broadcast_discovers_peer_on_port_1001_and_reply_uses_secure_mapping() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1001);
    b.bind(sb, 1001);
    a.option(sa, 0x20, 1);
    b.option(sb, 0x20, 1);
    let broadcast = SocketAddrV4::new(Ipv4Addr::BROADCAST, 1001);
    assert_eq!(a.sendto(sa, broadcast, b"synthetic-discovery"), 19);
    assert_eq!(b.readable(sb), 1);
    assert_eq!(b.recvfrom(sb, 128), 19);
    assert_eq!(b.get(DATA, 19), b"synthetic-discovery");
    let ina = u32::from_le_bytes(Ipv4Addr::LOCALHOST.octets());
    assert_eq!(b.call("XNetInAddrToXnAddr", &[ina, XNA, KID]), 0);
    assert_eq!(b.get(XNA + 8, 2), a.config.address.port().to_be_bytes());
    // The real title broadcasts its reply too, on this same channel.
    assert_eq!(b.sendto(sb, broadcast, b"broadcast-reply"), 15);
    assert_eq!(a.readable(sa), 1);
    assert_eq!(a.recvfrom(sa, 64), 15);
    assert_eq!(a.get(DATA, 15), b"broadcast-reply");
    assert_eq!(
        b.sendto(sb, SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1001), b"found"),
        5
    );
    assert_eq!(a.readable(sa), 1);
    assert_eq!(a.recvfrom(sa, 64), 5);
    // Each endpoint excluded itself from the broadcast fan-out.
    a.nonblock(sa, true);
    assert_eq!(a.recvfrom(sa, 64), ERROR);
    assert_eq!(a.error(), 10035);
}
#[test]
fn loopback_broadcast_requires_so_broadcast() {
    let (mut a, _b) = pair();
    let id = a.socket(17);
    a.bind(id, 1001);
    assert_eq!(
        a.sendto(id, SocketAddrV4::new(Ipv4Addr::BROADCAST, 1001), b"probe"),
        ERROR
    );
    assert_eq!(a.error(), 10013);
}
#[test]
fn datagram_truncation_returns_wsaemsgsize_and_preserves_buffer_canary() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    a.sendto(sa, b.config.address, b"oversized");
    b.readable(sb);
    b.put(DATA, &[0xcc; 16]);
    assert_eq!(b.recvfrom(sb, 4), ERROR);
    assert_eq!(b.error(), 10040);
    assert_eq!(b.get(DATA, 8), b"over\xcc\xcc\xcc\xcc");
    b.nonblock(sb, true);
    assert_eq!(b.recvfrom(sb, 4), ERROR);
    assert_eq!(b.error(), 10035);
}
#[test]
fn invalid_guest_outputs_do_not_consume_datagrams() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    a.sendto(sa, b.config.address, b"intact");
    b.readable(sb);
    assert_eq!(b.call("recv", &[sb, 0, 16, 0]), ERROR);
    assert_eq!(b.error(), 10014);
    b.word(OUT, 8);
    assert_eq!(b.call("recvfrom", &[sb, DATA, 16, 0, ADDR, OUT]), ERROR);
    assert_eq!(b.error(), 10014);
    b.word(OUT, u32::MAX);
    assert_eq!(b.call("recvfrom", &[sb, DATA, 16, 0, ADDR, OUT]), ERROR);
    assert_eq!(b.error(), 10014);
    assert_eq!(b.recvfrom(sb, 16), 6);
    assert_eq!(b.get(DATA, 6), b"intact");
}
#[test]
fn tcp_nonblocking_connect_select_send_recv_and_shutdown_reach_real_listener() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let dest = match listener.local_addr().unwrap() {
        std::net::SocketAddr::V4(a) => a,
        _ => unreachable!(),
    };
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut b = [0; 4];
        stream.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"ping");
        stream.write_all(b"pong").unwrap();
        let mut eof = [0; 1];
        assert_eq!(stream.read(&mut eof).unwrap(), 0);
    });
    let mut w = World::new();
    w.start();
    let id = w.socket(6);
    w.nonblock(id, true);
    w.put(ADDR, &sockaddr(dest));
    let result = w.call("connect", &[id, ADDR, 16]);
    if result == ERROR {
        assert_eq!(w.error(), 10035);
    }
    w.word(SET, 1);
    w.word(SET + 4, id);
    w.word(TIME, 1);
    w.word(TIME + 4, 0);
    assert_eq!(w.call("select", &[0, 0, SET, 0, TIME]), 1);
    w.put(DATA, b"ping");
    assert_eq!(w.call("send", &[id, DATA, 4, 0]), 4);
    assert_eq!(w.readable(id), 1);
    assert_eq!(w.call("recv", &[id, DATA, 4, 0]), 4);
    assert_eq!(w.get(DATA, 4), b"pong");
    assert_eq!(w.call("shutdown", &[id, 1]), 0);
    server.join().unwrap();
    assert_eq!(w.readable(id), 1);
    assert_eq!(w.call("recv", &[id, DATA, 4, 0]), 0);
    assert_eq!(w.call("closesocket", &[id]), 0);
}
#[test]
fn blocking_recv_releases_guest_memory_and_network_state_locks() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    b.option(sb, 0x1006, 1000);
    let destination = b.config.address;
    let rt = b.rt.clone();
    let server = std::thread::spawn(move || {
        assert_eq!(b.call("recv", &[sb, DATA, 64, 0]), 4);
        assert_eq!(b.get(DATA, 4), b"wake");
    });
    // This would deadlock if the blocked receiver retained the guest lock.
    rt.memory().set_u32(OUT, 0x1234).unwrap();
    assert_eq!(a.sendto(sa, destination, b"wake"), 4);
    server.join().unwrap();
}
#[test]
fn game_socket_options_are_translated_or_retained_with_guest_layouts() {
    let mut w = World::new();
    w.start();
    let udp = w.socket(254);
    let tcp = w.socket(6);
    for (id, level, name, value) in [
        (udp, 0xffff, 0x20, 1),
        (udp, 0xffff, 4, 1),
        (udp, 0xffff, 0x1001, 16384),
        (udp, 0xffff, 0x1002, 32768),
        (tcp, 0xffff, 0x4001, 0x1234),
        (tcp, 6, 1, 1),
        (tcp, 0xffff, 0x80, 0x00010001),
        (tcp, 0xffff, 0x1005, 200),
        (tcp, 0xffff, 0x1006, 200),
    ] {
        w.word(OUT, value);
        assert_eq!(w.call("setsockopt", &[id, level, name, OUT, 4]), 0);
        w.word(OUT + 4, 4);
        w.put(DATA, &[0xcc; 8]);
        assert_eq!(w.call("getsockopt", &[id, level, name, DATA, OUT + 4]), 0);
        assert_eq!(w.u32(OUT + 4), 4);
        assert_eq!(w.get(DATA + 4, 4), vec![0xcc; 4]);
        if !matches!(name, 0x1001 | 0x1002) {
            assert_eq!(w.u32(DATA), value);
        } else {
            assert!(w.u32(DATA) >= value);
        }
    }
    w.word(OUT, 4);
    assert_eq!(w.call("getsockopt", &[udp, 0xffff, 0x1008, DATA, OUT]), 0);
    assert_eq!(w.u32(DATA), 2);
}
#[test]
fn xbox_security_option_is_available_on_tcp_and_udp_but_not_vdp() {
    let mut w = World::new();
    w.start();
    for protocol in [6, 17, 254] {
        let id = w.socket(protocol);
        w.word(OUT, 1);
        let expected = if protocol == 254 { ERROR } else { 0 };
        assert_eq!(
            w.call("setsockopt", &[id, 0xffff, 0x4001, OUT, 4]),
            expected
        );
        if expected == ERROR {
            assert_eq!(w.error(), 10042);
        }
    }
}

#[test]
fn socket_errors_translate_to_xbox_values_and_handles_are_never_reused() {
    let mut w = World::new();
    assert_eq!(w.call("socket", &[2, 2, 17]), ERROR);
    assert_eq!(w.error(), 10093);
    w.start();
    for (args, code) in [
        ([23, 2, 17], 10047),
        ([2, 3, 17], 10044),
        ([2, 2, 6], 10043),
    ] {
        assert_eq!(w.call("socket", &args), ERROR);
        assert_eq!(w.error(), code);
    }
    let id = w.socket(17);
    assert_eq!(w.call("closesocket", &[id]), 0);
    let next = w.socket(17);
    assert_ne!(id, next);
    assert_eq!(w.call("send", &[id, DATA, 4, 0]), ERROR);
    assert_eq!(w.error(), 10038);
    assert_eq!(w.call("ioctlsocket", &[next, 123, OUT]), ERROR);
    assert_eq!(w.error(), 10022);
    assert_eq!(w.call("setsockopt", &[next, 0xffff, 0xdead, OUT, 4]), ERROR);
    assert_eq!(w.error(), 10042);
    w.word(OUT, u32::MAX);
    assert_eq!(
        w.call("getsockopt", &[next, 0xffff, 0x1008, DATA, OUT]),
        ERROR
    );
    assert_eq!(w.error(), 10014);
    assert_eq!(w.call("shutdown", &[next, 3]), ERROR);
    assert_eq!(w.error(), 10022);
}
#[test]
fn conflicting_bind_is_wsaeaddrinuse_and_bad_sockaddrs_are_rejected() {
    let mut w = World::new();
    w.start();
    let a = w.socket(17);
    let b = w.socket(17);
    w.bind(a, 1000);
    w.put(
        ADDR,
        &sockaddr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 1000)),
    );
    assert_eq!(w.call("bind", &[b, ADDR, 16]), ERROR);
    assert_eq!(w.error(), 10048);
    assert_eq!(w.call("bind", &[b, ADDR, 8]), ERROR);
    assert_eq!(w.error(), 10014);
    w.put(ADDR, &[0; 16]);
    assert_eq!(w.call("connect", &[b, ADDR, 16]), ERROR);
    assert_eq!(w.error(), 10047);
}
#[test]
fn select_rejects_bad_sets_timeout_and_unknown_handles() {
    let mut w = World::new();
    w.start();
    w.word(SET, 65);
    w.word(TIME, 0);
    w.word(TIME + 4, 0);
    assert_eq!(w.call("select", &[0, SET, 0, 0, TIME]), ERROR);
    assert_eq!(w.error(), 10022);
    w.word(SET, 1);
    w.word(SET + 4, 12345);
    assert_eq!(w.call("select", &[0, SET, 0, 0, TIME]), ERROR);
    assert_eq!(w.error(), 10038);
    w.word(SET, 0);
    w.word(TIME, u32::MAX);
    assert_eq!(w.call("select", &[0, SET, 0, 0, TIME]), ERROR);
    assert_eq!(w.error(), 10022);
    assert_eq!(w.call("select", &[0, 0, 0, 0, TIME]), ERROR);
    assert_eq!(w.error(), 10022);
}
#[test]
fn title_address_key_registration_roundtrip_and_connect_status_are_consistent() {
    let (mut a, mut b) = pair();
    a.key();
    assert_eq!(b.call("XNetGetTitleXnAddr", &[XNA]), 4);
    let original = b.get(XNA, 36);
    a.put(XNA, &original);
    assert_eq!(a.call("XNetXnAddrToInAddr", &[XNA, KID, OUT]), 0);
    let ina = a.u32(OUT);
    assert_eq!(ina.to_le_bytes(), Ipv4Addr::LOCALHOST.octets());
    assert_eq!(a.call("XNetGetConnectStatus", &[ina]), 0);
    assert_eq!(a.call("XNetConnect", &[ina]), 0);
    assert_eq!(a.call("XNetGetConnectStatus", &[ina]), 2);
    assert_eq!(a.call("XNetInAddrToXnAddr", &[ina, DATA, DATA + 64]), 0);
    assert_eq!(a.get(DATA, 36), original);
    assert_eq!(a.get(DATA + 64, 8), a.get(KID, 8));
    assert_eq!(a.call("XNetUnregisterInAddr", &[ina]), 0);
    assert_eq!(a.call("XNetGetConnectStatus", &[ina]), 3);
    assert_eq!(a.call("XNetInAddrToXnAddr", &[ina, DATA, 0]), 10022);
    a.peer(&b);
    assert_eq!(a.call("XNetUnregisterKey", &[KID]), 0);
    assert_eq!(a.call("XNetConnect", &[ina]), 10022);
}
#[test]
fn secure_mapping_routes_guest_channel_ports_to_remote_instance_base() {
    let (mut a, mut b) = pair();
    a.key();
    a.peer(&b);
    let sa = a.socket(254);
    let sb = b.socket(254);
    a.bind(sa, 1006);
    b.bind(sb, 1006);
    assert_eq!(
        a.sendto(
            sa,
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1006),
            b"secure-without-encryption"
        ),
        25
    );
    assert_eq!(b.readable(sb), 1);
    assert_eq!(b.recvfrom(sb, 64), 25);
}
#[test]
fn random_and_create_key_fill_only_requested_output_and_system_link_flag() {
    let mut w = World::new();
    w.start();
    w.put(DATA, &[0xcc; 128]);
    assert_eq!(w.call("XNetRandom", &[DATA, 64]), 0);
    let first = w.get(DATA, 64);
    assert_ne!(first, vec![0xcc; 64]);
    assert_eq!(w.get(DATA + 64, 64), vec![0xcc; 64]);
    assert_eq!(w.call("XNetRandom", &[DATA, 64]), 0);
    assert_ne!(first, w.get(DATA, 64));
    assert_eq!(w.call("XNetCreateKey", &[KID, KEY]), 0);
    assert_eq!(w.get(KID, 1)[0] & 0xf0, 0);
    assert_ne!(w.get(KEY, 16), vec![0; 16]);
    assert_eq!(w.call("XNetRandom", &[0, 4]), 10014);
    assert_eq!(w.call("XNetCreateKey", &[0, KEY]), 10014);
}
#[test]
fn key_and_address_errors_do_not_publish_invalid_or_ambiguous_mappings() {
    let (mut a, b) = pair();
    a.key();
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 10048);
    a.peer(&b);
    let old = a.get(XNA, 36);
    let mut other = old.clone();
    other[8..10].copy_from_slice(&(b.config.address.port() + 10).to_be_bytes());
    a.put(XNA, &other);
    assert_eq!(a.call("XNetXnAddrToInAddr", &[XNA, KID, OUT]), 10048);
    a.word(OUT, 0xdeadbeef);
    a.put(XNA, &[0; 36]);
    assert_eq!(a.call("XNetXnAddrToInAddr", &[XNA, KID, OUT]), 10049);
    assert_eq!(a.u32(OUT), 0xdeadbeef);
    assert_eq!(a.call("XNetUnregisterKey", &[KEY + 128]), 10022);
}
#[test]
fn cleanup_closes_sockets_clears_keys_addresses_and_keeps_old_handles_invalid() {
    let (mut a, b) = pair();
    let socket = a.socket(17);
    a.key();
    let ina = a.peer(&b);
    assert_eq!(a.call("WSACleanup", &[]), 0);
    assert_eq!(a.call("send", &[socket, DATA, 1, 0]), ERROR);
    assert_eq!(a.error(), 10093);
    assert_eq!(a.call("WSAStartup", &[2, DATA]), 0);
    assert_eq!(a.call("closesocket", &[socket]), ERROR);
    assert_eq!(a.error(), 10038);
    assert_eq!(a.call("XNetCleanup", &[]), 0);
    assert_eq!(a.call("XNetStartup", &[0]), 0);
    assert_eq!(a.call("XNetGetConnectStatus", &[ina]), 3);
    assert_eq!(a.call("XNetUnregisterKey", &[KID]), 10022);
}
#[test]
fn qos_listener_transfers_lobby_data_and_lookup_completes_with_good_rtt() {
    let (mut a, mut b) = pair();
    b.key();
    a.put(KID, &b.get(KID, 8));
    a.put(KEY, &b.get(KEY, 16));
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 0);
    b.put(DATA, b"synthetic-lobby");
    assert_eq!(
        b.call("XNetQosListen", &[KID, DATA, 15, 2000000, 1 | 4 | 8]),
        0
    );
    a.peer(&b);
    a.word(SET, XNA);
    a.word(SET + 4, KID);
    a.word(SET + 8, KEY);
    assert_eq!(
        a.call(
            "XNetQosLookup",
            &[1, SET, SET + 4, SET + 8, 0, 0, 0, 3, 0, 0, 0, OUT]
        ),
        0
    );
    let q = a.u32(OUT);
    assert_eq!(a.u32(q), 1);
    assert_eq!(a.u32(q + 4), 0);
    assert_eq!(a.get(q + 8, 1), vec![0xb]);
    assert_eq!(a.get(q + 10, 6), vec![3, 0, 3, 0, 15, 0]);
    assert_eq!(a.get(q + 20, 4), vec![5, 0, 5, 0]);
    assert_eq!(a.u32(q + 24), 2000000);
    assert_eq!(a.u32(q + 28), 2000000);
    let data = a.u32(q + 16);
    assert_eq!(a.get(data, 15), b"synthetic-lobby");
    assert!(data >= q + 32);
    assert_eq!(a.call("XNetQosRelease", &[q]), 0);
    assert_eq!(a.call("XNetQosRelease", &[q]), 10022);
    assert_eq!(b.call("XNetQosListen", &[KID, 0, 0, 0, 0x10]), 0);
}
#[test]
fn qos_unreachable_and_disabled_peers_complete_without_inventing_lobby_data() {
    let (mut a, mut b) = pair();
    b.key();
    a.put(KID, &b.get(KID, 8));
    a.put(KEY, &b.get(KEY, 16));
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 0);
    a.peer(&b);
    a.word(SET, XNA);
    a.word(SET + 4, KID);
    a.word(SET + 8, KEY);
    let args = [1, SET, SET + 4, SET + 8, 0, 0, 0, 1, 0, 0, 0, OUT];
    assert_eq!(a.call("XNetQosLookup", &args), 0);
    let q = a.u32(OUT);
    assert_eq!(a.get(q + 8, 1), vec![1]);
    assert_eq!(a.u32(q + 4), 0);
    assert_eq!(a.u32(q + 16), 0);
    assert_eq!(b.call("XNetQosListen", &[KID, 0, 0, 0, 2]), 0);
    assert_eq!(a.call("XNetQosLookup", &args), 0);
    assert_eq!(a.get(a.u32(OUT) + 8, 1), vec![7]);
}
/// The UNABLE TO JOIN ("network problems") case: a guest asks about a
/// session the host doesn't listen for (one it re-advertised under a new
/// id), and the host's responder stays silent, so the lookup completes
/// uncontacted after its tries. The host's current session is answered.
#[test]
fn qos_for_a_session_the_host_left_is_uncontacted_and_the_current_one_answers() {
    let (mut a, mut b) = pair();
    b.key();
    let current = b.get(KID, 8);
    let mut left = current.clone();
    left[7] ^= 0x5a;
    b.put(DATA, b"synthetic-lobby");
    assert_eq!(
        b.call("XNetQosListen", &[KID, DATA, 15, 2000000, 1 | 4 | 8]),
        0
    );
    a.put(KID, &left);
    a.put(KEY, &b.get(KEY, 16));
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 0);
    a.peer(&b);
    a.word(SET, XNA);
    a.word(SET + 4, KID);
    a.word(SET + 8, KEY);
    let args = [1, SET, SET + 4, SET + 8, 0, 0, 0, 3, 0, 0, 0, OUT];
    let started = std::time::Instant::now();
    assert_eq!(a.call("XNetQosLookup", &args), 0);
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(200),
        "tried more than once: {:?}",
        started.elapsed()
    );
    let q = a.u32(OUT);
    assert_eq!(a.get(q + 8, 1), vec![1], "complete, never contacted");
    assert_eq!(a.u32(q + 16), 0);
    assert_eq!(a.call("XNetQosRelease", &[q]), 0);
    // The current session: contacted, with its data.
    a.put(KID, &current);
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 0);
    assert_eq!(a.call("XNetQosLookup", &args), 0);
    let q = a.u32(OUT);
    assert_eq!(a.get(q + 8, 1), vec![0xb]);
    assert_eq!(a.get(a.u32(q + 16), 15), b"synthetic-lobby");
}
/// The 2026-11-18 run: the host listens on its squad's XNKID with no data
/// (`XNetQosListen` flags 5, `0x00073243`), then on the session id the
/// server gave its advertisement with the 172-byte lobby data. A guest
/// probes the squad's id; it gets the lobby data, which the title's join
/// decodes.
#[test]
fn a_probe_for_the_squads_id_gets_the_lobby_data_set_under_the_server_id() {
    let (mut a, mut b) = pair();
    b.key();
    let squad = b.get(KID, 8);
    let key = b.get(KEY, 16);
    assert_eq!(b.call("XNetQosListen", &[KID, 0, 0, 0, 5]), 0);
    // The server's id for the advertisement, registered and listened on.
    let mut server = squad.clone();
    server[1] ^= 0xff;
    b.put(KID, &server);
    assert_eq!(b.call("XNetRegisterKey", &[KID, KEY]), 0);
    assert_eq!(b.call("XNetQosListen", &[KID, 0, 0, 0, 5]), 0);
    b.put(DATA, &[0xab; 172]);
    assert_eq!(b.call("XNetQosListen", &[KID, DATA, 172, 0, 5]), 0);
    // The guest: the squad's key, its address, the probe.
    a.put(KID, &squad);
    a.put(KEY, &key);
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 0);
    a.peer(&b);
    a.word(SET, XNA);
    a.word(SET + 4, KID);
    a.word(SET + 8, KEY);
    let args = [1, SET, SET + 4, SET + 8, 0, 0, 0, 2, 0, 0, 0, OUT];
    assert_eq!(a.call("XNetQosLookup", &args), 0);
    let q = a.u32(OUT);
    assert_eq!(a.get(q + 8, 1), vec![0xb], "contacted, with data");
    assert_eq!(a.get(q + 14, 2), vec![172, 0]);
    assert_eq!(a.get(a.u32(q + 16), 172), vec![0xab; 172]);
}
#[test]
fn qos_rejects_invalid_counts_flags_pointers_and_cleans_up_allocations() {
    let mut w = World::new();
    w.start();
    w.key();
    assert_eq!(w.call("XNetQosListen", &[KID, DATA, 1021, 0, 5]), 10022);
    assert_eq!(w.call("XNetQosListen", &[KID, DATA, 0, 0, 0x80]), 10022);
    assert_eq!(
        w.call("XNetQosLookup", &[65, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, OUT]),
        10022
    );
    assert_eq!(
        w.call("XNetQosLookup", &[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, OUT]),
        0
    );
    let q = w.u32(OUT);
    assert_eq!(w.u32(q), 0);
    assert_eq!(w.call("XNetCleanup", &[]), 0);
    assert_eq!(w.call("XNetQosRelease", &[q]), 10022);
}
#[test]
fn host_byte_order_helpers_use_16_and_32_bit_xbox_values() {
    let mut w = World::new();
    assert_eq!(w.call("htonl/ntohl", &[0x01020304]), 0x04030201);
    assert_eq!(w.call("htons/ntohs", &[0xabcd1234]), 0x3412);
}
#[test]
fn network_configuration_parses_loopback_pair_and_rejects_overlapping_ranges() {
    let c = NetworkConfig::from_values(Some("127.0.0.1:31000"), Some("127.0.0.1:31010")).unwrap();
    assert_eq!(c.address.port(), 31000);
    assert_eq!(c.loopback_peers[0].port(), 31010);
    for (a, b) in [
        ("0.0.0.0:1000", None),
        ("127.0.0.1:65535", None),
        ("127.0.0.1:0", None),
        ("127.0.0.1:31000", Some("127.0.0.1:31005")),
        ("192.168.1.2:31000", Some("127.0.0.1:31010")),
        ("127.0.0.1:31000", Some("garbage")),
    ] {
        assert!(NetworkConfig::from_values(Some(a), b).is_err());
    }
}

#[test]
fn failed_tcp_connect_is_in_except_set_and_so_error_reports_xbox_code() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let dest = match listener.local_addr().unwrap() {
        std::net::SocketAddr::V4(a) => a,
        _ => unreachable!(),
    };
    drop(listener);
    let mut w = World::new();
    w.start();
    let socket = w.socket(6);
    w.nonblock(socket, true);
    w.put(ADDR, &sockaddr(dest));
    assert_eq!(w.call("connect", &[socket, ADDR, 16]), ERROR);
    let immediate = w.error();
    assert!(matches!(immediate, 10035 | 10061));
    if immediate == 10035 {
        w.word(SET, 1);
        w.word(SET + 4, socket);
        w.word(SET + 64, 1);
        w.word(SET + 68, socket);
        w.word(TIME, 1);
        w.word(TIME + 4, 0);
        assert_eq!(w.call("select", &[0, 0, SET, SET + 64, TIME]), 1);
        assert_eq!(w.u32(SET), 0);
        assert_eq!(w.u32(SET + 64), 1);
        // Repeated readiness does not consume the cached connect error.
        w.word(SET, 1);
        w.word(SET + 4, socket);
        assert_eq!(w.call("select", &[0, 0, SET, SET + 64, TIME]), 1);
        w.word(OUT, 4);
        assert_eq!(
            w.call("getsockopt", &[socket, 0xffff, 0x1007, DATA, OUT]),
            0
        );
        assert_eq!(w.u32(DATA), 10061);
    }
}
#[test]
fn fionbio_can_restore_blocking_mode_and_timeout_has_winsock_result() {
    let mut w = World::new();
    w.start();
    let socket = w.socket(17);
    w.bind(socket, 1000);
    w.nonblock(socket, true);
    w.nonblock(socket, false);
    w.option(socket, 0x1006, 30);
    let begin = std::time::Instant::now();
    assert_eq!(w.call("recv", &[socket, DATA, 16, 0]), ERROR);
    assert_eq!(w.error(), 10060);
    assert!(begin.elapsed() >= Duration::from_millis(10));
}
#[test]
fn empty_datagrams_and_fionread_preserve_udp_message_boundaries() {
    let (mut a, mut b) = pair();
    let sa = a.socket(17);
    let sb = b.socket(17);
    a.bind(sa, 1000);
    b.bind(sb, 1000);
    assert_eq!(a.sendto(sa, b.config.address, b""), 0);
    assert_eq!(b.readable(sb), 1);
    assert_eq!(b.call("ioctlsocket", &[sb, 0x4004667f, OUT]), 0);
    assert_eq!(b.u32(OUT), 0);
    assert_eq!(b.recvfrom(sb, 32), 0);
    b.nonblock(sb, true);
    assert_eq!(b.recvfrom(sb, 32), ERROR);
    assert_eq!(b.error(), 10035);
}
#[test]
fn released_peer_qos_storage_is_reused_for_periodic_lobby_queries() {
    let mut w = World::new();
    w.start();
    let args = [0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, OUT];
    assert_eq!(w.call("XNetQosLookup", &args), 0);
    let ptr = w.u32(OUT);
    let mappings = w.rt.memory().mappings().len();
    assert_eq!(w.call("XNetQosRelease", &[ptr]), 0);
    assert_eq!(w.call("XNetQosRelease", &[ptr]), 10022);
    for _ in 0..10000 {
        assert_eq!(w.call("XNetQosLookup", &args), 0);
        let next = w.u32(OUT);
        assert_eq!(next, ptr);
        assert_eq!(w.call("XNetQosRelease", &[next]), 0);
    }
    assert_eq!(w.rt.memory().mappings().len(), mappings);
}

#[test]
fn qos_full_64_peer_result_array_packs_guest_pointers_without_overlap() {
    let (mut a, mut b) = pair();
    b.key();
    a.put(KID, &b.get(KID, 8));
    a.put(KEY, &b.get(KEY, 16));
    assert_eq!(a.call("XNetRegisterKey", &[KID, KEY]), 0);
    b.put(DATA, &[0x5a; 1020]);
    assert_eq!(b.call("XNetQosListen", &[KID, DATA, 1020, 0, 5]), 0);
    a.peer(&b);
    for i in 0..64 {
        a.word(SET + i * 4, XNA);
        a.word(SET + 256 + i * 4, KID);
        a.word(SET + 512 + i * 4, KEY);
    }
    assert_eq!(
        a.call(
            "XNetQosLookup",
            &[64, SET, SET + 256, SET + 512, 0, 0, 0, 2, 0, 0, 0, OUT]
        ),
        0
    );
    let q = a.u32(OUT);
    assert_eq!(a.u32(q), 64);
    assert_eq!(a.u32(q + 4), 0);
    for i in 0..64 {
        let info = q + 8 + 24 * i;
        assert_eq!(a.get(info, 1), vec![0xb]);
        assert_eq!(a.get(info + 6, 2), 1020u16.to_le_bytes());
        let data = a.u32(info + 8);
        assert!(data >= q + 8 + 24 * 64);
        assert_eq!(a.get(data, 1020), vec![0x5a; 1020]);
    }
    assert_eq!(a.call("XNetQosRelease", &[q]), 0);
}
#[test]
fn pure_fd_membership_and_lost_address_status_work_before_startup() {
    let mut w = World::new();
    w.word(SET, 1);
    w.word(SET + 4, 123);
    assert_eq!(w.call("__WSAFDIsSet", &[123, SET]), 1);
    assert_eq!(w.call("XNetGetConnectStatus", &[0x0100007f]), 3);
}
#[test]
fn same_host_discovery_configuration_can_advertise_lan_ipv4_for_the_real_title() {
    let c =
        NetworkConfig::from_values(Some("192.168.1.50:31000"), Some("192.168.1.50:31010")).unwrap();
    assert_eq!(*c.address.ip(), Ipv4Addr::new(192, 168, 1, 50));
}

#[test]
fn nested_transport_references_keep_sockets_and_keys_until_final_cleanup() {
    let (mut a, b) = pair();
    let id = a.socket(17);
    a.bind(id, 1000);
    a.key();
    let ina = a.peer(&b);
    assert_eq!(a.call("XNetStartup", &[0]), 0);
    assert_eq!(a.call("WSAStartup", &[2, DATA]), 0);
    assert_eq!(a.call("XNetCleanup", &[]), 0);
    assert_eq!(a.call("WSACleanup", &[]), 0);
    assert_eq!(a.call("XNetConnect", &[ina]), 0);
    assert_eq!(a.sendto(id, b.config.address, b"alive"), 5);
    assert_eq!(a.call("WSACleanup", &[]), 0);
    assert_eq!(a.call("WSAStartup", &[2, DATA]), 0);
    assert_eq!(a.call("closesocket", &[id]), ERROR);
    assert_eq!(a.error(), 10038);
}
