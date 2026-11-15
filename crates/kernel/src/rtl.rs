//! Pure Rtl helpers: RtlCompareMemoryUlong, RtlTimeFieldsToTime and
//! RtlTimeToTimeFields.
use crate::{
    Action, Core,
    abi::{Args, Value},
    ord,
};
use runtime::{Guest, GuestFault, guest::Access};

const HNS_PER_MS: u64 = 10_000;
const MS_PER_DAY: u64 = 86_400_000;
const HNS_PER_DAY: u64 = HNS_PER_MS * MS_PER_DAY;
/// TIME_FIELDS: Year, Month, Day, Hour, Minute, Second, Milliseconds, Weekday.
const TIME_FIELDS_SIZE: usize = 16;

fn leap(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}
fn month_days(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap(year) => 29,
        _ => 28,
    }
}
/// Leap years in 1..=`year`.
fn leaps_through(year: u64) -> u64 {
    year / 4 - year / 100 + year / 400
}
/// Days from 1601-01-01 to the first of `month` in `year`.
fn days_before(year: u32, month: u32) -> u64 {
    let year = year as u64;
    let mut days = (year - 1601) * 365 + leaps_through(year - 1) - leaps_through(1600);
    for m in 1..month {
        days += month_days(year as u32, m) as u64;
    }
    days
}

impl Core {
    pub(crate) fn rtl_call(&mut self, memory: &mut Guest, a: &Args) -> Result<Action, GuestFault> {
        match a.export.ordinal {
            ord::RTL_COMPARE_MEMORY_ULONG => {
                // RtlCompareMemoryUlong(Source, Length, Pattern): the number of
                // leading bytes, in whole ULONGs, equal to Pattern. Source must
                // be 4-aligned and a trailing partial ULONG is ignored.
                let (source, length, pattern) = (a.get(0), a.get(1), a.get(2));
                if source % 4 != 0 {
                    return Ok(Action::Stop("RtlCompareMemoryUlong unaligned source"));
                }
                let words = length / 4;
                memory.check(source, (words * 4) as usize, Access::Read, 1)?;
                let mut matched = 0;
                while matched < words && memory.u32(source.wrapping_add(matched * 4))? == pattern {
                    matched += 1;
                }
                Ok(Action::value(matched * 4))
            }
            ord::RTL_TIME_FIELDS_TO_TIME => {
                memory.check(a.get(1), 8, Access::Write, 1)?;
                let mut raw = [0u8; TIME_FIELDS_SIZE];
                memory.read_bytes(a.get(0), &mut raw, 1)?;
                let f = |i: usize| u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]) as i16 as i32;
                let (year, month, day) = (f(0), f(1), f(2));
                let (hour, minute, second, ms) = (f(3), f(4), f(5), f(6));
                // Weekday is ignored. Out-of-range fields fail, writing nothing.
                if year < 1601
                    || !(1..=12).contains(&month)
                    || day < 1
                    || day > month_days(year as u32, month as u32) as i32
                    || !(0..24).contains(&hour)
                    || !(0..60).contains(&minute)
                    || !(0..60).contains(&second)
                    || !(0..1000).contains(&ms)
                {
                    return Ok(Action::value(0));
                }
                let days = days_before(year as u32, month as u32) + (day - 1) as u64;
                let millis =
                    ((hour as u64 * 60 + minute as u64) * 60 + second as u64) * 1000 + ms as u64;
                let time = days * HNS_PER_DAY + millis * HNS_PER_MS;
                memory.write_bytes(a.get(1), &time.to_le_bytes(), 1)?;
                Ok(Action::value(1))
            }
            ord::RTL_TIME_TO_TIME_FIELDS => {
                memory.check(a.get(1), TIME_FIELDS_SIZE, Access::Write, 1)?;
                let mut raw = [0u8; 8];
                memory.read_bytes(a.get(0), &mut raw, 1)?;
                let time = i64::from_le_bytes(raw);
                if time < 0 {
                    return Ok(Action::Stop(
                        "RtlTimeToTimeFields of a negative time unverified",
                    ));
                }
                let time = time as u64;
                let mut days = time / HNS_PER_DAY;
                let millis = time % HNS_PER_DAY / HNS_PER_MS;
                let weekday = ((days + 1) % 7) as u16; // 1601-01-01 is a Monday
                let mut year = 1601 + (days / 366) as u32;
                // days / 366 never overshoots; step forward to the right year.
                while days >= days_before(year + 1, 1) {
                    year += 1;
                }
                days -= days_before(year, 1);
                let mut month = 1;
                while days >= month_days(year, month) as u64 {
                    days -= month_days(year, month) as u64;
                    month += 1;
                }
                let fields = [
                    year as u16,
                    month as u16,
                    days as u16 + 1,
                    (millis / 3_600_000) as u16,
                    (millis / 60_000 % 60) as u16,
                    (millis / 1000 % 60) as u16,
                    (millis % 1000) as u16,
                    weekday,
                ];
                let mut out = [0u8; TIME_FIELDS_SIZE];
                for (i, v) in fields.iter().enumerate() {
                    out[i * 2..i * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
                memory.write_bytes(a.get(1), &out, 1)?;
                Ok(Action::Complete(Value::Void))
            }
            _ => Ok(Action::Stop("unrouted service case")),
        }
    }
}
