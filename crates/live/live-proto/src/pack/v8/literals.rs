//! Bounded literals shared by the range-coded columns and their key seeding.
use super::*;

pub(super) fn put(out: &mut Vec<u8>, mut n: u64) {
    while n >= 128 {
        out.push((n as u8) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

pub(super) fn take(input: &mut &[u8]) -> Result<u64> {
    let mut n = 0;
    for shift in (0..64).step_by(7) {
        let (&b, rest) = input.split_first().ok_or(PackError::Short)?;
        *input = rest;
        if (shift == 63 && b > 1) || (b == 0 && shift != 0) {
            return Err(PackError::Bad("invalid v8 varint"));
        }
        n |= u64::from(b & 127) << shift;
        if b < 128 {
            return Ok(n);
        }
    }
    Err(PackError::Bad("v8 varint overflow"))
}

pub(super) fn bounded(n: u64, max: usize) -> Result<usize> {
    if n > max as u64 {
        Err(PackError::Bad("v8 count out of range"))
    } else {
        Ok(n as usize)
    }
}

pub(super) fn blob<T: Symbols>(r: &mut T, ctx: usize, input: &[u8], max: usize) -> Result<Vec<u8>> {
    let n = bounded(r.uint(ctx, input.len() as u64), max)?;
    if T::WRITING {
        for &b in input {
            r.literal(b);
        }
        return Ok(Vec::new());
    }
    Ok((0..n).map(|_| r.literal(0)).collect())
}

fn text(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| PackError::Bad("invalid v8 UTF-8"))
}

pub(super) fn write_meta(f: &Frame) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    put(&mut out, f.game_id.len() as u64);
    out.extend_from_slice(f.game_id.as_bytes());
    put(&mut out, f.bsp.map_or(0, |v| u64::from(v) + 1));
    put(
        &mut out,
        match f.cut {
            Some(v) => v
                .checked_add(1)
                .ok_or(PackError::Value("cut out of range"))?,
            None => 0,
        },
    );
    out.push(u8::from(f.cinematic));
    Ok(out)
}

pub(super) fn read_meta(mut data: &[u8]) -> Result<(String, Option<u32>, Option<u64>, bool)> {
    let n = bounded(take(&mut data)?, MAX_STRING)?;
    let (game, mut rest) = data.split_at_checked(n).ok_or(PackError::Short)?;
    let bsp = take(&mut rest)?
        .checked_sub(1)
        .map(u32::try_from)
        .transpose()
        .map_err(|_| PackError::Bad("bsp out of range"))?;
    let cut = take(&mut rest)?.checked_sub(1);
    let cine = match rest {
        [0] => false,
        [1] => true,
        _ => return Err(PackError::Bad("v8 metadata")),
    };
    Ok((text(game)?, bsp, cut, cine))
}

fn read_literal(kind: Kind, mut bytes: &[u8]) -> Result<Val> {
    match kind {
        Kind::Str => Ok(Val::Str(text(bytes)?)),
        Kind::Id => Ok(Val::Id(Id::parse(&text(bytes)?))),
        Kind::Actions
            if bytes.len() <= MAX_FRAME_ACTIONS
                && bytes.iter().all(|&b| usize::from(b) < ACTIONS.len()) =>
        {
            Ok(Val::List(bytes.to_vec()))
        }
        Kind::Agos => {
            let mut ages = Vec::new();
            while !bytes.is_empty() && ages.len() < MAX_FRAME_ACTIONS {
                let n = take(&mut bytes)?;
                if n > u64::from(u32::MAX) {
                    return Err(PackError::Bad("v8 action age"));
                }
                ages.push(n);
            }
            if bytes.is_empty() {
                Ok(Val::Ints(ages))
            } else {
                Err(PackError::Bad("v8 action ages"))
            }
        }
        _ => Err(PackError::Bad("v8 dictionary literal")),
    }
}

pub(super) fn dictionary<T: Symbols>(
    r: &mut T,
    ctx: usize,
    kind: Kind,
    source: Option<&Val>,
    prev: i64,
    strings: &mut Vec<Val>,
    ids: &mut HashMap<Val, usize>,
) -> Result<(usize, Val)> {
    let want = source
        .and_then(|v| ids.get(v))
        .copied()
        .unwrap_or(strings.len());
    let id = bounded(
        (prev + r.signed(ctx, want as i64 - prev)) as u64,
        strings.len(),
    )?;
    if id == strings.len() {
        // One maximum-size frame can introduce more than MAX_STRINGS typed
        // literals. The writer resets at its next key before accepting more.
        if id >= MAX_STRINGS * 4 {
            return Err(PackError::Bad("v8 dictionary full"));
        }
        let v = if let Some(v) = source {
            let owned;
            let bytes = match v {
                Val::Str(s) => s.as_bytes(),
                Val::Id(id) => {
                    owned = id.text().into_bytes();
                    &owned
                }
                Val::List(v) => v.as_slice(),
                Val::Ints(v) => {
                    let mut b = Vec::new();
                    for &n in v {
                        put(&mut b, n);
                    }
                    owned = b;
                    &owned
                }
                _ => return Err(PackError::Value("invalid v8 literal")),
            };
            blob(r, META + 64, bytes, MAX_STRING)?;
            v.clone()
        } else {
            read_literal(kind, &blob(r, META + 64, &[], MAX_STRING)?)?
        };
        ids.insert(v.clone(), id);
        strings.push(v);
    }
    Ok((
        id,
        if T::WRITING {
            Val::None
        } else {
            strings[id].clone()
        },
    ))
}

pub(super) fn validate_number(kind: Kind, v: [i64; 3], shift: [i64; 3]) -> Result<()> {
    let valid = match kind {
        Kind::Pos => v
            .iter()
            .all(|q| q.unsigned_abs() <= (MAX_COORD * POS_SCALE) as u64),
        Kind::Vel => v
            .iter()
            .all(|q| q.unsigned_abs() <= (MAX_COORD * VEL_SCALE) as u64),
        Kind::Dir => (0..2).all(|i| (0..=DIR_MAX as i64).contains(&(v[i] + shift[i]))),
        Kind::Bits(n) => (0..1i64 << n).contains(&v[0]),
        Kind::AnimPhase => (0..=(MAX_ANIM_FRAME * ANIM_SCALE) as i64).contains(&v[0]),
        _ => (0..=1i64 << 49).contains(&v[0]),
    };
    if valid {
        Ok(())
    } else {
        Err(PackError::Bad("v8 numeric field out of range"))
    }
}
