//! Native causal columns over the same quantized values the game passes to
//! the v7 writer. Historical state and probability tables are serializable;
//! the outer record has one sequence/reset tag, tick and payload per sample.
use super::super::*;
use super::arithmetic::{Bits, Coder, Input, Output};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
#[cfg(test)]
use std::time::{Duration, Instant};
const HIT_LIST: usize = LIST_COUNT;
const LISTS_WITH_HITS: usize = LIST_COUNT + 1;
const CHANNELS: usize = 24;
const WIDTH: usize = 128;
const META: usize = LISTS_WITH_HITS * CHANNELS * WIDTH;

#[derive(Default, Clone, Copy, Serialize, Deserialize)]
struct FieldHistory {
    previous: [i64; 3],
    slope: [i64; 3],
    present: bool,
    shared: bool,
}
impl FieldHistory {
    fn update(&mut self, v: [i64; 3]) {
        self.slope = if self.present {
            std::array::from_fn(|i| v[i] - self.previous[i])
        } else {
            [0; 3]
        };
        self.present = true;
        self.previous = v;
    }
}

type Header = (String, Option<u32>, Option<u64>, bool);
#[derive(Default, Serialize, Deserialize)]
struct History {
    rows: [Vec<Vec<FieldHistory>>; LISTS_WITH_HITS],
    lengths: [usize; LISTS_WITH_HITS],
    #[serde(default)]
    cold: [Vec<Entity>; LISTS_WITH_HITS],
    #[serde(skip)]
    held_lists: u8,
    strings: Vec<Val>,
    #[serde(skip)]
    ids: HashMap<Val, usize>,
    meta: Header,
    tick: u64,
    seq: u64,
}
struct Connection {
    model: Vec<u16>,
    history: History,
}
impl Default for Connection {
    fn default() -> Self {
        Self {
            model: vec![2048; META + 256],
            history: History::default(),
        }
    }
}
fn var(out: &mut Vec<u8>, mut n: u64) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn number(input: &mut &[u8]) -> u64 {
    let mut n = 0;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = input.split_first().expect("truncated research packet");
        *input = rest;
        n |= u64::from(b & 127) << shift;
        if b < 128 {
            return n;
        }
    }
    panic!("bad research packet")
}
trait Symbols {
    const WRITING: bool;
    const SPARSE: bool = false;
    fn bit(&mut self, ctx: usize, want: bool) -> bool;
    fn uint(&mut self, ctx: usize, want: u64) -> u64;
    fn signed(&mut self, ctx: usize, want: i64) -> i64 {
        unzigzag(self.uint(ctx, zigzag(want)))
    }
    fn literal(&mut self, want: u8) -> u8;
}
impl<T: Bits> Symbols for Coder<'_, T> {
    const WRITING: bool = T::WRITING;
    #[inline]
    fn bit(&mut self, c: usize, v: bool) -> bool {
        self.bit(c, v)
    }
    #[inline]
    fn uint(&mut self, c: usize, v: u64) -> u64 {
        self.uint(c, v)
    }
    #[inline]
    fn literal(&mut self, v: u8) -> u8 {
        self.literal(v)
    }
}
fn blob<T: Symbols>(r: &mut T, ctx: usize, input: &[u8]) -> Vec<u8> {
    let n = r.uint(ctx, input.len() as u64) as usize;
    assert!(n <= 1 << 20);
    (0..n)
        .map(|i| r.literal(input.get(i).copied().unwrap_or(0)))
        .collect()
}
fn dictionary<T: Symbols>(
    r: &mut T,
    ctx: usize,
    source: Option<&Val>,
    prev: i64,
    strings: &mut Vec<Val>,
    ids: &mut HashMap<Val, usize>,
) -> (usize, Val) {
    let want = source
        .and_then(|v| ids.get(v))
        .copied()
        .unwrap_or(strings.len());
    let id = (prev + r.signed(ctx, want as i64 - prev)) as usize;
    if id == strings.len() {
        let literal = source
            .map(|v| serde_json::to_vec(v).unwrap())
            .unwrap_or_default();
        let bytes = blob(r, META + 64, &literal);
        let v: Val = source
            .cloned()
            .unwrap_or_else(|| serde_json::from_slice(&bytes).unwrap());
        ids.insert(v.clone(), id);
        strings.push(v);
    }
    (
        id,
        if T::WRITING {
            Val::None
        } else {
            strings[id].clone()
        },
    )
}

