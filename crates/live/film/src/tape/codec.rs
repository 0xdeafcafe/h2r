//! The binary call stream (docs/game/input-film.md, "What the replay depends on").
//!
//! A batch holds calls in order across all streams. A large singleton may
//! span multiple physical chunks (see `tape::read_batch`).
//!
//! ```text
//! u8      MAGIC (never `[`, so a JSON batch is told apart and refused)
//! uleb    operation count, then each name: uleb length, UTF-8 bytes
//! uleb    call count
//! per call:
//!   uleb  operation code (index into the table above)
//!   uleb  thread
//!   uleb  call index, zigzag delta from the call before (the first from 0)
//!   uleb  result << 1 | 1 for an error
//!   uleb  argument count, then each argument
//!   uleb  output count, then each output: a kind byte and
//!           0 raw       uleb length, bytes
//!           1 repeat    nothing: the bytes of the last output of this
//!                       operation, first argument and slot
//!           2 delta     uleb changed bytes, then for each its gap from the
//!                       previous changed position and its new byte; the
//!                       last output had the same length
//! ```
//!
//! The "last output" of a pad read is the handle's last packet, so a pad
//! read costs the bytes that changed. State starts empty at each batch.
use anyhow::{Context, Result, bail, ensure};
use std::collections::HashMap;

use super::Call;

/// The first byte of a binary batch. A JSON batch starts with `[`.
pub const MAGIC: u8 = 0xC7;
const MAX_OPS: u64 = 256;
const MAX_NAME: u64 = 64;
const MAX_CALLS: u64 = 4096;
// A Live service call carries one output per guest write. Bound decoded
// storage (including repeated packets) instead of assuming an ABI slot count.
const MAX_STORAGE: usize = crate::MAX_CHUNK;

pub(super) fn call_storage(call: &Call) -> Result<usize> {
    ensure!(call.op.len() as u64 <= MAX_NAME, "tape operation name too long");
    let storage = call.args.len() * size_of::<u32>()
        + call.outputs.len() * size_of::<Vec<u8>>()
        + call.outputs.iter().map(Vec::len).sum::<usize>();
    Ok(storage)
}

const RAW: u8 = 0;
const REPEAT: u8 = 1;
const DELTA: u8 = 2;

