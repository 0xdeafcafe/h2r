use super::*;
use std::collections::VecDeque;

#[derive(Default)]
struct FakeSocket {
    reads: VecDeque<io::Result<Option<Message>>>,
    sent: Vec<Message>,
}

impl Wire for FakeSocket {
    fn send(&mut self, message: Message) -> io::Result<()> {
        self.sent.push(message);
        Ok(())
    }

    fn read(&mut self) -> io::Result<Option<Message>> {
        self.reads
            .pop_front()
            .unwrap_or_else(|| Err(io::ErrorKind::UnexpectedEof.into()))
    }

    fn close(&mut self) {}
}

fn host_frame(kind: Kind, port: u16, peer: u32, payload: &[u8]) -> Message {
    Message::binary(live_proto::tunnel::encode(&Frame {
        kind,
        port,
        peer,
        payload: payload.to_vec(),
    }))
}

const HOST: Ipv4Addr = Ipv4Addr::new(10, 77, 5, 1);
const PEER: u32 = 0x0102_0304;

#[test]
fn hello_sends_unassigned_client_id_and_reads_host_address() {
    let reply = host_frame(Kind::Hello, 0, PEER, &HOST.octets());
    let mut socket = FakeSocket::default();
    socket.reads.extend([
        Ok(None),
        Ok(Some(Message::Ping(Default::default()))),
        Ok(Some(reply)),
    ]);
    let hello = handshake(&mut socket).unwrap();

    assert_eq!(hello.peer, PEER);
    assert_eq!(hello.host, HOST);
    assert_eq!(
        socket.sent,
        vec![Message::binary([1, 1, 0, 0, 0, 0, 0, 0].to_vec())]
    );
}

#[test]
fn hello_requires_an_assigned_peer_and_exactly_one_synthetic_ipv4() {
    for reply in [
        host_frame(Kind::Hello, 0, 0, &HOST.octets()),
        host_frame(Kind::Hello, 1, PEER, &HOST.octets()),
        host_frame(Kind::Hello, 0, PEER, &[10, 77, 1]),
        host_frame(Kind::Hello, 0, PEER, &[10, 77, 1, 2, 3]),
        host_frame(Kind::Hello, 0, PEER, &[192, 0, 2, 1]),
        host_frame(Kind::Datagram, 0, PEER, &HOST.octets()),
        host_frame(Kind::Bye, 0, PEER, &[]),
        Message::binary(vec![1, 1]),
    ] {
        let mut socket = FakeSocket::default();
        socket.reads.push_back(Ok(Some(reply)));
        assert!(handshake(&mut socket).is_err());
    }
}

#[test]
fn every_channel_delivers_the_original_payload_including_empty_datagrams() {
    for port in 0..=6 {
        for payload in [
            Vec::new(),
            vec![0, 0xff],
            vec![0xab; live_proto::tunnel::MAX_PAYLOAD],
        ] {
            let frame = Frame {
                kind: Kind::Datagram,
                port,
                peer: PEER,
                payload,
            };
            let message = Message::binary(live_proto::tunnel::encode(&frame));
            let hello = Hello {
                peer: PEER,
                host: HOST,
            };
            let Incoming::Datagram(delivered) = incoming(hello, message).unwrap() else {
                panic!("channel {port} did not produce a datagram");
            };
            assert_eq!(delivered, frame);
        }
    }
}

#[test]
fn control_and_foreign_frames_never_deliver_game_data() {
    let hello = Hello {
        peer: PEER,
        host: HOST,
    };
    let bye = host_frame(Kind::Bye, 0, PEER, &[]);
    assert!(matches!(incoming(hello, bye), Ok(Incoming::Bye)));
    for message in [
        Message::Ping(Default::default()),
        Message::Pong(Default::default()),
    ] {
        assert!(matches!(incoming(hello, message), Ok(Incoming::Control)));
    }
    for message in [
        host_frame(Kind::Datagram, 0, PEER + 1, b"other player"),
        host_frame(Kind::Datagram, 6, 0, b"unassigned"),
        host_frame(Kind::Hello, 0, PEER, &HOST.octets()),
        host_frame(Kind::Bye, 6, PEER, &[]),
        host_frame(Kind::Bye, 0, PEER, &[0]),
        Message::binary(vec![1, 0]),
    ] {
        assert!(incoming(hello, message).is_err());
    }
}

