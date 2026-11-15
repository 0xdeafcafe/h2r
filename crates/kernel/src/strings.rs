//! Fixed-width guest descriptors and a bounded guest-varargs formatter.
use crate::ord;
use crate::{
    Action, Core,
    abi::{Args, Value},
    status,
};
use runtime::{
    Guest, GuestFault,
    guest::{Access, FaultReason},
};

pub(crate) fn overflow(address: u32) -> GuestFault {
    GuestFault {
        addr: address,
        len: 1,
        access: Access::Read,
        reason: FaultReason::AddressOverflow,
    }
}
fn address(base: u32, offset: usize) -> Result<u32, GuestFault> {
    base.checked_add(u32::try_from(offset).map_err(|_| overflow(base))?)
        .ok_or_else(|| overflow(base))
}
fn c_string(
    memory: &Guest,
    source: u32,
    wide: bool,
    limit: usize,
) -> Result<Option<Vec<u8>>, GuestFault> {
    let mut bytes = Vec::new();
    let step = if wide { 2 } else { 1 };
    for i in 0..limit / step {
        let p = address(source, i * step)?;
        if wide {
            let v = memory.u16(p)?;
            if v == 0 {
                return Ok(Some(bytes));
            }
            bytes.extend(v.to_le_bytes());
        } else {
            let v = memory.u8(p)?;
            if v == 0 {
                return Ok(Some(bytes));
            }
            bytes.push(v);
        }
    }
    Ok(None)
}
fn descriptor(memory: &Guest, at: u32) -> Result<(u16, u16, u32), GuestFault> {
    let bytes = memory.read::<8>(at)?;
    Ok((
        u16::from_le_bytes(bytes[..2].try_into().unwrap()),
        u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..].try_into().unwrap()),
    ))
}
fn put_descriptor(
    memory: &mut Guest,
    at: u32,
    length: u16,
    maximum: u16,
    buffer: u32,
) -> Result<(), GuestFault> {
    let mut bytes = [0; 8];
    bytes[..2].copy_from_slice(&length.to_le_bytes());
    bytes[2..4].copy_from_slice(&maximum.to_le_bytes());
    bytes[4..].copy_from_slice(&buffer.to_le_bytes());
    memory.write(at, bytes)
}
pub(crate) fn call(core: &mut Core, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
    Ok(match a.export.ordinal {
        ord::RTL_INIT_ANSI_STRING | ord::RTL_INIT_UNICODE_STRING => {
            memory.check(a.get(0), 8, Access::Write, 1)?;
            if a.get(1) == 0 {
                put_descriptor(memory, a.get(0), 0, 0, 0)?;
            } else {
                let wide = a.export.ordinal == ord::RTL_INIT_UNICODE_STRING;
                let terminator = if wide { 2 } else { 1 };
                let Some(bytes) = c_string(memory, a.get(1), wide, u16::MAX as usize)? else {
                    return Ok(Action::Stop("string exceeds descriptor length"));
                };
                if bytes.len() + terminator > u16::MAX as usize {
                    return Ok(Action::Stop("string descriptor overflow"));
                }
                put_descriptor(
                    memory,
                    a.get(0),
                    bytes.len() as u16,
                    (bytes.len() + terminator) as u16,
                    a.get(1),
                )?;
            }
            Action::Complete(Value::Void)
        }
        ord::RTL_FREE_ANSI_STRING | ord::RTL_FREE_UNICODE_STRING => {
            let (_, _, buffer) = descriptor(memory, a.get(0))?;
            memory.check(a.get(0), 8, Access::Write, 1)?;
            if buffer != 0 && !core.memory.free_pool(buffer) {
                return Ok(Action::Stop("freeing an untracked string buffer"));
            }
            put_descriptor(memory, a.get(0), 0, 0, 0)?;
            Action::Complete(Value::Void)
        }
        ord::RTL_EQUAL_STRING => {
            let (len_a, _, buffer_a) = descriptor(memory, a.get(0))?;
            let (len_b, _, buffer_b) = descriptor(memory, a.get(1))?;
            if len_a != len_b {
                return Ok(Action::value(0));
            }
            let mut x = vec![0; len_a as usize];
            let mut y = vec![0; len_b as usize];
            memory.read_bytes(buffer_a, &mut x, 1)?;
            memory.read_bytes(buffer_b, &mut y, 1)?;
            let equal = if a.boolean(2) {
                if !x.is_ascii() || !y.is_ascii() {
                    return Ok(Action::Stop("non-ASCII case mapping requires a code page"));
                }
                x.eq_ignore_ascii_case(&y)
            } else {
                x == y
            };
            Action::value(equal as u32)
        }
        ord::RTL_ANSI_STRING_TO_UNICODE_STRING | ord::RTL_UNICODE_STRING_TO_ANSI_STRING => {
            memory.check(a.get(0), 8, Access::Write, 1)?;
            let (len, max, source) = descriptor(memory, a.get(1))?;
            if len > max
                || a.export.ordinal == ord::RTL_UNICODE_STRING_TO_ANSI_STRING && len % 2 != 0
            {
                return Ok(Action::status(status::INVALID_PARAMETER));
            }
            let mut bytes = vec![0; len as usize];
            memory.read_bytes(source, &mut bytes, 1)?;
            let (output, terminator) = if a.export.ordinal == ord::RTL_ANSI_STRING_TO_UNICODE_STRING
            {
                if !bytes.is_ascii() {
                    return Ok(Action::Stop("ANSI conversion needs configured code page"));
                }
                (
                    bytes.into_iter().flat_map(|v| [v, 0]).collect::<Vec<_>>(),
                    2,
                )
            } else {
                let mut output = Vec::new();
                for pair in bytes.as_chunks::<2>().0 {
                    let c = u16::from_le_bytes([pair[0], pair[1]]);
                    if c > 127 {
                        return Ok(Action::Stop(
                            "Unicode conversion needs configured code page",
                        ));
                    }
                    output.push(c as u8);
                }
                (output, 1)
            };
            if output.len() + terminator > u16::MAX as usize {
                return Ok(Action::status(status::INVALID_PARAMETER));
            }
            let length = output.len();
            let needed = length + terminator;
            let (buffer, maximum) = if a.boolean(2) {
                let buffer = core
                    .memory
                    .allocate_pool(memory, needed as u32, 0x6c74_5352);
                if buffer == 0 {
                    return Ok(Action::status(status::NO_MEMORY));
                }
                (buffer, needed as u16)
            } else {
                let (_, max, buffer) = descriptor(memory, a.get(0))?;
                if (max as usize) < needed {
                    return Ok(Action::Stop(
                        "string conversion overflow/truncation contract unverified",
                    ));
                }
                memory.check(buffer, needed, Access::Write, 1)?;
                (buffer, max)
            };
            let mut output = output;
            output.resize(needed, 0);
            memory.write_bytes(buffer, &output, 1)?;
            put_descriptor(memory, a.get(0), length as u16, maximum, buffer)?;
            Action::status(status::SUCCESS)
        }
        _ => return Ok(Action::Stop("unrouted service case")),
    })
}

