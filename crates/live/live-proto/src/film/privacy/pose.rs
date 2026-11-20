//! Walk the public pose grammar without retaining the object dictionary. The
//! shape of an object edit does not depend on the previous transform. The v8
//! supplemental range coder does depend on its predictor, reset for each TICK.
use super::stream;

pub(super) fn check(raw: &[u8], packer: Option<&mut crate::pack::Decoder>) -> bool {
    fn take_byte(src: &mut &[u8]) -> Option<u8> {
        let (&b, tail) = src.split_first()?;
        *src = tail;
        Some(b)
    }
    fn var(src: &mut &[u8]) -> Option<u64> {
        stream::read_varint(src).ok()
    }
    fn packed(data: &[u8], embedded: bool, decoder: Option<&mut crate::pack::Decoder>) -> bool {
        if !matches!(data.first(), Some(8 | 24)) || (embedded && data.first() != Some(&24)) {
            return false;
        }
        decoder.is_none_or(|d| {
            if embedded {
                *d = crate::pack::Decoder::new();
            }
            d.decode(data)
                .is_ok_and(|f| f.sid == 0 && (!embedded || f.seq == 0))
        })
    }
    fn shape(mut src: &[u8], packer: Option<&mut crate::pack::Decoder>) -> Option<()> {
        const LANES: u64 = (1 << 24) - 1;
        const NAME: u64 = 1 << 26;
        let op = take_byte(&mut src)?;
        if op == 2 {
            return packed(src, false, packer).then_some(());
        }
        if op > 1 {
            return None;
        }
        let count = var(&mut src)?;
        if count > if op == 0 { 2048 } else { 4096 } {
            return None;
        }
        let mut datum = 0u32;
        for i in 0..count {
            let head = var(&mut src)?;
            let step = u32::try_from(head >> 2).ok()?;
            datum = datum.checked_add(step)?;
            if (i != 0 && step == 0) || datum == u32::MAX {
                return None;
            }
            match head & 3 {
                0 | 1 if op != 0 || head & 3 == 0 => {
                    let mask = var(&mut src)?;
                    if mask & !(LANES | NAME) != 0 {
                        return None;
                    }
                    for _ in 0..(mask & LANES).count_ones() {
                        var(&mut src)?;
                    }
                    if mask & NAME != 0 {
                        let n = usize::try_from(var(&mut src)?).ok()?;
                        if n > 256 {
                            return None;
                        }
                        let (text, tail) = src.split_at_checked(n)?;
                        std::str::from_utf8(text).ok()?;
                        src = tail;
                    }
                }
                2 if op == 1 => {}
                _ => return None,
            }
        }
        match take_byte(&mut src)? {
            0 | 1 => {}
            2 => {
                take_byte(&mut src)?;
                let corners = var(&mut src)?;
                if corners > 256 {
                    return None;
                }
                for _ in 0..3 * corners {
                    u32::try_from(var(&mut src)?).ok()?;
                }
            }
            _ => return None,
        }
        if op == 1 {
            return src.is_empty().then_some(());
        }
        let n = usize::try_from(var(&mut src)?).ok()?;
        if n != src.len() {
            return None;
        }
        if n == 0 {
            if let Some(p) = packer {
                *p = crate::pack::Decoder::new();
            }
            Some(())
        } else {
            packed(src, true, packer).then_some(())
        }
    }
    shape(raw, packer).is_some()
}
