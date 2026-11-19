//! Measurement hooks in the real v7 writer. All accounting is compiled out of the library.
use super::*;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::hash::Hash;

thread_local! {
    pub(super) static COARSE_POSITION: Cell<bool> = const { Cell::new(false) };
    pub(super) static COARSE_AIM: Cell<bool> = const { Cell::new(false) };
    pub(super) static SHORT_AXIS: Cell<bool> = const { Cell::new(false) };
    static BILL: RefCell<Option<Bill>> = const { RefCell::new(None) };
}
const NAMES: &[&str] = &[
    "player",
    "vehicle",
    "actor",
    "machine",
    "object",
    "projectile",
    "item",
    "hit",
    "frame",
];
const FIELDS: &[&[&str]] = &[
    &[
        "index",
        "alive",
        "pos",
        "yaw",
        "vehicle",
        "species",
        "aim",
        "weapon",
        "shots",
        "stance",
        "speed",
        "actions",
        "grenade",
        "vel",
        "fire",
        "body",
        "shield",
        "seat",
        "actions_ago",
        "anim_name",
        "anim_phase",
    ],
    &[
        "id", "kind", "pos", "forward", "up", "occupied", "parent", "aim", "vel",
    ],
    &[
        "id",
        "species",
        "pos",
        "yaw",
        "aim",
        "weapon",
        "shots",
        "team",
        "vehicle",
        "stance",
        "speed",
        "actions",
        "grenade",
        "vel",
        "fire",
        "body",
        "shield",
        "seat",
        "actions_ago",
        "anim_name",
        "anim_phase",
    ],
    &["node", "id", "kind", "pos", "forward", "up", "position"],
    &["kind", "index", "gone", "pos", "forward", "up"],
    &["id", "kind", "pos", "vel", "parent"],
    &["id", "kind", "pos", "forward", "up", "equipment"],
    &["from", "to", "weapon", "pos", "amount", "kind", "ago"],
];
#[derive(Default)]
struct Bill {
    key: usize,
    list: usize,
    cursor: usize,
    bits: BTreeMap<(usize, &'static str), [usize; 2]>,
    residuals: BTreeMap<(usize, usize, usize), Histogram>,
}

pub(super) fn start(key: bool) {
    BILL.with_borrow_mut(|b| {
        if let Some(b) = b {
            b.key = key as usize;
            b.list = 8;
            b.cursor = 0;
            if key {
                for h in b.residuals.values_mut() {
                    h.previous = None;
                }
            }
        }
    });
}
pub(super) fn set_list(n: usize) {
    BILL.with_borrow_mut(|b| {
        if let Some(b) = b {
            b.list = n;
        }
    });
}
impl Bill {
    fn charge(&mut self, bits: usize, field: &'static str) {
        self.bits.entry((self.list, field)).or_default()[self.key] += bits - self.cursor;
        self.cursor = bits;
    }
}
pub(super) fn charge(w: &BitWriter, field: &'static str) {
    BILL.with_borrow_mut(|b| {
        if let Some(b) = b {
            b.charge(w.bit_len(), field);
        }
    });
}
pub(super) fn before_field(schema: &[Field], i: usize) {
    if COARSE_AIM.get() {
        SHORT_AXIS.set(
            schema.len() == PLAYER.len()
                && matches!((schema[0].kind, i), (Kind::Uint(_), 6) | (Kind::Id, 4)),
        );
    }
}
pub(super) fn field(w: &BitWriter, i: usize) {
    SHORT_AXIS.set(false);
    BILL.with_borrow_mut(|b| {
        if let Some(b) = b {
            b.charge(w.bit_len(), FIELDS[b.list][i]);
        }
    });
}
pub(super) fn end(w: &BitWriter) {
    BILL.with_borrow_mut(|b| {
        if let Some(b) = b {
            assert_eq!(w.bit_len(), b.cursor);
            b.list = 8;
            let bytes = w.bit_len().div_ceil(8);
            b.charge(bytes * 8, "padding");
            let prefix = ((usize::BITS - bytes.leading_zeros()).max(1)).div_ceil(7) as usize;
            b.charge((bytes + prefix) * 8, "length");
        }
    });
}

#[derive(Default)]
struct Histogram {
    totals: HashMap<Symbol, usize>,
    pairs: HashMap<(Symbol, Symbol), usize>,
    starts: HashMap<Symbol, usize>,
    previous: Option<Symbol>,
}
#[derive(Clone, Hash, PartialEq, Eq)]
enum Symbol {
    Delta(i64),
    Literal(Val),
}
impl Histogram {
    fn see(&mut self, symbol: Symbol) {
        *self.totals.entry(symbol.clone()).or_default() += 1;
        if let Some(prev) = self.previous.replace(symbol.clone()) {
            *self.pairs.entry((prev, symbol)).or_default() += 1;
        } else {
            *self.starts.entry(symbol).or_default() += 1;
        }
    }
    fn bits(&self) -> (f64, f64) {
        fn entropy<K: Eq + Hash>(freq: &HashMap<K, usize>) -> f64 {
            let n = freq.values().sum::<usize>() as f64;
            freq.values()
                .map(|&v| v as f64 * (n / v as f64).log2())
                .sum()
        }
        let mut parents = HashMap::new();
        for ((p, _), &n) in &self.pairs {
            *parents.entry(p).or_insert(0) += n;
        }
        (
            entropy(&self.totals),
            entropy(&self.starts)
                + self
                    .pairs
                    .iter()
                    .map(|((p, _), &n)| n as f64 * (parents[p] as f64 / n as f64).log2())
                    .sum::<f64>(),
        )
    }
}

/// Traverse matched entities (including unchanged runs) against the writer's
/// actual post-advance base. Absent/present changes are tagged literals. H1's
/// predecessor is the preceding residual in that field/axis in wire order.
pub(super) fn matched(
    schema: &[Field],
    list: &[Entity],
    prev: &[Entity],
    matched: impl Fn(usize) -> Option<usize>,
    dt: u64,
) {
    BILL.with_borrow_mut(|b| {
        let Some(b) = b else { return };
        for (i, e) in list.iter().enumerate() {
            let Some(j) = matched(i) else { continue };
            for (f, (v, p)) in e.iter().zip(&prev[j]).enumerate() {
                let mut see = |axis, s| b.residuals.entry((b.list, f, axis)).or_default().see(s);
                match (v, p) {
                    (Val::Pos(a), Val::Pos(z)) | (Val::Vel(a), Val::Vel(z)) => {
                        for k in 0..3 {
                            see(k, Symbol::Delta(a[k] - z[k]));
                        }
                    }
                    (Val::Dir(a), Val::Dir(z)) => {
                        for k in 0..2 {
                            see(k, Symbol::Delta(i64::from(a[k]) - i64::from(z[k])));
                        }
                    }
                    (Val::Int(a), Val::Int(z)) => {
                        let mut d = *a as i64 - *z as i64;
                        if matches!(schema[f].kind, Kind::Angle) {
                            d = (d + 8192).rem_euclid(16384) - 8192;
                        }
                        if matches!(schema[f].kind, Kind::AnimPhase)
                            && let Some(p) = advanced_phase(Some(p), dt)
                        {
                            d = *a as i64 - p as i64;
                        }
                        see(0, Symbol::Delta(d));
                    }
                    _ if v == p => see(0, Symbol::Delta(0)),
                    _ => see(0, Symbol::Literal(v.clone())),
                }
            }
        }
    });
}
pub(crate) fn wire(frames: &[Frame], interval: u64) -> usize {
    let mut enc = Encoder::new(0, interval);
    let mut packet = Vec::new();
    frames
        .iter()
        .map(|f| {
            let bytes = enc.encode(f).unwrap().0;
            packet.clear();
            push_record(&mut packet, &bytes);
            packet.len()
        })
        .sum()
}
fn audit(frames: &[Frame]) -> usize {
    BILL.set(Some(Bill::default()));
    let bytes = wire(frames, 150);
    let bill = BILL.take().unwrap();
    println!("list.field                       delta_B       key_B   B/player/s");
    let mut sum = 0;
    for ((list, name), [d, k]) in bill.bits {
        sum += d + k;
        println!(
            "{:30} {:11.3} {:11.3} {:10.4}",
            format!("{}.{}", NAMES[list], name),
            d as f64 / 8.,
            k as f64 / 8.,
            (d + k) as f64 / (8. * 9600.)
        );
    }
    assert_eq!(sum, bytes * 8);
    println!(
        "Matched residuals incl implicit zeros; H0/H1 fitted on this match (ideal bits, no tables). Keys, new entities, absolute hits and framing are outside this conditional estimate."
    );
    for ((l, i, axis), h) in bill.residuals {
        let (h0, h1) = h.bits();
        println!(
            "H {}.{}[{}] N={} H0_bits={:.1} H1_bits={:.1}",
            NAMES[l],
            FIELDS[l][i],
            axis,
            h.totals.values().sum::<usize>(),
            h0,
            h1
        );
    }
    bytes
}
#[test]
fn bit_bill_reconciles_framing_and_padding() {
    audit(&crate::samples::multiplayer(12));
}

#[test]
#[ignore = "release research benchmark, same seed and 600 seconds as bench_match_wire"]
fn bench_live_floor() {
    let frames = crate::samples::multiplayer(9000);
    let baseline = audit(&frames);
    assert_eq!(baseline, 1_394_368, "check the bench_match_wire baseline");
    println!(
        "v7 15Hz {baseline} bytes {:.3} B/player/s",
        baseline as f64 / 9600.
    );
    for (label, pos, aim) in [("pos 1/256", true, false), ("unit aim v6", false, true)] {
        COARSE_POSITION.set(pos);
        COARSE_AIM.set(aim);
        let bytes = wire(&frames, 150);
        let mut errors = Vec::new();
        for p in frames.iter().flat_map(|f| &f.players) {
            if pos {
                let q = dpos(qpos(p.pos).unwrap());
                errors.push(distance(p.pos, q));
            } else {
                let a = p.aim.unwrap();
                let u = unoct_grid(oct_grid(a, 4094.).unwrap(), 4094.);
                errors.push(angle(a, u));
            }
        }
        let (p99, max) = summary(&mut errors);
        println!(
            "{label}: {bytes} bytes {:.3} B/player/s saving {:.3}; sampled player error p99 {p99:.8} max {max:.8} {}",
            bytes as f64 / 9600.,
            (baseline as f64 - bytes as f64) / 9600.,
            if pos { "world units" } else { "degrees" }
        );
        COARSE_POSITION.set(false);
        COARSE_AIM.set(false);
    }
    crate::samples::bench_floor_cadence(&frames, baseline);
}
fn angle(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dot = (0..3).map(|i| a[i] * b[i]).sum::<f64>();
    let norm = |a: [f64; 3]| a.iter().map(|x| x * x).sum::<f64>().sqrt();
    (dot / (norm(a) * norm(b)))
        .clamp(-1., 1.)
        .acos()
        .to_degrees()
}
pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}
pub(crate) fn summary(errors: &mut [f64]) -> (f64, f64) {
    errors.sort_unstable_by(f64::total_cmp);
    (
        errors[(errors.len() * 99).div_ceil(100) - 1],
        errors[errors.len() - 1],
    )
}