#[test]
fn routing_matches_the_whole_advertised_endpoint_and_only_its_seven_ports() {
    // The advertised address may differ from the host's hello address.
    for base in [1, 3074, u16::MAX - 6] {
        let advertised = SocketAddrV4::new(Ipv4Addr::new(192, 0, 2, 7), base);
        for offset in 0..=6 {
            let port = base + offset;
            let destination = SocketAddrV4::new(*advertised.ip(), port);
            assert_eq!(port_offset(advertised, destination), Some(offset));
            for other in [Ipv4Addr::BROADCAST, Ipv4Addr::new(192, 0, 2, 8)] {
                assert_eq!(
                    port_offset(advertised, SocketAddrV4::new(other, port)),
                    None
                );
            }
        }
        for port in [Some(base - 1), base.checked_add(7)].into_iter().flatten() {
            let destination = SocketAddrV4::new(*advertised.ip(), port);
            assert_eq!(port_offset(advertised, destination), None);
        }
    }
}

#[test]
fn upgrade_authenticates_in_a_sensitive_header() {
    let req = request("wss://live.test/v1/hosts/hl-1/tunnel", "join-token").unwrap();
    assert_eq!(req.uri().path(), "/v1/hosts/hl-1/tunnel");
    assert_eq!(req.uri().query(), None);
    let bearer = &req.headers()[AUTHORIZATION];
    assert_eq!(bearer, "Bearer join-token");
    assert!(bearer.is_sensitive());
    assert!(request("wss://live.test", "first\r\nsecond").is_err());
}

#[test]
fn a_drop_gets_one_fresh_hello_even_if_the_second_socket_drops_too() {
    let mut opened = 0;
    let mut peers = Vec::new();
    run_with_retry(
        FakeSocket::default(),
        Hello {
            peer: PEER,
            host: HOST,
        },
        || {
            opened += 1;
            let mut socket = FakeSocket::default();
            let reply = host_frame(Kind::Hello, 0, PEER + 1, &HOST.octets());
            socket.reads.push_back(Ok(Some(reply)));
            Ok(socket)
        },
        |socket, hello| {
            peers.push(hello.peer);
            if hello.peer == PEER + 1 {
                assert_eq!(socket.sent, vec![client_control(Kind::Hello)]);
            }
            Err(ws_error(tungstenite::Error::Protocol(
                tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
            )))
        },
        || false,
    );
    assert_eq!(opened, 1);
    assert_eq!(peers, vec![PEER, PEER + 1]);
}

#[test]
fn a_clean_end_protocol_error_or_local_leave_prevents_reconnect() {
    for (outcome, stopping) in [
        (Ok(()), false),
        (Err(io::ErrorKind::InvalidData), false),
        (Err(io::ErrorKind::ConnectionReset), true),
    ] {
        run_with_retry(
            FakeSocket::default(),
            Hello {
                peer: PEER,
                host: HOST,
            },
            || panic!("unexpected reconnect"),
            |_, _| outcome.map_err(io::Error::from),
            || stopping,
        );
    }
}

#[test]
fn reconnect_to_a_different_host_stops_before_game_traffic() {
    let mut peers = Vec::new();
    run_with_retry(
        FakeSocket::default(),
        Hello {
            peer: PEER,
            host: HOST,
        },
        || {
            let mut socket = FakeSocket::default();
            let other_host = [10, 77, 9, 9];
            socket
                .reads
                .push_back(Ok(Some(host_frame(Kind::Hello, 0, PEER + 1, &other_host))));
            Ok(socket)
        },
        |_, hello| {
            peers.push(hello.peer);
            Err(io::ErrorKind::UnexpectedEof.into())
        },
        || false,
    );
    assert_eq!(peers, vec![PEER]);
}