#[derive(Default)]
struct References {
    positions: Vec<Val>,
    first_aim: Option<([f64; 3], f64)>,
}
struct RowContext {
    direction: Option<[u32; 2]>,
    travel: Option<[i64; 3]>,
    hit: bool,
}
fn direction_base(first: Option<([f64; 3], f64)>, yaw: Option<&Val>) -> Option<[u32; 2]> {
    let (v, base) = first?;
    let Val::Int(y) = yaw? else { return None };
    crate::archive::rotated(v, dyaw(*y) - base).ok()
}
fn travel_base(vel: Option<&FieldHistory>, dt: u64, regular: bool) -> Option<[i64; 3]> {
    let vel = vel.filter(|v| v.present && dt <= PREDICT_TICKS)?;
    Some(std::array::from_fn(|i| {
        let dv = if regular { vel.slope[i] } else { 0 };
        let d = (2 * vel.previous[i] + dv) * 4 * dt as i64;
        (d + 15 * d.signum()) / 30
    }))
}

impl History {
    fn read_frame<T: Symbols>(&mut self, r: &mut T, dt: u64, seq: u64) -> Frame {
        if r.bit(META, false) {
            self.meta = serde_json::from_slice(&blob(r, META + 1, &[])).unwrap();
        }
        let mut refs = References::default();
        let lists = std::array::from_fn(|j| self.fields(r, j, None, dt, &mut refs));
        let hits = self.fields(r, HIT_LIST, None, dt, &mut refs);
        self.tick += dt;
        self.seq = seq + 1;
        let (game, bsp, cut, cinematic) = &self.meta;
        frame_of(
            game,
            self.tick,
            bsp.map(u64::from),
            *cut,
            *cinematic,
            &lists,
            &hits,
        )
        .unwrap()
    }

    fn write_frame<T: Symbols>(&mut self, r: &mut T, frame: &Frame, src: &Lists, dt: u64) {
        let meta = (&frame.game_id, frame.bsp, frame.cut, frame.cinematic);
        if r.bit(
            META,
            meta != (&self.meta.0, self.meta.1, self.meta.2, self.meta.3),
        ) {
            let data = serde_json::to_vec(&meta).unwrap();
            blob(r, META + 1, &data);
            self.meta = (frame.game_id.clone(), frame.bsp, frame.cut, frame.cinematic);
        }
        let mut refs = References::default();
        for (list, rows) in src
            .lists
            .iter()
            .chain(std::iter::once(&src.hits))
            .enumerate()
        {
            self.fields(r, list, Some(rows), dt, &mut refs);
        }
    }