fn put(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push(v as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get(input: &mut &[u8]) -> Result<u64> {
    let mut v = 0u64;
    for shift in (0..70).step_by(7) {
        let (&b, rest) = input.split_first().context("truncated tape batch")?;
        *input = rest;
        ensure!(shift < 63 || b <= 1, "tape number overflows");
        v |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    bail!("tape number is too long")
}

fn take<'a>(input: &mut &'a [u8], len: usize) -> Result<&'a [u8]> {
    let (head, rest) = input
        .split_at_checked(len)
        .context("truncated tape batch")?;
    *input = rest;
    Ok(head)
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}
fn unzigzag(v: u64) -> i64 {
    (v >> 1) as i64 ^ -((v & 1) as i64)
}

/// Where the last output of one slot is remembered.
type Key = (usize, u32, usize);

fn delta_size(old: &[u8], new: &[u8]) -> Option<usize> {
    if old.len() != new.len() {
        return None;
    }
    let mut size = 0;
    let (mut count, mut at) = (0u64, 0usize);
    for (i, (a, b)) in old.iter().zip(new).enumerate() {
        if a != b {
            let mut gap = Vec::new();
            put(&mut gap, (i - at) as u64);
            size += gap.len() + 1;
            at = i;
            count += 1;
        }
    }
    let mut head = Vec::new();
    put(&mut head, count);
    Some(size + head.len())
}

fn put_output(out: &mut Vec<u8>, last: Option<&Vec<u8>>, new: &[u8]) {
    if last.is_some_and(|old| old == new) {
        out.push(REPEAT);
        return;
    }
    let mut raw = Vec::new();
    put(&mut raw, new.len() as u64);
    let raw_size = raw.len() + new.len();
    if let Some(old) = last
        && let Some(size) = delta_size(old, new)
        && size < raw_size
    {
        out.push(DELTA);
        let changed = old.iter().zip(new).filter(|(a, b)| a != b).count();
        put(out, changed as u64);
        let mut at = 0;
        for (i, (a, b)) in old.iter().zip(new).enumerate() {
            if a != b {
                put(out, (i - at) as u64);
                out.push(*b);
                at = i;
            }
        }
        return;
    }
    out.push(RAW);
    put(out, new.len() as u64);
    out.extend_from_slice(new);
}

/// One batch, in call order.
pub fn encode_batch(calls: &[Call]) -> Result<Vec<u8>> {
    ensure!(
        !calls.is_empty() && calls.len() as u64 <= MAX_CALLS,
        "invalid tape batch length"
    );
    let mut names: Vec<&str> = Vec::new();
    let mut codes = HashMap::new();
    for call in calls {
        if !codes.contains_key(call.op.as_str()) {
            codes.insert(call.op.as_str(), names.len());
            names.push(&call.op);
        }
    }
    ensure!(
        names.len() as u64 <= MAX_OPS,
        "too many operations in a tape batch"
    );
    let mut out = vec![MAGIC];
    put(&mut out, names.len() as u64);
    for name in &names {
        ensure!(
            name.len() as u64 <= MAX_NAME,
            "tape operation name too long"
        );
        put(&mut out, name.len() as u64);
        out.extend_from_slice(name.as_bytes());
    }
    put(&mut out, calls.len() as u64);
    let mut last: HashMap<Key, Vec<u8>> = HashMap::new();
    let mut storage = if calls.len() == 1 { usize::MAX } else { MAX_STORAGE };
    let mut previous = 0u64;
    for call in calls {
        let code = codes[call.op.as_str()];
        storage = storage.checked_sub(call_storage(call)?).context("tape batch exceeds film chunk budget")?;
        put(&mut out, code as u64);
        put(&mut out, call.thread.into());
        put(&mut out, zigzag(call.index.wrapping_sub(previous) as i64));
        previous = call.index;
        let (value, error) = match call.result {
            Ok(v) => (v, 0),
            Err(v) => (v, 1),
        };
        put(&mut out, u64::from(value) << 1 | error);
        put(&mut out, call.args.len() as u64);
        for &arg in &call.args {
            put(&mut out, arg.into());
        }
        put(&mut out, call.outputs.len() as u64);
        let handle = call.args.first().copied().unwrap_or(0);
        for (slot, bytes) in call.outputs.iter().enumerate() {
            let key = (code, handle, slot);
            put_output(&mut out, last.get(&key), bytes);
            if calls.len() > 1 {
                last.insert(key, bytes.clone());
            }
        }
    }
    Ok(out)
}

/// The batch's calls, or an error naming what is wrong. Nothing is read
/// past the slice, and no length read from it sizes an allocation before
/// it is checked against the bytes left.
pub fn decode_batch(mut input: &[u8]) -> Result<Vec<Call>> {
    let input = &mut input;
    let (&magic, rest) = input.split_first().context("empty tape batch")?;
    ensure!(
        magic != b'[',
        "the call tape is in the JSON encoding (a breaking change); record the game again"
    );
    ensure!(magic == MAGIC, "unknown tape batch encoding");
    *input = rest;
    let ops = get(input)?;
    ensure!(ops <= MAX_OPS, "too many operations in a tape batch");
    let mut names = Vec::new();
    for _ in 0..ops {
        let len = get(input)?;
        ensure!(len <= MAX_NAME, "tape operation name too long");
        names.push(String::from_utf8(take(input, len as usize)?.to_vec())?);
    }
    let count = get(input)?;
    ensure!(count > 0 && count <= MAX_CALLS, "invalid tape batch length");
    let mut calls = Vec::new();
    let mut last: HashMap<Key, Vec<u8>> = HashMap::new();
    // A singleton has no preceding packet to repeat. Every slot has a
    // length or tag in the input and every raw output consumes its own bytes.
    let mut storage = if count == 1 { usize::MAX } else { MAX_STORAGE };
    let mut previous = 0u64;
    for _ in 0..count {
        let code = usize::try_from(get(input)?)?;
        let op = names
            .get(code)
            .context("unknown tape operation code")?
            .clone();
        let thread = u32::try_from(get(input)?)?;
        previous = previous.wrapping_add(unzigzag(get(input)?) as u64);
        let word = get(input)?;
        ensure!(word >> 33 == 0, "tape result out of range");
        let value = (word >> 1) as u32;
        let result = if word & 1 == 0 { Ok(value) } else { Err(value) };
        let n = get(input)?;
        ensure!(n <= input.len() as u64 && n <= (storage / size_of::<u32>()) as u64, "invalid tape argument count");
        storage -= n as usize * size_of::<u32>();
        let mut args = Vec::new();
        for _ in 0..n {
            args.push(u32::try_from(get(input)?)?);
        }
        let n = get(input)?;
        ensure!(n <= input.len() as u64 && n <= (storage / size_of::<Vec<u8>>()) as u64, "invalid tape output count");
        storage -= n as usize * size_of::<Vec<u8>>();
        let handle = args.first().copied().unwrap_or(0);
        let mut outputs = Vec::new();
        for slot in 0..n as usize {
            let key = (code, handle, slot);
            let (&kind, rest) = input.split_first().context("truncated tape batch")?;
            *input = rest;
            if matches!(kind, REPEAT | DELTA) {
                let old = last.get(&key).context("tape output has no earlier value")?;
                ensure!(old.len() <= storage, "tape output too large");
            }
            let bytes = match kind {
                RAW => {
                    let len = get(input)?;
                    ensure!(len <= storage as u64, "tape output too large");
                    take(input, len as usize)?.to_vec()
                }
                REPEAT => last
                    .get(&key)
                    .context("tape repeat has no earlier output")?
                    .clone(),
                DELTA => {
                    let mut bytes = last
                        .get(&key)
                        .context("tape delta has no earlier output")?
                        .clone();
                    let changed = get(input)?;
                    ensure!(
                        changed <= bytes.len() as u64,
                        "tape delta changes too many bytes"
                    );
                    let mut at = 0usize;
                    for _ in 0..changed {
                        at = at
                            .checked_add(usize::try_from(get(input)?)?)
                            .context("tape delta position overflows")?;
                        let b = take(input, 1)?[0];
                        *bytes.get_mut(at).context("tape delta is past its output")? = b;
                    }
                    bytes
                }
                _ => bail!("unknown tape output kind"),
            };
            storage -= bytes.len();
            if count > 1 {
                last.insert(key, bytes.clone());
            }
            outputs.push(bytes);
        }
        calls.push(Call {
            index: previous,
            op,
            args,
            thread,
            result,
            outputs,
        });
    }
    ensure!(input.is_empty(), "trailing bytes in a tape batch");
    Ok(calls)
}

#[cfg(test)]
mod tests;