pub(crate) fn debug_print(core: &mut Core, memory: &Guest, a: &Args) -> Result<Action, GuestFault> {
    let Some(format) = c_string(memory, a.get(0), false, 4096)? else {
        return Ok(Action::Stop("DbgPrint format exceeds bound"));
    };
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut argument = 0;
    while cursor < format.len() {
        if format[cursor] != b'%' {
            out.push(format[cursor]);
            cursor += 1;
            continue;
        }
        cursor += 1;
        if format.get(cursor) == Some(&b'%') {
            out.push(b'%');
            cursor += 1;
            continue;
        }
        let mut left = false;
        let mut zero = false;
        let mut plus = false;
        let mut alternate = false;
        let mut space = false;
        loop {
            match format.get(cursor) {
                Some(b'-') => left = true,
                Some(b'0') => zero = true,
                Some(b'+') => plus = true,
                Some(b'#') => alternate = true,
                Some(b' ') => space = true,
                _ => break,
            }
            cursor += 1;
        }
        let mut width = 0usize;
        if format.get(cursor) == Some(&b'*') {
            let signed = a.vararg(memory, argument)? as i32;
            argument += 1;
            cursor += 1;
            left |= signed < 0;
            width = signed.unsigned_abs() as usize;
        } else {
            while format.get(cursor).is_some_and(u8::is_ascii_digit) {
                width = width
                    .saturating_mul(10)
                    .saturating_add((format[cursor] - b'0') as usize);
                cursor += 1;
            }
        }
        let mut precision = None;
        if format.get(cursor) == Some(&b'.') {
            cursor += 1;
            let mut n = 0usize;
            if format.get(cursor) == Some(&b'*') {
                let v = a.vararg(memory, argument)? as i32;
                argument += 1;
                cursor += 1;
                if v >= 0 {
                    precision = Some(v as usize);
                }
            } else {
                while format.get(cursor).is_some_and(u8::is_ascii_digit) {
                    n = n
                        .saturating_mul(10)
                        .saturating_add((format[cursor] - b'0') as usize);
                    cursor += 1;
                }
                precision = Some(n);
            }
        }
        if width > 4096 || precision.is_some_and(|n| n > 4096) {
            return Ok(Action::Stop("DbgPrint width exceeds bound"));
        }
        let mut long = false;
        if format.get(cursor) == Some(&b'l') {
            cursor += 1;
            long = true;
        }
        let Some(&specifier) = format.get(cursor) else {
            return Ok(Action::Stop("incomplete DbgPrint format"));
        };
        cursor += 1;
        if long && !matches!(specifier, b'd' | b'i' | b'u' | b'x' | b'X' | b'o')
            || !matches!(
                specifier,
                b'd' | b'i' | b'u' | b'x' | b'X' | b'o' | b'p' | b'c' | b's'
            )
        {
            return Ok(Action::Stop("unsupported DbgPrint conversion"));
        }
        let v = a.vararg(memory, argument)?;
        argument += 1;
        let mut prefix = String::new();
        let mut text = match specifier {
            b's' => {
                if v == 0 {
                    b"(null)".to_vec()
                } else {
                    let mut s = Vec::new();
                    let limit = precision.unwrap_or(4096);
                    for i in 0..limit {
                        let c = memory.u8(address(v, i)?)?;
                        if c == 0 {
                            break;
                        }
                        s.push(c);
                    }
                    if precision.is_none() && s.len() == limit {
                        return Ok(Action::Stop("DbgPrint string exceeds bound"));
                    }
                    s
                }
            }
            b'c' => vec![v as u8],
            _ => {
                let signed = matches!(specifier, b'd' | b'i');
                let magnitude = if signed { (v as i32).unsigned_abs() } else { v };
                if signed {
                    if (v as i32) < 0 {
                        prefix.push('-');
                    } else if plus {
                        prefix.push('+');
                    } else if space {
                        prefix.push(' ');
                    }
                }
                if alternate && magnitude != 0 {
                    match specifier {
                        b'x' => prefix.push_str("0x"),
                        b'X' => prefix.push_str("0X"),
                        _ => {}
                    }
                }
                let mut number = match specifier {
                    b'x' => format!("{magnitude:x}"),
                    b'X' | b'p' => format!("{magnitude:X}"),
                    b'o' => format!("{magnitude:o}"),
                    _ => magnitude.to_string(),
                };
                if magnitude == 0 && precision == Some(0) {
                    number.clear();
                }
                let min = if specifier == b'p' {
                    8
                } else {
                    precision.unwrap_or(0)
                };
                if number.len() < min {
                    number = "0".repeat(min - number.len()) + &number;
                }
                if alternate && specifier == b'o' && !number.starts_with('0') {
                    number.insert(0, '0');
                }
                number.into_bytes()
            }
        };
        if specifier == b's'
            && let Some(n) = precision
        {
            text.truncate(n);
        }
        let padding = width.saturating_sub(prefix.len() + text.len());
        let numeric = !matches!(specifier, b's' | b'c');
        if !left && !(zero && numeric && precision.is_none()) {
            out.extend(std::iter::repeat_n(b' ', padding));
        }
        out.extend(prefix.as_bytes());
        if !left && zero && numeric && precision.is_none() {
            out.extend(std::iter::repeat_n(b'0', padding));
        }
        out.extend(text);
        if left {
            out.extend(std::iter::repeat_n(b' ', padding));
        }
        if out.len() > 4096 {
            return Ok(Action::Stop("DbgPrint output exceeds bound"));
        }
    }
    core.push_log(out);
    Ok(Action::status(status::SUCCESS))
}
