//! Committed, Rosetta-free fixture set: a selection of the full corpus.
//!
//! `fixtures/sse-fixtures-v1.bin.zst` is a zstd-compressed binary file (layout
//! in `docs/sse-oracle.md`). Case inputs are not stored: they are regenerated
//! from the case id by the same deterministic generator that wrote the corpus
//! (`corpus.rs`), and `regen-fixtures` verifies every regenerated input against
//! the corpus row it came from. Only the outputs are stored, as word diffs
//! against the input.
use crate::{
    Fixture, Observation, State,
    corpus::{
        EDGES, INTEGER_A, INTEGER_B, SEED, SHIFT_COUNTS, STICKY, pair, random_state, shift_state,
        sticky_state, with_mode,
    },
};
use anyhow::{Context, Result, ensure};

pub const MAGIC: &[u8; 8] = b"SSEFIX1\0";
/// Compressed fixture file, embedded so tests need no path or tooling.
pub static DATA: &[u8] = include_bytes!("../fixtures/sse-fixtures-v1.bin.zst");

pub const EDGE_COUNT: usize = 52;
const RANDOM_BASE: u16 = 2704;
const SHIFT_BASE: u16 = 3000;
const INTEGER_BASE: u16 = 3100;
const STICKY_BASE: u16 = 3200;
/// Random cases in the full corpus (`generate` default).
pub const RANDOM_CASES: usize = 64;

/// Case ids: edge `i*52+j`; others offset by the bases above.
pub fn case_name(id: u16) -> String {
    let id = id as usize;
    match id {
        0..2704 => format!("edge-{}-{}", id / 52, id % 52),
        2704..3000 => format!("random-{}", id - 2704),
        3000..3100 => format!("shift-count-{}", SHIFT_COUNTS[id - 3000]),
        3100..3200 => {
            let k = id - 3100;
            format!("integer-{:08x}-{:08x}", INTEGER_A[k / 4], INTEGER_B[k % 4])
        }
        _ => format!("sticky-{}", STICKY[id - 3200]),
    }
}
pub fn case_id(name: &str) -> Option<u16> {
    let num = |s: &str| s.parse::<usize>().ok();
    if let Some(r) = name.strip_prefix("edge-") {
        let (i, j) = r.split_once('-')?;
        return Some((num(i)? * 52 + num(j)?) as u16).filter(|v| *v < 2704);
    }
    if let Some(r) = name.strip_prefix("random-") {
        return Some(RANDOM_BASE + num(r)? as u16).filter(|v| *v < SHIFT_BASE);
    }
    if let Some(r) = name.strip_prefix("shift-count-") {
        let c: u32 = r.parse().ok()?;
        return Some(SHIFT_BASE + SHIFT_COUNTS.iter().position(|x| *x == c)? as u16);
    }
    if let Some(r) = name.strip_prefix("integer-") {
        let (a, b) = r.split_once('-')?;
        let a = INTEGER_A.iter().position(|x| format!("{x:08x}") == a)?;
        let b = INTEGER_B.iter().position(|x| format!("{x:08x}") == b)?;
        return Some(INTEGER_BASE + (a * 4 + b) as u16);
    }
    if let Some(r) = name.strip_prefix("sticky-") {
        let c: u32 = r.parse().ok()?;
        return Some(STICKY_BASE + STICKY.iter().position(|x| *x == c)? as u16);
    }
    None
}

/// All case ids kept, ascending: the complete 52x52 edge matrix (every edge
/// value in both operand positions), every random, shift-count, integer
/// saturation and sticky-mask case in the corpus. Each is stored for all 8
/// RC x FTZ modes.
pub fn selected_cases() -> Vec<u16> {
    let mut v: Vec<u16> = (0..2704).collect();
    v.extend((0..RANDOM_CASES as u16).map(|k| RANDOM_BASE + k));
    v.extend((0..SHIFT_COUNTS.len() as u16).map(|k| SHIFT_BASE + k));
    v.extend((0..20).map(|k| INTEGER_BASE + k));
    v.extend((0..STICKY.len() as u16).map(|k| STICKY_BASE + k));
    v
}

