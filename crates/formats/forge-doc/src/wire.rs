use std::{fmt, io};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, DeserializeSeed, SeqAccess, Visitor},
};

use crate::{F32, Refusal, Result};

pub(crate) fn check_size(len: usize, what: &str, max: usize) -> Result<()> {
    if len > max {
        return Err(Refusal::limit(what, max));
    }
    Ok(())
}

pub(crate) fn json_error(error: serde_json::Error) -> Refusal {
    Refusal::Json(error.to_string().chars().take(256).collect())
}

/// Reject the first excess array element before decoding it as a value.
pub(crate) fn bounded<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const N: usize>(std::marker::PhantomData<T>);

    impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for Bounded<T, N> {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "an array with at most {N} items")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<T>, A::Error> {
            let mut items = Vec::new();
            while items.len() < N {
                let Some(item) = seq.next_element()? else {
                    return Ok(items);
                };
                items.push(item);
            }
            seq.next_element_seed(Excess(&self))?;
            Ok(items)
        }
    }

    struct Excess<'a, T, const N: usize>(&'a Bounded<T, N>);

    impl<'de, T: Deserialize<'de>, const N: usize> DeserializeSeed<'de> for Excess<'_, T, N> {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, _: D) -> std::result::Result<(), D::Error> {
            Err(de::Error::invalid_length(N + 1, self.0))
        }
    }

    d.deserialize_seq(Bounded::<T, N>(std::marker::PhantomData))
}

/// Bound serialisation of an in-memory document before building a JSON value
/// tree. Compact serde_json and JCS use the same escapes and integer spellings
/// for this schema. JCS then supplies the canonical key order.
pub(crate) fn measure(value: &impl Serialize, what: &str, max: usize) -> Result<usize> {
    struct Size {
        len: usize,
        max: usize,
        exceeded: bool,
    }

    impl io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.max - self.len {
                self.exceeded = true;
                return Err(io::Error::other("size limit"));
            }
            self.len += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut size = Size {
        len: 0,
        max,
        exceeded: false,
    };
    serde_json::to_writer(&mut size, value).map_err(|error| {
        if size.exceeded {
            Refusal::limit(what, max)
        } else {
            json_error(error)
        }
    })?;
    Ok(size.len)
}

pub(crate) fn canonical(value: &impl Serialize, what: &str, max: usize) -> Result<String> {
    measure(value, what, max)?;
    let value = serde_json::to_value(value).map_err(json_error)?;
    let json = content::canonicalize(&value)
        .map_err(|error| Refusal::invalid("canonical JSON", error.to_string()))?;
    check_size(json.len(), what, max)?;
    Ok(json)
}

impl Serialize for F32 {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(&format_args!("{:08x}", self.0))
    }
}

impl<'de> Deserialize<'de> for F32 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let hex = std::borrow::Cow::<str>::deserialize(d)?;
        if hex.len() != 8
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(de::Error::custom(
                "expected eight lowercase hex digits for a float",
            ));
        }
        let bits = u32::from_str_radix(&hex, 16).map_err(de::Error::custom)?;
        Self::new(f32::from_bits(bits)).map_err(de::Error::custom)
    }
}

pub(crate) mod block {
    use super::*;
    use crate::BlockKind;

    pub fn serialize<S: Serializer>(
        block: &BlockKind,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(block.name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<BlockKind, D::Error> {
        let name = std::borrow::Cow::<str>::deserialize(d)?;
        BlockKind::from_name(&name).ok_or_else(|| de::Error::custom("unknown placement block"))
    }
}