#[test]
fn game_send_checks_message_size_and_queue_capacity_before_reporting_success() {
    let (out, rx) = sync_channel(1);
    let local = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31000);
    let handle = TunnelHandle(Arc::new(Client {
        out,
        shared: Arc::new(Shared {
            hello: Mutex::new(Hello {
                peer: PEER,
                host: HOST,
            }),
            inject: Mutex::default(),
            stopping: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        }),
        injector: local,
        qos_base: local,
    }));

    let max = vec![0xd5; MAX_PAYLOAD];
    assert_eq!(handle.send(7, &[1]), Err(10022));
    assert_eq!(handle.send(6, &[0; MAX_PAYLOAD + 1]), Err(10040));
    assert_eq!(handle.send(6, &max), Ok(MAX_PAYLOAD));
    assert_eq!(handle.send(1, &[]), Err(10035));
    assert_eq!(
        rx.try_recv().unwrap(),
        Frame {
            kind: Kind::Datagram,
            port: 6,
            peer: 0,
            payload: max
        }
    );
    assert_eq!(handle.send(1, &[]), Ok(0));
    assert_eq!(
        rx.try_recv().unwrap(),
        Frame {
            kind: Kind::Datagram,
            port: 1,
            peer: 0,
            payload: vec![]
        }
    );
    drop(rx);
    assert_eq!(handle.send(0, &[1]), Err(10054));
}

#[test]
fn inbound_channel_selects_the_bound_xnet_port_on_each_address_layout() {
    let local = Ipv4Addr::new(192, 0, 2, 14);
    let base = SocketAddrV4::new(local, 31000);
    let config = crate::NetworkConfig::new(base);
    let mut with_peers = config.clone();
    with_peers
        .loopback_peers
        .push(SocketAddrV4::new(local, 31010));

    for offset in 0..=6 {
        let port = 1000 + offset;
        let wildcard = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        let expected = SocketAddrV4::new(local, base.port() + offset);
        assert_eq!(
            channel_destination(&config, wildcard, offset),
            Some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, expected.port()))
        );
        assert_eq!(
            channel_destination(&with_peers, wildcard, offset),
            Some(expected)
        );
        assert_eq!(
            channel_destination(&config, SocketAddrV4::new(local, port), offset),
            Some(expected)
        );
        assert_eq!(
            channel_destination(&config, wildcard, (offset + 1) % 7),
            None
        );
    }
    assert_eq!(
        channel_destination(&config, SocketAddrV4::new(local, 999), 0),
        None
    );
}

#[test]
fn idle_connections_ping_at_five_seconds_and_time_out_after_three_intervals() {
    let start = Instant::now();
    let after = |ms| start + Duration::from_millis(ms);
    let mut keepalive = Keepalive::new(start);
    let mut wire = FakeSocket::default();

    keepalive.tick(&mut wire, after(4999)).unwrap();
    assert!(wire.sent.is_empty());
    keepalive.tick(&mut wire, after(5000)).unwrap();
    keepalive.tick(&mut wire, after(9999)).unwrap();
    assert_eq!(wire.sent, vec![Message::Ping(Default::default())]);
    keepalive.tick(&mut wire, after(10000)).unwrap();
    assert_eq!(wire.sent.len(), 2);
    assert_eq!(
        keepalive.tick(&mut wire, after(15000)).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
}

#[test]
fn received_traffic_keeps_the_tunnel_alive_across_ping_intervals() {
    let start = Instant::now();
    let mut keepalive = Keepalive::new(start);
    let mut wire = FakeSocket::default();
    for i in 1..=5 {
        let now = start + Duration::from_secs(5 * i);
        keepalive.tick(&mut wire, now).unwrap();
        keepalive.heard(now + Duration::from_millis(2));
    }
    assert_eq!(wire.sent, vec![Message::Ping(Default::default()); 5]);
}