    fn fields<T: Symbols>(
        &mut self,
        r: &mut T,
        list: usize,
        src: Option<&[Entity]>,
        dt: u64,
        refs: &mut References,
    ) -> Vec<Entity> {
        let (schema, max) = if list == HIT_LIST {
            (HIT, MAX_FRAME_HITS)
        } else {
            LISTS[list]
        };
        let base = list * CHANNELS * WIDTH;
        let static_list = T::SPARSE && matches!(list, 3 | 4 | 6);
        if static_list && r.bit(base + 23 * WIDTH + 40, self.held_lists & (1 << list) != 0) {
            return if T::WRITING {
                Vec::new()
            } else {
                self.cold[list].clone()
            };
        }
        let want = src.map_or(0, <[Entity]>::len) as i64;
        let n = (self.lengths[list] as i64
            + r.signed(base + 23 * WIDTH, want - self.lengths[list] as i64))
            as usize;
        assert!(n <= max);
        self.lengths[list] = n;
        let rows = &mut self.rows[list];
        rows.resize_with(n, || vec![FieldHistory::default(); schema.len()]);
        let velocity = schema.iter().position(|f| matches!(f.kind, Kind::Vel));
        let is_unit = list == 0 || list == 2;
        let aim = if list == 0 { 6 } else { 4 };
        // A single decision covers all slowly changing fields in this row.
        // The ordinary field grammar handles every exception and list replacement.
        let full_mask = (1u32 << schema.len()) - 1;
        let hot_mask = if !T::SPARSE {
            full_mask
        } else {
            match list {
                0 => {
                    (1 << 2)
                        | (1 << 3)
                        | (1 << 6)
                        | (1 << 8)
                        | (1 << 13)
                        | (1 << 14)
                        | (1 << 16)
                        | (1 << 20)
                }
                1 => (1 << 2) | (1 << 3) | (1 << 4) | (1 << 7) | (1 << 8),
                _ => full_mask,
            }
        };
        let grouped = hot_mask != full_mask;
        let cache = &mut self.cold[list];
        if grouped {
            cache.resize_with(n, Vec::new);
        }
        refs.first_aim = None;
        let mut out = Vec::with_capacity(if T::WRITING { 0 } else { n });
        for (i, hist) in rows.iter_mut().enumerate() {
            let mut e: Entity = Vec::with_capacity(if T::WRITING { 0 } else { schema.len() });
            let source = src.map(|v| &v[i]);
            let mut ctx = RowContext {
                direction: None,
                travel: travel_base(velocity.map(|vi| &hist[vi]), dt, is_unit),
                hit: list == HIT_LIST,
            };
            let held = grouped
                && r.bit(
                    base + 23 * WIDTH + 41,
                    source.is_some_and(|s| {
                        let prior = &cache[i];
                        if prior.is_empty() {
                            return false;
                        }
                        let mut bits = full_mask ^ hot_mask;
                        while bits != 0 {
                            let j = bits.trailing_zeros() as usize;
                            bits &= bits - 1;
                            if s[j] != prior[j] {
                                return false;
                            }
                        }
                        true
                    }),
                );
            if !T::WRITING && held {
                e.clone_from(&cache[i]);
            }
            let mut mask = if held { hot_mask } else { full_mask };
            while mask != 0 {
                let j = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                let field = &schema[j];
                if is_unit && j == aim {
                    ctx.direction = direction_base(refs.first_aim, source.unwrap_or(&e).get(3));
                } else {
                    ctx.direction = None;
                }
                let expect = source.map(|s| &s[j]);
                let value = channel(
                    r,
                    base + j * WIDTH,
                    field,
                    &mut hist[j],
                    expect,
                    &ctx,
                    &refs.positions,
                    &mut self.strings,
                    &mut self.ids,
                );
                if !T::WRITING {
                    if held {
                        e[j] = value;
                    } else {
                        e.push(value);
                    }
                }
            }
            let actual = source.unwrap_or(&e);
            if grouped && !held {
                cache[i].clone_from(actual);
            }
            if list == 0 {
                refs.positions.push(actual[2].clone());
            }
            if i == 0
                && is_unit
                && let (Val::Dir(q), Val::Int(y)) = (&actual[aim], &actual[3])
            {
                refs.first_aim = Some((unoct(*q), dyaw(*y)));
            }
            if !T::WRITING {
                out.push(e);
            }
        }
        if static_list {
            src.unwrap_or(&out).clone_into(cache);
        }
        out
    }
}
#[allow(clippy::too_many_arguments)]
fn channel<T: Symbols>(
    r: &mut T,
    ctx: usize,
    field: &Field,
    h: &mut FieldHistory,
    src: Option<&Val>,
    row: &RowContext,
    players: &[Val],
    strings: &mut Vec<Val>,
    ids: &mut HashMap<Val, usize>,
) -> Val {
    let was = h.present;
    if field.opt && !r.bit(ctx + usize::from(was), src.is_some_and(|v| *v != Val::None)) {
        *h = FieldHistory::default();
        return Val::None;
    }
    let lane = ctx + 8;
    if matches!(
        field.kind,
        Kind::Id | Kind::Str | Kind::Actions | Kind::Agos
    ) {
        let (id, v) = dictionary(r, lane, src, h.previous[0], strings, ids);
        h.update([id as i64, 0, 0]);
        return v;
    }
    if matches!(field.kind, Kind::Bool) {
        let v = r.bit(
            ctx + 2 + usize::from(h.previous[0] != 0),
            matches!(src, Some(Val::Bool(true))),
        );
        h.update([i64::from(v), 0, 0]);
        return Val::Bool(v);
    }
    let mut value = match src {
        Some(Val::Int(n)) => [*n as i64, 0, 0],
        Some(Val::Pos(q) | Val::Vel(q)) => *q,
        Some(Val::Dir(q)) => [i64::from(q[0]), i64::from(q[1]), 0],
        _ => [0; 3],
    };
    let count = match field.kind {
        Kind::Pos | Kind::Vel => 3,
        Kind::Dir => 2,
        _ => 1,
    };
    if row.hit && matches!(field.kind, Kind::Pos) && !players.is_empty() {
        let ordinal = src.and_then(|p| players.iter().position(|v| v == p));
        if r.bit(ctx + 4, ordinal.is_some()) {
            let index = r.uint(lane, ordinal.unwrap_or(0) as u64) as usize;
            return players[index].clone();
        }
    }
    let mut shift = [0; 3];
    if let Some(base) = row.direction {
        let share = (0..2).all(|k| (value[k] - i64::from(base[k])).abs() <= 16);
        let share = r.bit(ctx + 5 + usize::from(h.shared), share);
        if share != h.shared {
            *h = FieldHistory::default();
            h.shared = share;
        }
        if share {
            shift = [i64::from(base[0]), i64::from(base[1]), 0];
        }
    } else if h.shared {
        *h = FieldHistory::default();
    }
    for k in 0..count {
        let prev = h.previous[k];
        let pred = match field.kind {
            Kind::Pos => prev + row.travel.map_or(h.slope[k], |d| d[k]),
            Kind::Vel | Kind::Dir | Kind::Angle | Kind::AnimPhase => prev + h.slope[k],
            _ => prev,
        };
        let mut delta = value[k] - shift[k] - pred;
        if matches!(field.kind, Kind::Angle) {
            delta = (delta + 8192).rem_euclid(16384) - 8192;
        }
        value[k] = pred + r.signed(lane + 32 * k, delta);
        if matches!(field.kind, Kind::Angle) {
            value[k] = value[k].rem_euclid(16384);
        }
    }
    h.update(value);
    match field.kind {
        Kind::Pos => Val::Pos(value),
        Kind::Vel => Val::Vel(value),
        Kind::Dir => Val::Dir([(value[0] + shift[0]) as u32, (value[1] + shift[1]) as u32]),
        _ => Val::Int(value[0] as u64),
    }
}
impl Connection {
    fn reset(&mut self) {
        self.model.fill(2048);
        self.history = History::default();
    }
    fn encode(&mut self, frame: &Frame, key: bool) -> Vec<u8> {
        let seq = self.history.seq;
        if key {
            self.reset();
        }
        let dt = frame.tick - self.history.tick;
        let src = Lists::of(frame).unwrap();
        let mut payload = Vec::new();
        var(&mut payload, seq * 2 + u64::from(key));
        var(&mut payload, dt);
        let mut r = Coder::new(Output::default(), &mut self.model);
        self.history.write_frame(&mut r, frame, &src, dt);
        payload.extend(r.finish());
        self.history.seq = seq + 1;
        self.history.tick = frame.tick;
        let mut packet = Vec::new();
        push_record(&mut packet, &payload);
        packet
    }
    fn decode(&mut self, packet: &[u8]) -> Frame {
        let mut it = records(packet);
        let mut body = it.next().unwrap().unwrap();
        assert!(it.next().is_none());
        let tag = number(&mut body);
        let seq = tag / 2;
        if tag & 1 != 0 {
            self.reset();
        } else {
            assert_eq!(seq, self.history.seq);
        }
        let dt = number(&mut body);
        let mut r = Coder::new(Input::new(body), &mut self.model);
        self.history.read_frame(&mut r, dt, seq)
    }
    fn snapshot(&self) -> Vec<u8> {
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        serde_json::to_writer(&mut z, &self.history).unwrap();
        // Length delimiter is independent of JSON whitespace or string data.
        let data = z.finish().unwrap();
        let mut bytes = Vec::new();
        var(&mut bytes, data.len() as u64);
        bytes.extend(data);
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        for &p in &self.model {
            z.write_all(&p.to_le_bytes()).unwrap();
        }
        bytes.extend(z.finish().unwrap());
        bytes
    }
    fn resume(&mut self, mut bytes: &[u8]) {
        let n = number(&mut bytes) as usize;
        self.history =
            serde_json::from_reader(flate2::read::DeflateDecoder::new(&bytes[..n])).unwrap();
        self.history.ids = self
            .history
            .strings
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, v)| (v, i))
            .collect();
        let mut z = flate2::read::DeflateDecoder::new(&bytes[n..]);
        for p in &mut self.model {
            let mut b = [0; 2];
            z.read_exact(&mut b).unwrap();
            *p = u16::from_le_bytes(b);
        }
    }
}
#[test]
fn native_causal_keys_deltas_and_midstream_join() {
    for frames in [
        crate::samples::multiplayer(16),
        crate::samples::campaign(12),
    ] {
        let mut enc = Connection::default();
        let mut dec = Connection::default();
        for (n, f) in frames.iter().enumerate() {
            if n == 5 {
                dec = Connection::default();
                dec.resume(&enc.snapshot());
            }
            if n == 9 {
                dec = Connection::default();
            }
            let bytes = enc.encode(f, n == 0 || n == 9);
            assert_eq!(dec.decode(&bytes), quantize(f).unwrap());
        }
    }
}
#[test]
#[ignore = "native causal entropy codec, one independently terminated arithmetic packet per sample"]
fn bench_live_floor_native() {
    let frames = crate::samples::multiplayer(9000);
    for interval in [150, 450, usize::MAX] {
        let mut enc = Connection::default();
        let mut dec = Connection::default();
        let mut control = Encoder::new(0, 150);
        let mut v7_time = Duration::ZERO;
        let mut v7_bytes = 0;
        let mut container = Vec::new();
        let mut total = 0;
        let mut time = Duration::ZERO;
        let mut snapshot = 0;
        for (n, f) in frames.iter().enumerate() {
            if n == 4771 {
                let b = enc.snapshot();
                snapshot = b.len();
                dec = Connection::default();
                dec.resume(&b);
            }
            // Alternate first position so both writers get both cache conditions.
            let mut v7 = || {
                let start = Instant::now();
                let (record, _) = control.encode(f).unwrap();
                container.clear();
                push_record(&mut container, &record);
                v7_time += start.elapsed();
                v7_bytes += container.len();
            };
            if n % 2 == 0 {
                v7();
            }
            let start = Instant::now();
            let packet = enc.encode(f, n % interval == 0);
            time += start.elapsed();
            if n % 2 != 0 {
                v7();
            }
            total += packet.len();
            assert_eq!(dec.decode(&packet), quantize(f).unwrap());
        }
        assert_eq!(v7_bytes, 1_394_368);
        println!(
            "paired v7 control {:.3} us/frame; causal/v7 {:.3}",
            v7_time.as_secs_f64() * 1e6 / frames.len() as f64,
            time.as_secs_f64() / v7_time.as_secs_f64()
        );
        println!(
            "native causal interval_frames={interval} bytes={total} B/player/s={:.3} encode_us/frame={:.3} snapshot_B={snapshot}",
            total as f64 / 9600.,
            time.as_secs_f64() * 1e6 / frames.len() as f64
        );
    }
}

#[cfg(feature = "live-floor-bench")]
#[path = "speed.rs"]
pub(super) mod speed;