/// Regenerates case inputs (before the RC/FTZ mode bits are applied).
pub struct Inputs {
    random: Vec<State>,
}
impl Default for Inputs {
    fn default() -> Self {
        let mut rng = SEED;
        Self {
            random: (0..RANDOM_CASES).map(|_| random_state(&mut rng)).collect(),
        }
    }
}
impl Inputs {
    pub fn case(&self, id: u16) -> State {
        let n = id as usize;
        match n {
            0..2704 => pair(EDGES[n / 52], EDGES[n % 52]),
            2704..3000 => self.random[n - 2704].clone(),
            3000..3100 => shift_state(SHIFT_COUNTS[n - 3000]),
            3100..3200 => pair(INTEGER_A[(n - 3100) / 4], INTEGER_B[(n - 3100) % 4]),
            _ => sticky_state(STICKY[n - 3200]),
        }
    }
    /// Full input for mode `m = rc*2 + ftz`.
    pub fn input(&self, id: u16, mnemonic: &str, mode: usize) -> State {
        with_mode(
            self.case(id),
            mnemonic,
            (mode / 2) as u32,
            (mode % 2) as u32,
        )
    }
}

/// One form's header in the file.
pub struct FormFixtures {
    pub mnemonic: String,
    pub code: String,
    pub instruction: Vec<u8>,
    pub cases: Vec<(u16, [Observation; 8])>,
}

const EXCEPTION_BIT: u64 = 1 << 63;
const WORDS: usize = 56;

#[allow(clippy::needless_range_loop)]
pub fn encode(forms: &[FormFixtures]) -> Vec<u8> {
    let inputs = Inputs::default();
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&SEED.to_le_bytes());
    out.extend_from_slice(&(forms.len() as u32).to_le_bytes());
    for f in forms {
        for s in [f.mnemonic.as_bytes(), f.code.as_bytes(), &f.instruction] {
            out.push(s.len() as u8);
            out.extend_from_slice(s);
        }
        out.extend_from_slice(&(f.cases.len() as u32).to_le_bytes());
        for (id, observations) in &f.cases {
            out.extend_from_slice(&id.to_le_bytes());
            for (mode, o) in observations.iter().enumerate() {
                let input = inputs.input(*id, &f.mnemonic, mode).words();
                match (&o.state, o.exception) {
                    (Some(state), None) => {
                        let w = state.words();
                        let mut mask = 0u64;
                        for k in 0..WORDS {
                            mask |= ((w[k] != input[k]) as u64) << k;
                        }
                        out.extend_from_slice(&mask.to_le_bytes());
                        for k in 0..WORDS {
                            if mask >> k & 1 == 1 {
                                out.extend_from_slice(&w[k].to_le_bytes());
                            }
                        }
                    }
                    (None, Some(signal)) => {
                        out.extend_from_slice(&(EXCEPTION_BIT | signal as u64).to_le_bytes());
                    }
                    _ => unreachable!("observation has state xor exception"),
                }
            }
        }
    }
    out
}

fn take<'a>(data: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
    ensure!(data.len() >= n, "truncated fixture file");
    let (a, b) = data.split_at(n);
    *data = b;
    Ok(a)
}
fn string(data: &mut &[u8]) -> Result<String> {
    let n = take(data, 1)?[0] as usize;
    Ok(String::from_utf8(take(data, n)?.to_vec())?)
}

/// Upper bound on decoded size (committed file: 76,002,648 bytes).
pub const MAX_DECODED: u64 = 96 << 20;
/// Zstd window bound (the encoder uses 2^27).
const WINDOW_LOG_MAX: u32 = 27;

/// Decompress with a hard size limit and a bounded window. The limit is
/// enforced while streaming, so a small hostile input cannot allocate more.
pub fn decompress(packed: &[u8], limit: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut dec = zstd::stream::Decoder::new(packed)?;
    dec.window_log_max(WINDOW_LOG_MAX)?;
    let mut out = Vec::new();
    dec.take(limit + 1)
        .read_to_end(&mut out)
        .context("decompressing fixtures")?;
    ensure!(
        out.len() as u64 <= limit,
        "fixtures exceed {limit} decoded bytes"
    );
    Ok(out)
}
/// Decompress the embedded file.
pub fn raw() -> Result<Vec<u8>> {
    decompress(DATA, MAX_DECODED)
}

fn valid_case(id: u16) -> bool {
    let n = id as usize;
    n < 2704 + RANDOM_CASES
        || (3000..3000 + SHIFT_COUNTS.len()).contains(&n)
        || (3100..3120).contains(&n)
        || (3200..3200 + STICKY.len()).contains(&n)
}

/// (mnemonic, code, instruction, cases left, previous case id)
type Header = (String, String, Vec<u8>, u32, Option<u16>);

