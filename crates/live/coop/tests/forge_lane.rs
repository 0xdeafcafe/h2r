//! The Forge lane of the co-op datagram (docs/game/forge-design.md, F5): a
//! reliable ordered byte stream each way and the machines' beats, over a
//! link that drops, delays and reorders.

use coop::Peer;
use coop::link::{Conditions, FakeClock, sim_pair};
use coop::peer::FORGE_WINDOW;
use coop::wire::{BEAT, MAX_FORGE};

const SESSION: u32 = 0x00c0_0b02;

fn bytes(seed: u8, n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

fn run(conditions: Conditions, seed: u64) {
    let clock = FakeClock::default();
    let (la, lb, _) = sim_pair(&clock, conditions, seed);
    let mut a = Peer::new(la, SESSION, 0, 2);
    let mut b = Peer::new(lb, SESSION, 1, 2);
    // More than one datagram's worth each way, sent in pieces.
    let to_b = bytes(1, 5000);
    let to_a = bytes(2, 3000);
    a.forge_set_beat(Some([7; BEAT]));
    b.forge_set_beat(Some([9; BEAT]));
    let (mut got_a, mut got_b) = (Vec::new(), Vec::new());
    let mut sent = (0, 0);
    for step in 0..40_000u64 {
        if sent.0 < to_b.len() && step % 50 == 0 {
            let end = (sent.0 + 700).min(to_b.len());
            assert!(a.forge_send(&to_b[sent.0..end]));
            sent.0 = end;
        }
        if sent.1 < to_a.len() && step % 70 == 0 {
            let end = (sent.1 + 500).min(to_a.len());
            assert!(b.forge_send(&to_a[sent.1..end]));
            sent.1 = end;
        }
        a.pump();
        b.pump();
        got_a.extend(a.forge_recv());
        got_b.extend(b.forge_recv());
        clock.advance(1_000);
        if got_a.len() == to_a.len() && got_b.len() == to_b.len() && a.forge_unacked() == 0 {
            break;
        }
    }
    assert_eq!(got_b, to_b, "seed {seed}");
    assert_eq!(got_a, to_a, "seed {seed}");
    // Each machine sees the other's beat, and the queues drain once acknowledged.
    for _ in 0..200 {
        a.pump();
        b.pump();
        clock.advance(1_000);
    }
    assert_eq!(a.forge_beat(), Some([9; BEAT]));
    assert_eq!(b.forge_beat(), Some([7; BEAT]));
    assert_eq!((a.forge_unacked(), b.forge_unacked()), (0, 0));
}

#[test]
fn a_clean_link_delivers_both_streams_in_order() {
    run(Conditions::default(), 1);
}

#[test]
fn loss_jitter_and_reordering_change_nothing_the_streams_hold() {
    for seed in 1..=8 {
        run(
            Conditions {
                latency: 8_000,
                jitter: 12_000,
                loss: 0.3,
                reorder: 0.2,
            },
            seed,
        );
    }
}

#[test]
fn the_queue_refuses_more_than_it_may_hold_and_queues_nothing() {
    let clock = FakeClock::default();
    let (la, _lb, _) = sim_pair(&clock, Conditions::default(), 1);
    let mut a = Peer::new(la, SESSION, 0, 2);
    let big = vec![0u8; coop::peer::FORGE_OUT_MAX];
    assert!(a.forge_send(&big));
    assert!(!a.forge_send(&[1]));
    assert_eq!(a.forge_unacked(), big.len());
}

/// A welcome-sized stream crosses in a bounded number of round trips: the
/// window carries [`FORGE_WINDOW`] runs a round trip, not one, so a stream
/// of a few kilobytes (a welcome's document and log) joins in seconds on
/// a relay's round trip, not one run a round trip.
#[test]
fn a_welcome_sized_stream_takes_a_bounded_number_of_round_trips() {
    let clock = FakeClock::default();
    let (la, lb, _) = sim_pair(
        &clock,
        Conditions {
            latency: 25_000,
            ..Default::default()
        },
        3,
    );
    let mut a = Peer::new(la, SESSION, 0, 2);
    let mut b = Peer::new(lb, SESSION, 1, 2);
    let stream = bytes(4, 64 << 10);
    assert!(a.forge_send(&stream));
    let rtt = 50_000;
    // One round trip carries a window of runs; a few spare round trips
    // cover the last ack and the link's first flight.
    let windows = stream.len().div_ceil(FORGE_WINDOW * MAX_FORGE);
    let bound = rtt * (windows as u64 + 8);
    let mut got = Vec::new();
    let mut elapsed = 0;
    for _ in 0..40_000 {
        a.pump();
        b.pump();
        got.extend(b.forge_recv());
        clock.advance(1_000);
        elapsed += 1_000;
        if got.len() == stream.len() && a.forge_unacked() == 0 {
            break;
        }
    }
    assert_eq!(got, stream);
    assert!(
        elapsed <= bound,
        "the stream took {elapsed} µs, over the {bound} µs bound"
    );
}
