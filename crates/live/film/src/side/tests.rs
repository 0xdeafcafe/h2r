use super::*;
use crate::{
    columns::{ACTION_BYTES, SEATS},
    file::{FilmReader, FilmWriter, Seed, TICK_DIGEST},
};
use std::{
    io::{self, Cursor, Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};

/// A film file being written that the host's forwarder can read as it grows.
#[derive(Clone, Default)]
struct Growing(Arc<Mutex<Cursor<Vec<u8>>>>);
impl Write for Growing {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Seek for Growing {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.0.lock().unwrap().seek(to)
    }
}
impl Growing {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().get_ref().clone()
    }
}

fn row(tick: u64) -> [Option<[u8; ACTION_BYTES]>; SEATS] {
    let mut row = [None; SEATS];
    for seat in 0..4 {
        let mut action = [0u8; ACTION_BYTES];
        action[4..8].copy_from_slice(&((tick * 3 + seat) as f32).to_bits().to_le_bytes());
        action[0] = (tick / 40) as u8;
        row[seat as usize] = Some(action);
    }
    row
}

/// A host's film as it is written, with what its forwarder sends.
fn host() -> (Vec<u8>, Forwarder, Vec<Frame>) {
    let file = Growing::default();
    let seed = Seed {
        header: crate::tests::header(7),
        descriptor: Default::default(),
    };
    let mut writer = FilmWriter::new(file.clone(), &seed, 30).unwrap();
    let (mut host, mut frames) = Forwarder::new(&file.bytes()).unwrap();
    let mut sent = file.bytes().len();
    let mut grown = |host: &mut Forwarder, frames: &mut Vec<Frame>| {
        let now = file.bytes();
        frames.extend(host.forward(&now[sent..]));
        sent = now.len();
    };
    writer
        .start_point(&zstd::bulk::compress(b"start", 3).unwrap())
        .unwrap();
    grown(&mut host, &mut frames);
    for tick in 1..=10 * 256 {
        writer
            .push_tick(TICK_DIGEST, tick * 77, &row(tick))
            .unwrap();
        grown(&mut host, &mut frames);
    }
    // A block too large to forward.
    writer.game_start_frame(&vec![7u8; 70 << 10]).unwrap();
    grown(&mut host, &mut frames);
    writer.finish().unwrap();
    grown(&mut host, &mut frames);
    let film = file.bytes();
    frames.push(host.seal(film[..HEADER_BYTES].try_into().unwrap()));
    (film, host, frames)
}

fn index(frame: &Frame) -> Option<u32> {
    match frame {
        Frame::Part { index, .. } | Frame::Skipped { index, .. } => Some(*index),
        Frame::Seal { .. } => None,
    }
}

#[test]
fn a_client_that_drops_three_forwarded_chunks_ends_with_the_hosts_film() {
    let (film, host, frames) = host();
    // The head, the start point, nine closed chunks, the large block, then
    // the last chunk and the trailer.
    let pieces = frames.iter().filter_map(index).max().unwrap() + 1;
    assert_eq!(pieces, 13);
    let dropped = [6, 9, 12];
    let mut client = Assembler::default();
    for frame in &frames {
        let wire = frame.encode();
        assert!(wire.len() <= MAX_FRAME);
        let frame = Frame::decode(&wire).unwrap();
        if index(&frame).is_some_and(|i| dropped.contains(&i)) {
            continue;
        }
        client.accept(frame).unwrap();
    }
    assert_eq!(client.film(), None);
    assert_eq!(client.missing(), dropped);
    for i in client.missing() {
        for frame in host.resend(i).unwrap() {
            client.accept(frame).unwrap();
        }
    }
    assert!(client.missing().is_empty());
    // The block the host did not forward, read from its film on Live.
    let wanted = client.wanted();
    assert_eq!(wanted.len(), 1);
    for (i, range) in wanted {
        client
            .fill(i, &film[range.start as usize..range.end as usize])
            .unwrap();
    }
    let copy = client.film().unwrap();
    assert_eq!(copy, film);
    let reader = FilmReader::open(Cursor::new(copy)).unwrap();
    assert!(reader.sealed);
    assert_eq!(reader.length_ticks, 10 * 256);
}

#[test]
fn a_piece_the_host_no_longer_holds_is_named_for_a_read_from_live() {
    let (film, host, frames) = host();
    let mut client = Assembler::default();
    for frame in frames.into_iter().filter(|f| index(f) != Some(2)) {
        client.accept(frame).unwrap();
    }
    assert_eq!(client.missing(), [2]);
    let resent = host.resend(2).unwrap();
    assert!(matches!(resent[..], [Frame::Skipped { index: 2, .. }]));
    for frame in resent {
        client.accept(frame).unwrap();
    }
    assert!(client.missing().is_empty());
    for (i, range) in client.wanted() {
        client
            .fill(i, &film[range.start as usize..range.end as usize])
            .unwrap();
    }
    assert_eq!(client.film().unwrap(), film);
}

#[test]
fn frames_out_of_their_piece_or_of_another_film_are_refused() {
    let (_, _, frames) = host();
    let part = frames[0].encode();
    let mut short = part.clone();
    short.pop();
    assert!(Frame::decode(&short).is_err());
    let mut kind = part.clone();
    kind[0] = 9;
    assert!(Frame::decode(&kind).is_err());
    let mut client = Assembler::default();
    client.accept(frames[0].clone()).unwrap();
    let mut other = part;
    other[1] ^= 1;
    assert!(client.accept(Frame::decode(&other).unwrap()).is_err());
}