/// Validating reader over decoded bytes; yields fixtures form by form.
/// Every malformed input is an `Err` (after which iteration ends); it never
/// panics. It also rejects trailing bytes, reserved mask bits, zero-signal
/// exceptions, invalid or non-ascending case ids.
pub struct Fixtures {
    data: Vec<u8>,
    pos: usize,
    forms_left: u32,
    inputs: Inputs,
    form: Option<Header>,
    pending: std::collections::VecDeque<Fixture>,
    failed: bool,
}
impl Fixtures {
    pub fn new() -> Result<Self> {
        Self::from_raw(raw()?)
    }
    pub fn from_raw(data: Vec<u8>) -> Result<Self> {
        let mut head = &data[..];
        ensure!(take(&mut head, 8)? == MAGIC, "bad fixture magic");
        ensure!(
            u64::from_le_bytes(take(&mut head, 8)?.try_into()?) == SEED,
            "fixture seed does not match generator"
        );
        let forms_left = u32::from_le_bytes(take(&mut head, 4)?.try_into()?);
        Ok(Self {
            pos: 20,
            data,
            forms_left,
            inputs: Inputs::default(),
            form: None,
            pending: Default::default(),
            failed: false,
        })
    }
    /// Decode the next case into `pending`; `Ok(false)` at a clean end.
    fn next_case(&mut self) -> Result<bool> {
        let data = std::mem::take(&mut self.data);
        let result = self.next_case_in(&data);
        self.data = data;
        result
    }
    fn next_case_in(&mut self, data: &[u8]) -> Result<bool> {
        let mut rest = &data[self.pos..];
        while self.form.as_ref().is_none_or(|f| f.3 == 0) {
            if self.forms_left == 0 {
                ensure!(
                    rest.is_empty(),
                    "{} trailing bytes after fixtures",
                    rest.len()
                );
                return Ok(false);
            }
            self.forms_left -= 1;
            let r = &mut rest;
            let mnemonic = string(r)?;
            let code = string(r)?;
            let n = take(r, 1)?[0] as usize;
            let instruction = take(r, n)?.to_vec();
            let cases = u32::from_le_bytes(take(r, 4)?.try_into()?);
            self.form = Some((mnemonic, code, instruction, cases, None));
        }
        let form = self.form.as_mut().expect("form header read");
        let r = &mut rest;
        let id = u16::from_le_bytes(take(r, 2)?.try_into()?);
        ensure!(valid_case(id), "invalid case id {id}");
        ensure!(
            form.4.is_none_or(|prev| id > prev),
            "case ids not ascending at {id}"
        );
        form.4 = Some(id);
        form.3 -= 1;
        let (mnemonic, code, instruction) = (&form.0, &form.1, &form.2);
        for mode in 0..8 {
            let input = self.inputs.input(id, mnemonic, mode);
            let mask = u64::from_le_bytes(take(r, 8)?.try_into()?);
            let output = if mask & EXCEPTION_BIT != 0 {
                ensure!(
                    mask & !EXCEPTION_BIT & !0xff == 0 && mask & 0xff != 0,
                    "bad exception record {mask:#x}"
                );
                Observation {
                    state: None,
                    exception: Some((mask & 0xff) as u32),
                }
            } else {
                ensure!(mask >> WORDS == 0, "reserved mask bits {mask:#x}");
                let mut w = input.words();
                for (k, word) in w.iter_mut().enumerate() {
                    if mask >> k & 1 == 1 {
                        *word = u32::from_le_bytes(take(r, 4)?.try_into()?);
                    }
                }
                Observation {
                    state: Some(State::from_words(&w)),
                    exception: None,
                }
            };
            self.pending.push_back(Fixture {
                schema: 1,
                backend: "rosetta2-x86_64".into(),
                mnemonic: mnemonic.clone(),
                code: code.clone(),
                instruction: instruction.clone(),
                case: case_name(id),
                input,
                output,
            });
        }
        self.pos = data.len() - rest.len();
        Ok(true)
    }
}
impl Iterator for Fixtures {
    type Item = Result<Fixture>;
    fn next(&mut self) -> Option<Result<Fixture>> {
        loop {
            if let Some(f) = self.pending.pop_front() {
                return Some(Ok(f));
            }
            if self.failed {
                return None;
            }
            match self.next_case() {
                Ok(true) => (),
                Ok(false) => return None,
                Err(e) => {
                    self.failed = true;
                    return Some(Err(e));
                }
            }
        }
    }
}

/// Fallible iteration over the committed fixtures.
pub fn try_fixtures() -> Result<Fixtures> {
    Fixtures::new()
}
/// All committed fixtures, in file order (grouped by form, then case, then
/// RC x FTZ mode with `rc*2 + ftz` ascending). Needs no x86, Rosetta or clang.
/// Panics if the committed file is corrupt; use [`try_fixtures`] to handle that.
pub fn fixtures() -> impl Iterator<Item = Fixture> {
    Fixtures::new()
        .expect("committed fixture file is readable")
        .map(|f| f.expect("committed fixtures are valid"))
}

/// Rosetta + clang usable? Used to skip live generation cleanly.
pub fn rosetta_available() -> bool {
    let ok = |c: &mut std::process::Command| {
        c.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    cfg!(all(target_os = "macos", target_arch = "aarch64"))
        && ok(std::process::Command::new("arch").args(["-x86_64", "/usr/bin/true"]))
        && ok(std::process::Command::new("clang").arg("--version"))
}

/// Does the fixture set keep this case? (Every case the generator emits.)
pub fn keep(_mnemonic: &str, case: &str) -> bool {
    case_id(case).is_some()
}

#[derive(Default)]
pub struct Builder {
    forms: Vec<FormFixtures>,
    index: std::collections::HashMap<(String, Vec<u8>), usize>,
    partial: std::collections::BTreeMap<(usize, u16), [Option<Observation>; 8]>,
    pub rows_seen: usize,
}
impl Builder {
    /// Add one corpus row (any order within a form). Verifies that the input
    /// regenerates exactly from the case id and mode.
    pub fn add(&mut self, inputs: &Inputs, f: Fixture) -> Result<()> {
        let id = case_id(&f.case).with_context(|| format!("unknown case {}", f.case))?;
        let key = (f.code.clone(), f.instruction.clone());
        let next = self.forms.len();
        let form = *self.index.entry(key).or_insert(next);
        if form == next {
            self.forms.push(FormFixtures {
                mnemonic: f.mnemonic.clone(),
                code: f.code.clone(),
                instruction: f.instruction.clone(),
                cases: Vec::new(),
            });
        }
        let rc = (f.input.mxcsr >> 13) & 3;
        let ftz = (f.input.mxcsr >> 15) & 1;
        let mode = (rc * 2 + ftz) as usize;
        ensure!(
            inputs.input(id, &f.mnemonic, mode) == f.input,
            "input of {} {} mode {mode} does not regenerate",
            f.code,
            f.case
        );
        let slot = &mut self.partial.entry((form, id)).or_default()[mode];
        ensure!(slot.is_none(), "duplicate row {} {} {mode}", f.code, f.case);
        *slot = Some(f.output);
        self.rows_seen += 1;
        Ok(())
    }
    pub fn finish(mut self) -> Result<Vec<FormFixtures>> {
        for ((form, id), modes) in std::mem::take(&mut self.partial) {
            let modes: Vec<Observation> = modes.into_iter().flatten().collect();
            ensure!(
                modes.len() == 8,
                "{} {}: expected 8 modes, found {}",
                self.forms[form].code,
                case_name(id),
                modes.len()
            );
            self.forms[form]
                .cases
                .push((id, modes.try_into().ok().unwrap()));
        }
        Ok(self.forms)
    }
}

/// Stream a JSONL corpus, parsing only the rows the selection keeps.
pub fn select_from_corpus(mut corpus: impl std::io::BufRead) -> Result<Vec<FormFixtures>> {
    let inputs = Inputs::default();
    let mut builder = Builder::default();
    let mut line = Vec::new();
    let mut total = 0usize;
    loop {
        line.clear();
        if corpus.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        total += 1;
        let text = std::str::from_utf8(&line)?;
        let field = |name: &str| -> Option<&str> {
            let k = text.find(name)? + name.len();
            Some(&text[k..k + text[k..].find('"')?])
        };
        let (Some(m), Some(c)) = (field("\"mnemonic\":\""), field("\"case\":\"")) else {
            bail_line(total)?;
            unreachable!()
        };
        if keep(m, c) {
            let f: Fixture = serde_json::from_str(text).with_context(|| format!("line {total}"))?;
            builder.add(&inputs, f)?;
        }
    }
    builder.finish()
}
fn bail_line(n: usize) -> Result<()> {
    anyhow::bail!("line {n}: missing mnemonic or case")
}

/// Compress with zstd at the maximum level.
pub fn compress(raw: &[u8]) -> Result<Vec<u8>> {
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 22)?;
    enc.long_distance_matching(true)?;
    enc.window_log(27)?;
    std::io::Write::write_all(&mut enc, raw)?;
    Ok(enc.finish()?)
}
